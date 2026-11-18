"""Meow 行为指纹适配：题目、请求载荷与离线评分；模型流量仍由宿主执行。"""
from __future__ import annotations

import json
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "vendor/meow"))

from gpt56_vnext import BUNDLED_BASELINES, __version__
from gpt56_vnext.benchmark import cells_by_id, load_package
from gpt56_vnext.errors import AppError
from gpt56_vnext.normalizers import normalize_answer
from gpt56_vnext.predictive import SCORING_VERSION as PREDICTIVE_VERSION, UNKNOWN
from gpt56_vnext.probability_model import SCORING_VERSION, score_counts
from gpt56_vnext.protocol import build_payload

BASELINES = ROOT / "vendor/meow/gpt56_vnext/baselines" / BUNDLED_BASELINES
MANIFEST = json.loads((BASELINES / "manifest.json").read_text())
PACKAGE = load_package(
    (BASELINES / next(p["file"] for p in MANIFEST["packages"] if p["mode"] == "gpt")).read_text()
)
CELLS = cells_by_id(PACKAGE)


def build_single_jobs(package, tier, request_model):
    """与上游 detector 相同的单题计划，不引入其网络执行依赖。"""
    cells = cells_by_id(package)
    return [
        {"job_id": f"{identity}:{index}", "probe_id": cells[identity]["probe_id"],
         "cell_id": identity, "model": request_model}
        for identity, count in sorted(package["tiers"][tier]["counts"].items())
        for index in range(count)
    ]


def calibration_matches(package, tier):
    if package["engine"]["scoring_version"] == PREDICTIVE_VERSION:
        from gpt56_vnext.predictive_calibration import calibration_matches as predictive_matches
        return predictive_matches(package, tier)
    if package["engine"]["scoring_version"] != SCORING_VERSION:
        return False
    result = package["calibration"].get("tiers", {}).get(tier, {})
    return result.get("status") == "target_met" and result.get("thresholds") == package["tiers"][tier]["thresholds"]


def catalog():
    return {
        "engine_version": __version__,
        "id": PACKAGE["id"],
        "version": PACKAGE["version"],
        "hash": PACKAGE["content_sha256"],
        "models": [
            {"id": m["id"], "label": m.get("name", m["id"])}
            for m in PACKAGE["models"]
            if not m.get("reference_only") and m.get("request_model") != "reference-only:other"
        ],
        "tiers": [
            {"id": tier, "count": sum(value["counts"].values())}
            for tier, value in PACKAGE["tiers"].items()
        ],
    }


def check_plan(plan):
    if plan["hash"] != PACKAGE["content_sha256"]:
        raise AppError("baseline_changed")
    if plan["tier"] not in PACKAGE["tiers"]:
        raise AppError("invalid_tier")
    if plan["claimed_model"] not in [m["id"] for m in PACKAGE["models"]]:
        raise AppError("invalid_claimed_model")


def prepare(data):
    plan = {
        **catalog(),
        "method": "meow",
        "tier": data["tier"],
        "claimed_model": data["claimed_model"],
        "request_model": data["request_model"],
    }
    check_plan(plan)
    if not isinstance(plan["request_model"], str) or not 1 <= len(plan["request_model"].strip()) <= 256:
        raise AppError("invalid_request_model")
    jobs = build_single_jobs(PACKAGE, plan["tier"], plan["request_model"])
    jobs.sort(key=lambda j: (int(j["job_id"].rsplit(":", 1)[1]), j["cell_id"]))
    plan["jobs"] = jobs
    plan["retry_budget"] = (len(jobs) + 1) // 2
    del plan["models"], plan["tiers"]
    return plan


def payload(plan, job_id):
    check_plan(plan)
    job = next((j for j in plan["jobs"] if j["job_id"] == job_id), None)
    if job is None:
        raise AppError("unknown_sample")
    return build_payload("gpt", plan["request_model"], CELLS[job["cell_id"]])


def evaluate(plan, job_id, text):
    check_plan(plan)
    job = next((j for j in plan["jobs"] if j["job_id"] == job_id), None)
    if job is None:
        raise AppError("unknown_sample")
    cell = CELLS[job["cell_id"]]
    category = normalize_answer(text, cell["normalizer"])
    if category != "__INVALID_OUTPUT__" and category not in PACKAGE["fitted"]["cells"][job["cell_id"]]["categories"]:
        # 原评分器同样把未入基线的类别合并到 UNKNOWN。
        category = UNKNOWN
    if category == "__INVALID_OUTPUT__":
        return {"status": "error", "error": "invalid_answer", "category": None}
    return {"status": "ok", "error": None, "category": category}


def score(plan, results):
    check_plan(plan)
    jobs = {j["job_id"]: j for j in build_single_jobs(PACKAGE, plan["tier"], plan["request_model"])}
    counts = defaultdict(Counter)
    seen = set()
    for result in results:
        if result["status"] != "ok":
            continue
        identity = result["job_id"]
        if identity not in jobs or identity in seen:
            raise AppError("duplicate_or_unknown_sample")
        seen.add(identity)
        counts[jobs[identity]["cell_id"]][result["category"]] += 1
    tier = PACKAGE["tiers"][plan["tier"]]
    return score_counts(
        PACKAGE["fitted"],
        dict(counts),
        tier["counts"],
        tier["thresholds"],
        calibrated=calibration_matches(PACKAGE, plan["tier"]),
        claimed_model=plan["claimed_model"],
        completion_ratio=0.6,
    )


def handle(operation, data):
    if operation == "meow_catalog":
        return catalog()
    if operation == "meow_prepare":
        return prepare(data)
    plan = data["plan"]
    check_plan(plan)
    if operation == "meow_payload":
        return payload(plan, data["jobId"])
    if operation == "meow_evaluate":
        return evaluate(plan, data["jobId"], data["text"])
    if operation == "meow_score":
        return score(plan, data["results"])
    raise ValueError("未知评分操作")

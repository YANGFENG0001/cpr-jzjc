"""Build a release archive without the cpr-plugin CLI.

Expands the author manifest the same way `Manifest::normalize_author` does
(contribution IDs, fixed stages, capability versions, secret fields), fills in
the `package` block from the actual files, and writes a reproducible
`<plugin-id>-<version>-<target>.tar.gz` plus its SHA256 file.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import tarfile

ROOT = Path(__file__).resolve().parents[1]
FIXED_STAGES = {"management": ["management"], "maintenance": ["maintenance"]}

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--manifest", type=Path, default=ROOT / "plugin.json")
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--target", default="x86_64-unknown-linux-gnu")
parser.add_argument("--resource-map", action="append", default=[])
parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
args = parser.parse_args()

manifest = json.loads(args.manifest.read_text())
if "package" in manifest:
    raise SystemExit("author manifest must not declare package metadata")
plugin_id = f"{manifest['publisher']}.{manifest['name']}"
contributes = {}
for capability, declaration in manifest.get("contributes", {}).items():
    declaration = dict(declaration)
    declaration.setdefault("id", f"{plugin_id}.{capability.replace('_', '-')}")
    declaration.setdefault("version", 1)
    declaration.setdefault("stages", FIXED_STAGES.get(capability, []))
    declaration.setdefault("inputFormats", [])
    declaration.setdefault("outputFormats", [])
    contributes[capability] = declaration
manifest["contributes"] = contributes
manifest.setdefault("secretFields", [])

mapping = dict(item.split("=", 1) for item in args.resource_map)
sources = {manifest["main"]: args.binary}
for path in manifest["resources"]:
    prefix, _, rest = path.partition("/")
    base = Path(mapping.get(prefix, ROOT / prefix))
    sources[path] = base / rest if rest else base

files = {}
for path, source in sources.items():
    if not source.is_file():
        raise SystemExit(f"missing package file: {path} ({source})")
    files[path] = source.read_bytes()

manifest["package"] = {
    "protocolVersion": 7,
    "target": {
        "os": "linux",
        "architecture": "x86_64" if args.target.startswith("x86_64") else "aarch64",
    },
    "files": {path: hashlib.sha256(data).hexdigest() for path, data in files.items()},
}

encoded = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode()
order = ["plugin.json", manifest["main"], *sorted(manifest["resources"])]
args.output_dir.mkdir(parents=True, exist_ok=True)
destination = args.output_dir / f"{plugin_id}-{manifest['version']}-{args.target}.tar.gz"
with destination.open("wb") as output, gzip.GzipFile(
    fileobj=output, mode="wb", mtime=0, compresslevel=9
) as compressed, tarfile.open(fileobj=compressed, mode="w|") as archive:
    for path in order:
        data = encoded if path == "plugin.json" else files[path]
        entry = tarfile.TarInfo(path)
        entry.size = len(data)
        entry.mode = 0o755 if path == manifest["main"] else 0o644
        entry.mtime = 0
        archive.addfile(entry, io.BytesIO(data))

digest = hashlib.sha256(destination.read_bytes()).hexdigest()
destination.with_name(destination.name + ".sha256").write_text(
    f"{digest}  {destination.name}\n"
)
print(json.dumps({"artifact": destination.name, "bytes": destination.stat().st_size, "sha256": digest}))

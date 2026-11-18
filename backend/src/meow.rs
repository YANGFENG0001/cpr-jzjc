//! Meow 行为指纹体检：独立任务槽、串行单题执行与离线评分。
//!
//! 与 ModelTrace 检测互不共享任务状态；模型流量仍由宿主按账号与计量 Key 发送。
use crate::{
    domain::Identity,
    engine::Engine,
    host::{self, fault, now},
};
use gateway_plugin_sdk::{
    PluginFault,
    call::management::ManagementRequest,
    client::{HostClient, TypedCall},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};

pub const SLOTS: usize = 16;
const NAMESPACE: &str = "meow";
const QUESTION_TIMEOUT: Duration = Duration::from_secs(90);
const ENGINE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Running,
    Paused,
    Completed,
    Stopped,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub slot: usize,
    pub created_at: u64,
    pub updated_at: u64,
    pub account_id: String,
    pub account_name: String,
    pub key_id: String,
    pub key_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    pub plan: Value,
    pub status: Status,
    pub attempts: Vec<Value>,
    pub inflight: Option<String>,
    pub report: Value,
}

impl Run {
    pub fn finished(&self) -> bool {
        matches!(self.status, Status::Completed | Status::Stopped) && self.inflight.is_none()
    }
    fn jobs(&self) -> &[Value] {
        self.plan["jobs"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    fn successful(&self, job: &str) -> bool {
        self.attempts
            .iter()
            .any(|a| a["job_id"] == job && a["status"] == "ok")
    }
    fn next_job(&self) -> Option<String> {
        let ids: Vec<_> = self
            .jobs()
            .iter()
            .filter_map(|j| j["job_id"].as_str())
            .collect();
        if let Some(id) = ids
            .iter()
            .find(|id| !self.attempts.iter().any(|a| a["job_id"] == **id))
        {
            return Some((*id).into());
        }
        let retries = self.attempts.len().saturating_sub(ids.len());
        if retries >= self.plan["retry_budget"].as_u64().unwrap_or(0) as usize {
            return None;
        }
        ids.into_iter()
            .find(|id| !self.successful(id))
            .map(str::to_owned)
    }
    fn reserve(&mut self, job: &str) -> Result<(), PluginFault> {
        if self.status != Status::Ready || self.inflight.is_some() {
            return Err(fault("任务未就绪，请刷新状态后继续"));
        }
        if self.next_job().as_deref() != Some(job) {
            return Err(fault("检测题目顺序发生变化"));
        }
        self.attempts
            .push(json!({"job_id":job,"status":"pending","error":null}));
        self.inflight = Some(job.into());
        self.status = Status::Running;
        Ok(())
    }
    fn finish_attempt(&mut self, mut result: Value) -> Result<(), PluginFault> {
        let job = self
            .inflight
            .take()
            .ok_or_else(|| fault("没有待完成的检测请求"))?;
        result["job_id"] = json!(job);
        let last = self
            .attempts
            .last_mut()
            .ok_or_else(|| fault("缺少检测尝试记录"))?;
        if last["job_id"] != job || last["status"] != "pending" {
            return Err(fault("检测尝试记录不一致"));
        }
        *last = result;
        if matches!(self.status, Status::Paused | Status::Stopped) {
            return Ok(());
        }
        self.status = if self.next_job().is_none() {
            Status::Completed
        } else if self.attempts.last().is_some_and(|a| a["status"] == "error") {
            Status::Paused
        } else {
            Status::Ready
        };
        Ok(())
    }
    fn recover(&mut self) -> Result<(), PluginFault> {
        if self.inflight.is_some() {
            self.finish_attempt(
                json!({"status":"error","error":"上次请求被中断，已计入尝试","category":null}),
            )?;
        }
        if !self.finished() {
            self.status = if self.next_job().is_some() {
                Status::Ready
            } else {
                Status::Completed
            };
        }
        Ok(())
    }
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id, "createdAt": self.created_at, "updatedAt": self.updated_at,
            "accountId": self.account_id, "accountName": self.account_name,
            "keyId": self.key_id, "keyName": self.key_name,
            "requestModel": self.plan["request_model"], "claimedModel": self.plan["claimed_model"],
            "tier": self.plan["tier"], "baselineHash": self.plan["hash"],
            "engineVersion": self.plan["engine_version"],
            "status": self.status, "attempts": self.attempts.len(), "planned": self.jobs().len(),
            "verdict": self.report["verdict"], "model": self.report["model"],
            "reasons": self.report["reasons"],
        })
    }
}

fn slot_from_id(id: &str) -> Result<usize, PluginFault> {
    let slot = id
        .split('-')
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|s| *s < SLOTS);
    if id.len() > 100 || !id.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return Err(fault("任务ID格式错误"));
    }
    slot.ok_or_else(|| fault("任务ID格式错误"))
}

async fn read(host: &HostClient, slot: usize) -> Result<Option<(Run, u64)>, PluginFault> {
    let record = host::state_get(host, NAMESPACE, &format!("run-{slot}")).await?;
    record
        .map(|r| {
            serde_json::from_value(r.value)
                .map(|run| (run, r.version))
                .map_err(|_| fault("Meow检测状态格式错误"))
        })
        .transpose()
}

async fn write(host: &HostClient, run: &Run, version: Option<u64>) -> Result<(), PluginFault> {
    host::state_put(host, NAMESPACE, &format!("run-{}", run.slot), run, version).await?;
    Ok(())
}

async fn find(host: &HostClient, id: &str) -> Result<(Run, u64), PluginFault> {
    let (run, version) = read(host, slot_from_id(id)?)
        .await?
        .ok_or_else(|| fault("检测记录不存在"))?;
    if run.id != id {
        return Err(fault("检测记录已超出 16 条历史保留窗口"));
    }
    Ok((run, version))
}

async fn histories(host: &HostClient) -> Result<Vec<(Run, u64)>, PluginFault> {
    let mut runs = Vec::new();
    for slot in 0..SLOTS {
        if let Some(run) = read(host, slot).await? {
            runs.push(run);
        }
    }
    runs.sort_by_key(|(r, _)| std::cmp::Reverse(r.created_at));
    Ok(runs)
}

fn excerpt(message: &str) -> String {
    let mut end = message.len().min(512);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

fn display_name(email: Option<&str>, name: &str) -> String {
    email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let trimmed = name.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .unwrap_or_default()
        .to_owned()
}

pub struct Meow {
    engine: Mutex<Engine>,
    state: Mutex<()>,
    execution: Semaphore,
}

impl Meow {
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(Engine::new()),
            state: Mutex::new(()),
            execution: Semaphore::new(1),
        }
    }
    async fn engine_call(&self, input: Value) -> Result<Value, PluginFault> {
        self.engine
            .lock()
            .await
            .call_with_timeout(input, ENGINE_TIMEOUT)
            .await
    }
    async fn score(&self, run: &mut Run) -> Result<(), PluginFault> {
        run.report = self
            .engine_call(json!({"op":"meow_score","plan":run.plan,"results":run.attempts}))
            .await?;
        run.updated_at = now();
        Ok(())
    }
    pub async fn handle(&self, call: &TypedCall<ManagementRequest>) -> Result<Value, PluginFault> {
        let host = &call.host;
        let body: Value = if call.payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&call.payload).map_err(|_| fault("请求JSON格式无效"))?
        };
        let query = |key: &str| {
            url::form_urlencoded::parse(call.request.query.as_bytes())
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        };
        let field = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
        match (call.request.method.as_str(), call.request.path.as_str()) {
            ("GET", "api/meow/bootstrap") => self.bootstrap(host).await,
            ("POST", "api/meow/start") => {
                self.start(
                    host,
                    &field("accountId"),
                    &field("keyId"),
                    &field("requestModel"),
                    &field("claimedModel"),
                    &field("tier"),
                )
                .await
            }
            ("POST", "api/meow/step") => self.step(host, &field("id")).await,
            ("POST", "api/meow/control") => {
                self.control(host, &field("id"), &field("action")).await
            }
            ("POST", "api/meow/delete") => self.delete(host, &field("id")).await,
            ("GET", "api/meow/report") => self.report(host, &query("id")).await,
            _ => Err(fault("管理接口不存在")),
        }
    }
    pub async fn bootstrap(&self, host: &HostClient) -> Result<Value, PluginFault> {
        let catalog = self.engine_call(json!({"op":"meow_catalog"})).await?;
        Ok(json!({
            "catalog": catalog,
            "runs": histories(host).await?.iter().map(|(r, _)| r.summary()).collect::<Vec<_>>(),
        }))
    }
    pub async fn start(
        &self,
        host: &HostClient,
        account_id: &str,
        key_id: &str,
        request_model: &str,
        claimed_model: &str,
        tier: &str,
    ) -> Result<Value, PluginFault> {
        if account_id.is_empty()
            || key_id.is_empty()
            || request_model.is_empty()
            || claimed_model.is_empty()
            || tier.is_empty()
        {
            return Err(fault("请选择账号、计量Key、请求模型、对照基线与题档"));
        }
        let account = host::accounts(host)
            .await?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| fault("请选择已启用的 OpenAI 账号"))?;
        if !account.supported || !account.enabled {
            return Err(fault("所选账号未启用或不支持检测"));
        }
        let key = host::keys(host)
            .await?
            .into_iter()
            .find(|k| k.id == key_id)
            .ok_or_else(|| fault("请选择已启用的计量Key"))?;
        let models: gateway_plugin_sdk::call::host::ModelListResult = host::meta(
            host,
            "host.models.list",
            json!({"client_key_id":key_id,"protocol":"openai","client_version":concat!("modeltrace-health/", env!("CARGO_PKG_VERSION"))}),
        )
        .await?;
        if !models.models.contains(&request_model.to_owned()) {
            return Err(fault("当前Key的模型列表不包含所选模型"));
        }
        let plan = self
            .engine_call(json!({"op":"meow_prepare","tier":tier,"claimed_model":claimed_model,"request_model":request_model}))
            .await?;
        let _lock = self.state.lock().await;
        let history = histories(host).await?;
        let slot = (0..SLOTS)
            .find(|s| !history.iter().any(|(r, _)| r.slot == *s))
            .or_else(|| {
                history
                    .iter()
                    .rev()
                    .find(|(r, _)| r.finished())
                    .map(|(r, _)| r.slot)
            })
            .ok_or_else(|| fault("已有16个未结束任务，请先结束一个任务"))?;
        let version = history
            .iter()
            .find(|(r, _)| r.slot == slot)
            .map(|(_, v)| *v);
        let runtime = host::runtime(host, account_id).await?;
        let mut run = Run {
            id: format!("{slot}-{}", now()),
            slot,
            created_at: now(),
            updated_at: now(),
            account_id: account.id.clone(),
            account_name: display_name(account.email.as_deref(), &account.name),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            identity: Some(host::identity(&runtime)),
            plan,
            status: Status::Ready,
            attempts: vec![],
            inflight: None,
            report: Value::Null,
        };
        self.score(&mut run).await?;
        write(host, &run, version).await?;
        Ok(json!({"run": run}))
    }
    pub async fn step(&self, host: &HostClient, id: &str) -> Result<Value, PluginFault> {
        let _execution = self
            .execution
            .try_acquire()
            .map_err(|_| fault("另一个检测请求正在执行，请等待该题完成"))?;
        let (snapshot, job, body) = {
            let _lock = self.state.lock().await;
            let (mut run, version) = find(host, id).await?;
            let runtime = host::runtime(host, &run.account_id).await?;
            if runtime.account_id != run.account_id
                || runtime.provider_id != "openai"
                || !runtime.enabled
            {
                return Err(fault("检测账号已停用或所属账号/平台发生变化"));
            }
            if run.identity.as_ref().is_some_and(|expected| {
                serde_json::to_value(expected).unwrap()
                    != serde_json::to_value(host::identity(&runtime)).unwrap()
            }) {
                return Err(fault("账号身份已改变，请结束当前检测后新建"));
            }
            let job = run.next_job().ok_or_else(|| fault("检测计划已完成"))?;
            let body = self
                .engine_call(json!({"op":"meow_payload","plan":run.plan,"jobId":job}))
                .await?;
            run.reserve(&job)?;
            run.updated_at = now();
            write(host, &run, Some(version)).await?;
            (run, job, body)
        };
        let model = snapshot.plan["request_model"]
            .as_str()
            .ok_or_else(|| fault("检测计划缺少请求模型"))?
            .to_owned();
        let result = match host::execute_raw(
            host,
            &snapshot.account_id,
            &snapshot.key_id,
            &model,
            body,
            QUESTION_TIMEOUT,
        )
        .await
        {
            Ok(reply) => {
                match self
                    .engine_call(json!({"op":"meow_evaluate","plan":snapshot.plan,"jobId":job,"text":reply.text}))
                    .await
                {
                    Ok(mut sample) => {
                        sample["request_id"] = json!(reply.request_id);
                        sample["answer"] = json!(excerpt(&reply.text));
                        sample
                    }
                    Err(error) => json!({"status":"error","error":excerpt(&error.message),"request_id":reply.request_id}),
                }
            }
            Err(message) => json!({"status":"error","error":excerpt(&message),"request_id":null}),
        };
        let _lock = self.state.lock().await;
        let (mut latest, version) = find(host, id).await?;
        latest.finish_attempt(result)?;
        self.score(&mut latest).await?;
        write(host, &latest, Some(version)).await?;
        Ok(json!({"run": latest}))
    }
    pub async fn control(
        &self,
        host: &HostClient,
        id: &str,
        action: &str,
    ) -> Result<Value, PluginFault> {
        if !["pause", "resume", "stop"].contains(&action) {
            return Err(fault("未知的任务控制操作"));
        }
        let _execution = if action == "resume" {
            Some(
                self.execution
                    .try_acquire()
                    .map_err(|_| fault("当前题目尚在完成，请稍后刷新"))?,
            )
        } else {
            None
        };
        let _lock = self.state.lock().await;
        let (mut run, version) = find(host, id).await?;
        if run.finished() {
            return Err(fault("任务已结束，历史报告保持不变"));
        }
        match action {
            "pause" => run.status = Status::Paused,
            "stop" => {
                run.status = Status::Stopped;
                if self.execution.available_permits() > 0 && run.inflight.is_some() {
                    run.recover()?;
                    run.status = Status::Stopped;
                }
            }
            "resume" => run.recover()?,
            _ => unreachable!(),
        }
        self.score(&mut run).await?;
        write(host, &run, Some(version)).await?;
        Ok(json!({"run": run}))
    }
    pub async fn report(&self, host: &HostClient, id: &str) -> Result<Value, PluginFault> {
        let (run, _) = find(host, id).await?;
        Ok(json!({"run": run}))
    }
    pub async fn delete(&self, host: &HostClient, id: &str) -> Result<Value, PluginFault> {
        let _lock = self.state.lock().await;
        let (run, _) = find(host, id).await?;
        if !run.finished() && run.inflight.is_some() {
            return Err(fault("任务仍在执行，请先结束检测"));
        }
        host::state_delete(host, NAMESPACE, &format!("run-{}", run.slot)).await?;
        Ok(json!({"ok": true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Run {
        Run {
            id: "0-1".into(),
            slot: 0,
            created_at: 0,
            updated_at: 0,
            account_id: "a".into(),
            account_name: "a".into(),
            key_id: "k".into(),
            key_name: "k".into(),
            identity: None,
            plan: json!({"jobs":[{"job_id":"a"},{"job_id":"b"}],"retry_budget":1}),
            status: Status::Ready,
            attempts: vec![],
            inflight: None,
            report: Value::Null,
        }
    }

    #[test]
    fn retries_do_not_skip_unsent_jobs() {
        let mut r = run();
        for job in ["a", "b", "a"] {
            r.status = Status::Ready;
            r.reserve(job).unwrap();
            r.finish_attempt(json!({"status":"error"})).unwrap();
        }
        assert_eq!(r.status, Status::Completed);
        assert!(r.next_job().is_none());
    }

    #[test]
    fn pause_survives_completion() {
        let mut r = run();
        r.reserve("a").unwrap();
        r.status = Status::Paused;
        r.finish_attempt(json!({"status":"ok"})).unwrap();
        assert_eq!(r.status, Status::Paused);
    }

    #[test]
    fn interrupted_attempts_are_charged() {
        let mut r = run();
        r.reserve("a").unwrap();
        r.recover().unwrap();
        assert_eq!(r.attempts.len(), 1);
        assert_eq!(r.next_job().as_deref(), Some("b"));
    }

    #[test]
    fn duplicate_dispatch_is_rejected() {
        let mut r = run();
        r.reserve("a").unwrap();
        assert!(r.reserve("a").is_err());
    }
}

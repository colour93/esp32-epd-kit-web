use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::mpsc,
};

use crate::{
    producer::{
        BuiltInSourceManifest, ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger,
    },
    publisher::{ResourcePublisher, SemanticResource},
    state::{SharedState, unix_now},
};

const RESOURCE_TTL_SEC: u64 = 600;
const SOURCE_ID: &str = "codex";
pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "codex.usage",
    title: "Codex Usage",
    description: "读取本机 Codex 登录态与额度窗口",
    configurable: false,
    multi_instance: false,
    auto_sync: true,
    built_in_source: Some(BuiltInSourceManifest {
        id: SOURCE_ID,
        title: "Codex Usage",
        resource_keys: &["codex/default", "codex/metrics"],
    }),
};

#[derive(Clone)]
pub struct CodexControl {
    trigger: mpsc::Sender<ProducerTrigger>,
}

impl CodexControl {
    pub fn spawn(context: ProducerContext) -> Self {
        let (trigger, receiver) = mpsc::channel(8);
        tokio::spawn(supervisor(context.state, context.publisher, receiver));
        Self { trigger }
    }

    pub fn control(&self) -> ProducerControl {
        ProducerControl::new(&MANIFEST, self.trigger.clone())
    }
}

struct AppServer {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    notifications: VecDeque<String>,
}

impl AppServer {
    async fn start(path: &PathBuf, state: Arc<SharedState>) -> Result<Self> {
        let mut child = Command::new(path)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start {} app-server", path.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("app-server stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("app-server stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("app-server stderr unavailable"))?;
        let stderr_state = state.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_state.log("warn", "codex.app-server", line).await;
            }
        });
        let mut server = Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: 1,
            notifications: VecDeque::new(),
        };
        state
            .log("info", "codex", "initializing app-server stdio session")
            .await;
        server.request("initialize", json!({
            "clientInfo": {
                "name": "epd_agent",
                "title": "EPD Agent",
                "version": env!("EPD_AGENT_VERSION")
            },
            "capabilities": {
                "optOutNotificationMethods": ["item/agentMessage/delta", "item/commandExecution/outputDelta"]
            }
        })).await?;
        server.notify("initialized", json!({})).await?;
        state
            .log("info", "codex", "app-server stdio session initialized")
            .await;
        Ok(server)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let line = serde_json::to_vec(&json!({ "method": method, "params": params }))?;
        self.stdin.write_all(&line).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let line = if params.is_null() {
            serde_json::to_vec(&json!({ "id": id, "method": method }))?
        } else {
            serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))?
        };
        self.stdin.write_all(&line).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(20), self.lines.next_line())
                .await
                .context("app-server response timed out")??
                .ok_or_else(|| anyhow!("app-server closed stdout"))?;
            let message: Value = serde_json::from_str(&line).context("decode app-server JSONL")?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    self.notifications.push_back(method.to_owned());
                }
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("app-server {method}: {error}");
            }
            return message
                .get("result")
                .cloned()
                .ok_or_else(|| anyhow!("app-server response has no result"));
        }
    }

    async fn next_notification(&mut self) -> Result<String> {
        if let Some(method) = self.notifications.pop_front() {
            return Ok(method);
        }
        loop {
            let line = self
                .lines
                .next_line()
                .await?
                .ok_or_else(|| anyhow!("app-server closed stdout"))?;
            let message: Value = serde_json::from_str(&line).context("decode app-server JSONL")?;
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                return Ok(method.to_owned());
            }
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn supervisor(
    state: Arc<SharedState>,
    publisher: ResourcePublisher,
    mut manual: mpsc::Receiver<ProducerTrigger>,
) {
    let mut restart_backoff = 1u64;
    state.log("info", "codex", "Codex supervisor started").await;
    loop {
        let path = match find_codex() {
            Ok(path) => path,
            Err(error) => {
                set_codex_error(&state, "missing", error.to_string()).await;
                state
                    .log("warn", "codex", format!("{error}; checking again in 30s"))
                    .await;
                wait_before_restart(&publisher, &mut manual, 30).await;
                continue;
            }
        };
        state
            .log(
                "info",
                "codex",
                format!("starting app-server from {}", path.display()),
            )
            .await;
        state
            .update_source(SOURCE_ID, |source| {
                source.phase = "starting".into();
                source.details["codex_path"] = json!(path.display().to_string());
                source.last_error = None;
            })
            .await;
        match AppServer::start(&path, state.clone()).await {
            Ok(mut server) => {
                restart_backoff = 1;
                if let Err(error) =
                    run_connected(&state, &publisher, &mut server, &mut manual).await
                {
                    set_codex_error(&state, "unavailable", error.to_string()).await;
                    state.log("error", "codex", error.to_string()).await;
                }
            }
            Err(error) => {
                set_codex_error(&state, "unavailable", error.to_string()).await;
                state.log("error", "codex", error.to_string()).await;
            }
        }
        state
            .log(
                "warn",
                "codex",
                format!("restarting app-server in {restart_backoff}s"),
            )
            .await;
        wait_before_restart(&publisher, &mut manual, restart_backoff).await;
        restart_backoff = (restart_backoff * 2).min(60);
    }
}

async fn wait_before_restart(
    publisher: &ResourcePublisher,
    triggers: &mut mpsc::Receiver<ProducerTrigger>,
    delay_sec: u64,
) {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(delay_sec)) => {}
        trigger = triggers.recv() => {
            if let Some(ProducerTrigger::SyncCycle(cycle_id)) = trigger {
                let _ = publisher.complete_cycle(cycle_id, MANIFEST.id, false).await;
            }
        }
    }
}

async fn run_connected(
    state: &SharedState,
    publisher: &ResourcePublisher,
    server: &mut AppServer,
    manual: &mut mpsc::Receiver<ProducerTrigger>,
) -> Result<()> {
    let mut auth_status_logged = false;
    loop {
        state
            .log("info", "codex.rpc", "request method=account/read")
            .await;
        let started = Instant::now();
        let account = server
            .request("account/read", json!({ "refreshToken": false }))
            .await?;
        state
            .log(
                "info",
                "codex.rpc",
                format!(
                    "response method=account/read elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
            )
            .await;
        let account_value = account.get("account").cloned().unwrap_or(Value::Null);
        let account_type = account_value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_owned();
        let supported = matches!(account_type.as_str(), "chatgpt" | "chatgptAuthTokens");
        state
            .update_source(SOURCE_ID, |source| {
                source.phase = if supported {
                    "ready".into()
                } else {
                    "auth_required".into()
                };
                source.details["account_type"] = json!(account_type.clone());
                source.details["email"] =
                    account_value.get("email").cloned().unwrap_or(Value::Null);
                source.details["plan_type"] = account_value
                    .get("planType")
                    .cloned()
                    .unwrap_or(Value::Null);
                source.last_error = if supported {
                    None
                } else {
                    Some("请先在 Codex 中登录 ChatGPT 账号".into())
                };
            })
            .await;
        if supported {
            state
                .log(
                    "info",
                    "codex",
                    format!("Codex account ready; type={account_type}"),
                )
                .await;
            break;
        }
        if !auth_status_logged {
            state
                .log(
                    "warn",
                    "codex",
                    format!("Codex account requires login; type={account_type}"),
                )
                .await;
            auth_status_logged = true;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            trigger = manual.recv() => {
                if let Some(ProducerTrigger::SyncCycle(cycle_id)) = trigger {
                    publisher.complete_cycle(cycle_id, MANIFEST.id, false).await?;
                }
            }
            notification = server.next_notification() => { notification?; }
        }
    }

    let mut delay = 0u64;
    let mut reason = "startup";
    loop {
        let mut cycle_id = None;
        if delay > 0 {
            reason = tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay)) => "poll",
                trigger = manual.recv() => {
                    match trigger {
                        Some(ProducerTrigger::Manual) => "manual",
                        Some(ProducerTrigger::SyncCycle(id)) => {
                            cycle_id = Some(id);
                            "sync_cycle"
                        }
                        None => return Err(anyhow!("producer control channel closed")),
                    }
                }
                notification = server.next_notification() => {
                    if notification? != "account/rateLimits/updated" { continue; }
                    "rateLimits.updated"
                }
            };
        }
        state
            .log(
                "info",
                "codex",
                format!("reading rate limits; reason={reason}"),
            )
            .await;
        let result = sync_once(state, publisher, server).await;
        if let Some(cycle_id) = cycle_id {
            publisher
                .complete_cycle(cycle_id, MANIFEST.id, result.is_ok())
                .await?;
        }
        match result {
            Ok(()) => delay = 60,
            Err(error) => {
                delay = if delay == 0 { 60 } else { (delay * 2).min(900) };
                state
                    .update_source(SOURCE_ID, |source| {
                        source.phase = "degraded".into();
                        source.last_error = Some(error.to_string());
                        source.next_sync_at = Some(unix_now() + delay);
                    })
                    .await;
                state.log("warn", "codex", error.to_string()).await;
            }
        }
    }
}

async fn sync_once(
    state: &SharedState,
    publisher: &ResourcePublisher,
    server: &mut AppServer,
) -> Result<()> {
    let started = Instant::now();
    state
        .log(
            "info",
            "codex.rpc",
            "request method=account/rateLimits/read",
        )
        .await;
    // The current app-server protocol declares this request with no params.
    let rate_limits = server
        .request("account/rateLimits/read", Value::Null)
        .await?;
    state
        .log(
            "info",
            "codex.rpc",
            format!(
                "response method=account/rateLimits/read elapsed_ms={}",
                started.elapsed().as_millis(),
            ),
        )
        .await;
    let selected = select_rate_limit(&rate_limits)
        .cloned()
        .ok_or_else(|| anyhow!("app-server returned no Codex rate-limit bucket"))?;
    let snapshot = state.snapshot().await;
    let account_plan = snapshot
        .sources
        .iter()
        .find(|source| source.id == SOURCE_ID)
        .and_then(|source| source.details.get("plan_type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let plan_type = selected
        .get("planType")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(account_plan)
        .unwrap_or_else(|| "unknown".into());
    let first = selected.get("primary");
    let second = selected.get("secondary");
    let (five_hour, seven_day) = normalized_windows(first, second);
    let payload = json!({
        "source_status": "ok",
        "plan_type": plan_type.clone(),
        "selected": {
            "primary": normalize_window(five_hour),
            "secondary": normalize_window(seven_day),
        },
    });
    let metrics_payload = json!({
        "source_status": "ok",
        "title": "Codex",
        "items": [
            percentage_metric("5h", five_hour),
            percentage_metric("7d", seven_day),
            reset_metric("5h 重置", five_hour),
            reset_metric("7d 重置", seven_day),
        ],
    });
    let now = unix_now();
    publisher
        .publish(SemanticResource {
            source_id: SOURCE_ID.into(),
            key: "codex/default".into(),
            schema_id: "codex.rate_limits",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload,
        })
        .await?;
    publisher
        .publish(SemanticResource {
            source_id: SOURCE_ID.into(),
            key: "codex/metrics".into(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload: metrics_payload,
        })
        .await?;
    state
        .update_source(SOURCE_ID, |source| {
            source.phase = "ready".into();
            source.last_sync_at = Some(now);
            source.next_sync_at = Some(now + 60);
            source.last_error = None;
            source.details["rate_limits"] = rate_limits.clone();
        })
        .await;
    state
        .log(
            "info",
            "codex",
            "rate-limit snapshot ready; next poll in 60s",
        )
        .await;
    Ok(())
}

fn select_rate_limit(rate_limits: &Value) -> Option<&Value> {
    rate_limits
        .get("rateLimitsByLimitId")
        .and_then(|value| value.get("codex"))
        .filter(|value| value.is_object())
        .or_else(|| rate_limits.get("rateLimits"))
        .filter(|value| value.is_object())
}

fn normalized_windows<'a>(
    first: Option<&'a Value>,
    second: Option<&'a Value>,
) -> (Option<&'a Value>, Option<&'a Value>) {
    let windows = [
        first.filter(|value| !value.is_null()),
        second.filter(|value| !value.is_null()),
    ];
    let exact_index = |duration_mins| {
        windows.iter().position(|window| {
            window
                .and_then(|value| value.get("windowDurationMins"))
                .and_then(Value::as_u64)
                == Some(duration_mins)
        })
    };
    let seven_day_index = exact_index(7 * 24 * 60);
    let five_hour_index = exact_index(5 * 60).or_else(|| {
        windows
            .iter()
            .enumerate()
            .find(|(index, window)| window.is_some() && Some(*index) != seven_day_index)
            .map(|(index, _)| index)
    });
    let seven_day_index = seven_day_index.or_else(|| {
        windows
            .iter()
            .enumerate()
            .rev()
            .find(|(index, window)| window.is_some() && Some(*index) != five_hour_index)
            .map(|(index, _)| index)
    });
    (
        five_hour_index.and_then(|index| windows[index]),
        seven_day_index.and_then(|index| windows[index]),
    )
}

fn normalize_window(value: Option<&Value>) -> Value {
    let Some(window) = value.filter(|value| !value.is_null()) else {
        return Value::Null;
    };
    json!({
        "used_percent": window.get("usedPercent").cloned().unwrap_or(json!(0)),
        "window_duration_mins": window.get("windowDurationMins").cloned().unwrap_or(Value::Null),
        "resets_at": window.get("resetsAt").cloned().unwrap_or(Value::Null),
    })
}

fn percentage_metric(label: &str, value: Option<&Value>) -> Value {
    let remaining = value
        .and_then(|window| window.get("usedPercent"))
        .and_then(Value::as_u64)
        .map(|used| 100u64.saturating_sub(used.min(100)));
    json!({
        "label": label,
        "data": remaining.map(Value::from).unwrap_or_else(|| json!("--")),
        "description": "剩余",
        "progress": remaining,
        "format": "percent",
    })
}

fn reset_metric(label: &str, value: Option<&Value>) -> Value {
    let resets_at = value
        .and_then(|window| window.get("resetsAt"))
        .and_then(Value::as_u64);
    json!({
        "label": label,
        "data": resets_at.map(Value::from).unwrap_or_else(|| json!("--")),
        "description": "距重置",
        "format": "countdown",
    })
}

async fn set_codex_error(state: &SharedState, phase: &str, error: String) {
    state
        .update_source(SOURCE_ID, |source| {
            source.phase = phase.into();
            source.last_error = Some(error);
        })
        .await;
}

fn find_codex() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("EPD_CODEX_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
    }
    let executable = if cfg!(windows) { "codex.exe" } else { "codex" };
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(executable);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    let candidates: HashMap<&str, PathBuf> = [
        ("homebrew-arm", PathBuf::from("/opt/homebrew/bin/codex")),
        ("homebrew-intel", PathBuf::from("/usr/local/bin/codex")),
    ]
    .into_iter()
    .collect();
    candidates
        .into_values()
        .find(|path| path.is_file())
        .ok_or_else(|| anyhow!("找不到 codex；请设置 EPD_CODEX_PATH 或把 codex 加入 PATH"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::select_rate_limit;

    #[test]
    fn selects_codex_bucket_from_current_response() {
        let response = json!({
            "rateLimits": { "limitId": "fallback" },
            "rateLimitsByLimitId": { "codex": { "limitId": "codex" } },
        });

        assert_eq!(select_rate_limit(&response).unwrap()["limitId"], "codex");
    }

    #[test]
    fn falls_back_when_bucket_map_is_null() {
        let response = json!({
            "rateLimits": { "limitId": "codex" },
            "rateLimitsByLimitId": null,
        });

        assert_eq!(select_rate_limit(&response).unwrap()["limitId"], "codex");
    }

    #[test]
    fn rejects_null_rate_limit_values() {
        let response = json!({ "rateLimits": null, "rateLimitsByLimitId": null });

        assert!(select_rate_limit(&response).is_none());
    }
}

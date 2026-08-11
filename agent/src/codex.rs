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
    producer::{ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger},
    publisher::{ResourcePublisher, SemanticResource},
    state::{SharedState, unix_now},
};

const RESOURCE_TTL_SEC: u64 = 600;
pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "codex.usage",
    title: "Codex Usage",
    resource_keys: &["codex/default"],
    auto_sync: true,
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
                "version": env!("CARGO_PKG_VERSION")
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
        let line = serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))?;
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
            .update_producer(MANIFEST.id, |producer| {
                producer.phase = "starting".into();
                producer.details["codex_path"] = json!(path.display().to_string());
                producer.last_error = None;
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
            .update_producer(MANIFEST.id, |producer| {
                producer.phase = if supported {
                    "ready".into()
                } else {
                    "auth_required".into()
                };
                producer.details["account_type"] = json!(account_type.clone());
                producer.details["email"] =
                    account_value.get("email").cloned().unwrap_or(Value::Null);
                producer.details["plan_type"] = account_value
                    .get("planType")
                    .cloned()
                    .unwrap_or(Value::Null);
                producer.last_error = if supported {
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
                    .update_producer(MANIFEST.id, |producer| {
                        producer.phase = "degraded".into();
                        producer.last_error = Some(error.to_string());
                        producer.next_sync_at = Some(unix_now() + delay);
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
    let rate_limits = server.request("account/rateLimits/read", json!({})).await?;
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
    let selected = rate_limits
        .get("rateLimitsByLimitId")
        .and_then(|value| value.get("codex"))
        .or_else(|| rate_limits.get("rateLimits"))
        .cloned()
        .ok_or_else(|| anyhow!("app-server returned no Codex rate-limit bucket"))?;
    let limits = rate_limits
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .map(|items| items.values().cloned().collect::<Vec<_>>())
        .unwrap_or_else(|| vec![selected.clone()]);
    let snapshot = state.snapshot().await;
    let account_plan = snapshot
        .producers
        .iter()
        .find(|producer| producer.id == MANIFEST.id)
        .and_then(|producer| producer.details.get("plan_type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let plan_type = selected
        .get("planType")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(account_plan)
        .unwrap_or_else(|| "unknown".into());
    let payload = json!({
        "source_status": "ok",
        "plan_type": plan_type,
        "limit_reached": selected.get("rateLimitReachedType").is_some_and(|value| !value.is_null()),
        "selected": normalize_bucket(&selected),
        "limits": limits.iter().map(normalize_bucket).collect::<Vec<_>>(),
        "rate_limit_reset_credits": rate_limits.get("rateLimitResetCredits").cloned().unwrap_or(Value::Null),
    });
    let now = unix_now();
    publisher
        .publish(SemanticResource {
            producer_id: MANIFEST.id,
            key: "codex/default",
            schema_id: "codex.rate_limits",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload,
        })
        .await?;
    state
        .update_producer(MANIFEST.id, |producer| {
            producer.phase = "ready".into();
            producer.last_sync_at = Some(now);
            producer.next_sync_at = Some(now + 60);
            producer.last_error = None;
            producer.details["rate_limits"] = rate_limits.clone();
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

fn normalize_bucket(bucket: &Value) -> Value {
    json!({
        "limit_id": bucket.get("limitId").cloned().unwrap_or(Value::Null),
        "limit_name": bucket.get("limitName").cloned().unwrap_or(Value::Null),
        "plan_type": bucket.get("planType").cloned().unwrap_or(Value::Null),
        "primary": normalize_window(bucket.get("primary")),
        "secondary": normalize_window(bucket.get("secondary")),
        "rate_limit_reached_type": bucket.get("rateLimitReachedType").cloned().unwrap_or(Value::Null),
        "credits": bucket.get("credits").cloned().unwrap_or(Value::Null),
    })
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

async fn set_codex_error(state: &SharedState, phase: &str, error: String) {
    state
        .update_producer(MANIFEST.id, |producer| {
            producer.phase = phase.into();
            producer.last_error = Some(error);
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

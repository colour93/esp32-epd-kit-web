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
    ble::BleGateway,
    state::{SharedState, unix_now},
};

const RESOURCE_TTL_SEC: u64 = 600;
const RESOURCE_HEARTBEAT_SEC: u64 = 300;

#[derive(Clone)]
pub struct CodexControl {
    trigger: mpsc::Sender<()>,
}

impl CodexControl {
    pub fn spawn(state: Arc<SharedState>, ble: BleGateway) -> Self {
        let (trigger, receiver) = mpsc::channel(8);
        tokio::spawn(supervisor(state, ble, receiver));
        Self { trigger }
    }

    pub async fn refresh(&self) -> Result<()> {
        self.trigger
            .send(())
            .await
            .map_err(|_| anyhow!("Codex supervisor stopped"))
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

async fn supervisor(state: Arc<SharedState>, ble: BleGateway, mut manual: mpsc::Receiver<()>) {
    let mut device_events = ble.subscribe();
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
                tokio::time::sleep(Duration::from_secs(30)).await;
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
            .update_codex(|codex| {
                codex.phase = "starting".into();
                codex.codex_path = Some(path.display().to_string());
                codex.last_error = None;
            })
            .await;
        match AppServer::start(&path, state.clone()).await {
            Ok(mut server) => {
                restart_backoff = 1;
                if let Err(error) =
                    run_connected(&state, &ble, &mut server, &mut manual, &mut device_events).await
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
        tokio::time::sleep(Duration::from_secs(restart_backoff)).await;
        restart_backoff = (restart_backoff * 2).min(60);
    }
}

async fn run_connected(
    state: &SharedState,
    ble: &BleGateway,
    server: &mut AppServer,
    manual: &mut mpsc::Receiver<()>,
    device_events: &mut tokio::sync::broadcast::Receiver<String>,
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
            .update_codex(|codex| {
                codex.phase = if supported {
                    "ready".into()
                } else {
                    "auth_required".into()
                };
                codex.account_type = Some(account_type.clone());
                codex.email = account_value
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                codex.plan_type = account_value
                    .get("planType")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                codex.last_error = if supported {
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
            _ = manual.recv() => {}
            notification = server.next_notification() => { notification?; }
        }
    }

    // The initial read already covers connection and key events queued while
    // app-server authentication was in progress.
    loop {
        match device_events.try_recv() {
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
        }
    }

    let mut delay = 0u64;
    let mut sent_payload_hash = None;
    let mut last_ble_write_at = None;
    let mut reason = "startup";
    loop {
        if delay > 0 {
            reason = tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay)) => "poll",
                _ = manual.recv() => "manual",
                event = device_events.recv() => {
                    match event.as_deref() {
                        Ok("ble.connected") => {
                            sent_payload_hash = None;
                            last_ble_write_at = None;
                            "ble.connected"
                        }
                        Ok("input.key") => "input.key",
                        _ => continue,
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
        let result = sync_once(
            state,
            ble,
            server,
            &mut sent_payload_hash,
            &mut last_ble_write_at,
        )
        .await;
        match result {
            Ok(()) => delay = 60,
            Err(error) => {
                delay = if delay == 0 { 60 } else { (delay * 2).min(900) };
                state
                    .update_codex(|codex| {
                        codex.phase = "degraded".into();
                        codex.last_error = Some(error.to_string());
                        codex.next_sync_at = Some(unix_now() + delay);
                    })
                    .await;
                state.log("warn", "codex", error.to_string()).await;
            }
        }
    }
}

async fn sync_once(
    state: &SharedState,
    ble: &BleGateway,
    server: &mut AppServer,
    sent_payload_hash: &mut Option<u32>,
    last_ble_write_at: &mut Option<u64>,
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
    let account_plan = state.snapshot().await.codex.plan_type;
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
    let payload_hash = crc32fast::hash(&serde_json::to_vec(&payload)?);
    let snapshot = state.snapshot().await;
    let current_revision = snapshot
        .device
        .resources
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some("codex/default"))
        .and_then(|item| item.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let now = unix_now();
    let revision = now.max(current_revision.saturating_add(1));
    let resource = json!({
        "key": "codex/default",
        "schema_id": "codex.rate_limits",
        "schema_version": 1,
        "revision": revision,
        "updated_at": now,
        "ttl_sec": RESOURCE_TTL_SEC,
        "persistence": "snapshot",
        "payload": payload,
    });
    let heartbeat_due =
        last_ble_write_at.is_none_or(|last| now.saturating_sub(last) >= RESOURCE_HEARTBEAT_SEC);
    let should_write = *sent_payload_hash != Some(payload_hash) || heartbeat_due;
    let mut synchronized = false;
    if should_write {
        if ble.is_connected() {
            ble.request("resource.put", json!({ "resource": resource }))
                .await
                .context("sync quota resource over BLE")?;
            *sent_payload_hash = Some(payload_hash);
            *last_ble_write_at = Some(now);
            synchronized = true;
            state
                .log(
                    "info",
                    "codex",
                    format!(
                        "quota resource synchronized; revision={revision} hash={payload_hash:08x}"
                    ),
                )
                .await;
            if let Ok(resources) = ble.request("resource.list", json!({})).await {
                state
                    .update_device(|device| {
                        device.resources = resources
                            .get("resources")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                    })
                    .await;
            }
        } else {
            state
                .log("info", "ble", "quota cached until device reconnects")
                .await;
        }
    } else {
        state
            .log(
                "info",
                "codex",
                format!("quota resource unchanged; hash={payload_hash:08x}"),
            )
            .await;
    }
    let battery_auto_session = snapshot.device.connection_mode == "auto"
        && snapshot
            .device
            .config
            .as_ref()
            .and_then(|config| config.get("power"))
            .and_then(|power| power.get("profile"))
            .and_then(Value::as_str)
            == Some("battery");
    if synchronized && battery_auto_session {
        let result = ble.request("system.sync.complete", json!({})).await?;
        state
            .log(
                "info",
                "ble",
                format!(
                    "battery sync complete; sleep_scheduled={}",
                    result
                        .get("sleep_scheduled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                ),
            )
            .await;
    }
    state
        .update_codex(|codex| {
            codex.phase = "ready".into();
            codex.last_sync_at = Some(now);
            codex.next_sync_at = Some(now + 60);
            codex.last_error = None;
            codex.rate_limits = Some(rate_limits.clone());
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
        .update_codex(|codex| {
            codex.phase = phase.into();
            codex.last_error = Some(error);
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

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    process::Command,
    sync::{RwLock, mpsc},
};

use crate::{
    producer::{ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger},
    publisher::{ResourcePublisher, SemanticResource},
    state::{SharedState, unix_now},
};

const POLL_SEC: u64 = 300;
const RESOURCE_TTL_SEC: u64 = 900;
const COMMAND_TIMEOUT_SEC: u64 = 30;
const MAX_COMMAND_BYTES: usize = 8192;
const MAX_EXPRESSION_BYTES: usize = 1024;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_DISPLAY_NAME_CHARS: usize = 32;
const MAX_VALUE_CHARS: usize = 48;
const MAX_DETAIL_CHARS: usize = 96;

pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "feishu.project",
    title: "Feishu Project",
    resource_keys: &["feishu/default"],
    auto_sync: true,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FeishuConfig {
    pub enabled: bool,
    pub display_name: String,
    pub command: String,
    pub value_expression: String,
    pub detail_expression: String,
}

impl FeishuConfig {
    fn configured(&self) -> bool {
        !self.display_name.trim().is_empty()
            && !self.command.trim().is_empty()
            && !self.value_expression.trim().is_empty()
    }

    fn validate(&self, require_complete: bool) -> Result<()> {
        if self.display_name.chars().count() > MAX_DISPLAY_NAME_CHARS {
            bail!("展示名最多 {MAX_DISPLAY_NAME_CHARS} 个字符");
        }
        if self.command.len() > MAX_COMMAND_BYTES {
            bail!("Meegle 命令不能超过 {MAX_COMMAND_BYTES} bytes");
        }
        if self.value_expression.len() > MAX_EXPRESSION_BYTES
            || self.detail_expression.len() > MAX_EXPRESSION_BYTES
        {
            bail!("JMESPath 表达式不能超过 {MAX_EXPRESSION_BYTES} bytes");
        }
        if require_complete || self.enabled || self.configured() {
            if self.display_name.trim().is_empty() {
                bail!("展示名不能为空");
            }
            parse_command(&self.command)?;
            compile_expression("主值", &self.value_expression)?;
            if !self.detail_expression.trim().is_empty() {
                compile_expression("详情", &self.detail_expression)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FeishuPreview {
    pub source_status: &'static str,
    pub display_name: String,
    pub value: String,
    pub detail: Option<String>,
    pub elapsed_ms: u128,
    pub output_bytes: usize,
}

#[derive(Clone)]
pub struct FeishuControl {
    config: Arc<RwLock<FeishuConfig>>,
    config_path: Arc<PathBuf>,
    trigger: mpsc::Sender<ProducerTrigger>,
}

impl FeishuControl {
    pub fn spawn(context: ProducerContext) -> Result<Self> {
        let config_path = config_path()?;
        let config = Arc::new(RwLock::new(load_config(&config_path)?));
        let (trigger, receiver) = mpsc::channel(8);
        tokio::spawn(run(context, config.clone(), receiver));
        Ok(Self {
            config,
            config_path: Arc::new(config_path),
            trigger,
        })
    }

    pub fn control(&self) -> ProducerControl {
        ProducerControl::new(&MANIFEST, self.trigger.clone())
    }

    pub async fn config(&self) -> FeishuConfig {
        self.config.read().await.clone()
    }

    pub async fn save_config(&self, config: FeishuConfig) -> Result<FeishuConfig> {
        config.validate(false)?;
        save_config(&self.config_path, &config)?;
        *self.config.write().await = config.clone();
        self.trigger
            .send(ProducerTrigger::Manual)
            .await
            .map_err(|_| anyhow!("Feishu producer stopped"))?;
        Ok(config)
    }

    pub async fn test_config(&self, config: FeishuConfig) -> Result<FeishuPreview> {
        config.validate(true)?;
        execute_and_project(&config).await
    }
}

async fn run(
    context: ProducerContext,
    config: Arc<RwLock<FeishuConfig>>,
    mut triggers: mpsc::Receiver<ProducerTrigger>,
) {
    let mut next_poll = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(POLL_SEC),
    );
    next_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let cycle_id = tokio::select! {
            _ = next_poll.tick() => None,
            trigger = triggers.recv() => match trigger {
                Some(ProducerTrigger::Manual) => None,
                Some(ProducerTrigger::SyncCycle(id)) => Some(id),
                None => return,
            },
        };
        let current = config.read().await.clone();
        let result = collect_and_publish(&context.state, &context.publisher, &current).await;
        if let Some(cycle_id) = cycle_id {
            let _ = context
                .publisher
                .complete_cycle(cycle_id, MANIFEST.id, result.is_ok())
                .await;
        }
        if let Err(error) = result {
            let message = error.to_string();
            let phase = if message.contains("未登录") {
                "auth_required"
            } else if message.contains("找不到 meegle") {
                "missing"
            } else {
                "degraded"
            };
            context
                .state
                .update_producer(MANIFEST.id, |producer| {
                    producer.phase = phase.into();
                    producer.last_error = Some(message.clone());
                    producer.next_sync_at = Some(unix_now() + POLL_SEC);
                })
                .await;
            context.state.log("warn", "feishu", message).await;
        }
    }
}

async fn collect_and_publish(
    state: &SharedState,
    publisher: &ResourcePublisher,
    config: &FeishuConfig,
) -> Result<()> {
    config.validate(false)?;
    if !config.configured() {
        publish_status(publisher, "unconfigured", "飞书项目").await?;
        update_non_running(state, "unconfigured", config).await;
        return Ok(());
    }
    if !config.enabled {
        publish_status(publisher, "disabled", &config.display_name).await?;
        update_non_running(state, "disabled", config).await;
        return Ok(());
    }

    state
        .update_producer(MANIFEST.id, |producer| {
            producer.phase = "syncing".into();
            producer.last_error = None;
            producer.details["enabled"] = json!(true);
            producer.details["configured"] = json!(true);
            producer.details["display_name"] = json!(config.display_name.clone());
        })
        .await;
    let preview = execute_and_project(config).await?;
    let mut payload = json!({
        "source_status": preview.source_status,
        "display_name": preview.display_name,
        "value": preview.value,
    });
    if let Some(detail) = &preview.detail {
        payload["detail"] = json!(detail);
    }
    publisher
        .publish(SemanticResource {
            producer_id: MANIFEST.id,
            key: "feishu/default",
            schema_id: "feishu.project_card",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload,
        })
        .await?;

    let now = unix_now();
    state
        .update_producer(MANIFEST.id, |producer| {
            producer.phase = "ready".into();
            producer.last_sync_at = Some(now);
            producer.next_sync_at = Some(now + POLL_SEC);
            producer.last_error = None;
            producer.details["enabled"] = json!(true);
            producer.details["configured"] = json!(true);
            producer.details["display_name"] = json!(config.display_name.clone());
            producer.details["elapsed_ms"] = json!(preview.elapsed_ms);
            producer.details["output_bytes"] = json!(preview.output_bytes);
        })
        .await;
    state
        .log(
            "info",
            "feishu",
            format!("project card ready; elapsed_ms={}", preview.elapsed_ms),
        )
        .await;
    Ok(())
}

async fn update_non_running(state: &SharedState, phase: &str, config: &FeishuConfig) {
    state
        .update_producer(MANIFEST.id, |producer| {
            producer.phase = phase.into();
            producer.next_sync_at = None;
            producer.last_error = None;
            producer.details = json!({
                "enabled": config.enabled,
                "configured": config.configured(),
                "display_name": if config.display_name.trim().is_empty() {
                    "飞书项目"
                } else {
                    config.display_name.trim()
                },
            });
        })
        .await;
}

async fn publish_status(
    publisher: &ResourcePublisher,
    source_status: &'static str,
    display_name: &str,
) -> Result<()> {
    publisher
        .publish(SemanticResource {
            producer_id: MANIFEST.id,
            key: "feishu/default",
            schema_id: "feishu.project_card",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload: json!({
                "source_status": source_status,
                "display_name": display_name,
            }),
        })
        .await?;
    Ok(())
}

async fn execute_and_project(config: &FeishuConfig) -> Result<FeishuPreview> {
    let spec = parse_command(&config.command)?;
    ensure_authenticated(&spec).await?;
    let started = Instant::now();
    let stdout = run_command(&spec).await?;
    let output_bytes = stdout.len();
    serde_json::from_str::<Value>(&stdout).context("Meegle stdout 不是有效 JSON")?;
    let value = evaluate_expression(&stdout, &config.value_expression, MAX_VALUE_CHARS)?;
    let detail = if config.detail_expression.trim().is_empty() {
        None
    } else {
        let projected = evaluate_expression(&stdout, &config.detail_expression, MAX_DETAIL_CHARS)?;
        (!projected.is_empty() && projected != "--").then_some(projected)
    };
    Ok(FeishuPreview {
        source_status: "ok",
        display_name: config.display_name.trim().to_owned(),
        value,
        detail,
        elapsed_ms: started.elapsed().as_millis(),
        output_bytes,
    })
}

#[derive(Debug)]
struct CommandSpec {
    executable: String,
    args: Vec<String>,
    profile_args: Vec<String>,
}

fn parse_command(command: &str) -> Result<CommandSpec> {
    let words = shell_words::split(command).context("Meegle 命令引号不完整")?;
    let executable = words
        .first()
        .ok_or_else(|| anyhow!("Meegle 命令不能为空"))?;
    let executable_name = Path::new(executable)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !matches!(executable_name, "meegle" | "meegle.exe") {
        bail!("命令的可执行文件必须是 meegle");
    }
    let args = words[1..].to_vec();
    let json_format = args.windows(2).any(|pair| pair == ["--format", "json"])
        || args.iter().any(|argument| argument == "--format=json");
    if !json_format {
        bail!("Meegle 命令必须包含 --format json");
    }
    let mut profile_args = Vec::new();
    for (index, argument) in args.iter().enumerate() {
        if argument == "--profile" {
            let profile = args
                .get(index + 1)
                .ok_or_else(|| anyhow!("--profile 缺少值"))?;
            profile_args.extend(["--profile".into(), profile.clone()]);
            break;
        }
        if argument.starts_with("--profile=") {
            profile_args.push(argument.clone());
            break;
        }
    }
    Ok(CommandSpec {
        executable: executable.clone(),
        args,
        profile_args,
    })
}

async fn ensure_authenticated(spec: &CommandSpec) -> Result<()> {
    let mut args = spec.profile_args.clone();
    args.extend([
        "auth".into(),
        "status".into(),
        "--format".into(),
        "json".into(),
    ]);
    let stdout = run_process(&spec.executable, &args).await?;
    let status: Value = serde_json::from_str(&stdout).context("无法解析 Meegle 登录状态")?;
    if status.get("authenticated").and_then(Value::as_bool) != Some(true) {
        bail!("Meegle 未登录；请先在终端执行 meegle auth login --host project.feishu.cn");
    }
    Ok(())
}

async fn run_command(spec: &CommandSpec) -> Result<String> {
    run_process(&spec.executable, &spec.args).await
}

async fn run_process(executable: &str, args: &[String]) -> Result<String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(COMMAND_TIMEOUT_SEC), command.output())
        .await
        .map_err(|_| anyhow!("Meegle 命令执行超过 {COMMAND_TIMEOUT_SEC} 秒"))?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow!("找不到 meegle；请安装 Meegle CLI 或填写其绝对路径")
            } else {
                anyhow!(error)
            }
        })?;
    if output.stdout.len() > MAX_OUTPUT_BYTES {
        bail!("Meegle stdout 超过 {} KiB", MAX_OUTPUT_BYTES / 1024);
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = truncate_text(stderr.trim(), 500);
        bail!("Meegle 命令失败 ({}): {}", output.status, message);
    }
    String::from_utf8(output.stdout).context("Meegle stdout 不是 UTF-8")
}

fn compile_expression(label: &str, expression: &str) -> Result<()> {
    if expression.trim().is_empty() {
        bail!("{label}表达式不能为空");
    }
    jmespath::compile(expression)
        .map(|_| ())
        .with_context(|| format!("{label} JMESPath 无效"))
}

fn evaluate_expression(input: &str, expression: &str, max_chars: usize) -> Result<String> {
    let compiled =
        jmespath::compile(expression).with_context(|| format!("JMESPath 无效: {expression}"))?;
    let data = jmespath::Variable::from_json(input)
        .map_err(|error| anyhow!("无法构造 JMESPath 输入: {error}"))?;
    let projected = compiled
        .search(data)
        .with_context(|| format!("JMESPath 执行失败: {expression}"))?;
    let value: Value =
        serde_json::from_str(&projected.to_string()).context("无法编码 JMESPath 结果")?;
    let text = match value {
        Value::Null => "--".into(),
        Value::String(value) => value,
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        value => serde_json::to_string(&value)?,
    };
    Ok(truncate_text(text.trim(), max_chars))
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn config_path() -> Result<PathBuf> {
    let directory = dirs::config_dir()
        .ok_or_else(|| anyhow!("config directory unavailable"))?
        .join("epd-agent");
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory.join("feishu-project.json"))
}

fn load_config(path: &Path) -> Result<FeishuConfig> {
    if !path.exists() {
        return Ok(FeishuConfig::default());
    }
    let contents = std::fs::read(path).context("读取飞书项目配置失败")?;
    let config: FeishuConfig =
        serde_json::from_slice(&contents).context("飞书项目配置 JSON 无效")?;
    config.validate(false)?;
    Ok(config)
}

fn save_config(path: &Path, config: &FeishuConfig) -> Result<()> {
    let contents = serde_json::to_vec_pretty(config)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).context("打开飞书项目配置失败")?;
    file.write_all(&contents).context("写入飞书项目配置失败")?;
    file.sync_all().context("同步飞书项目配置失败")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

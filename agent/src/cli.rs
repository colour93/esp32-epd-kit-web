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
const MAX_TITLE_CHARS: usize = 32;
const MAX_LABEL_CHARS: usize = 24;
const MAX_DATA_CHARS: usize = 48;
const MAX_DESCRIPTION_CHARS: usize = 96;
const MAX_ITEMS: usize = 4;

pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "cli.jmespath",
    title: "CLI + JMESPath",
    description: "执行 CLI 命令并用 JMESPath 投影 JSON 输出",
    configurable: true,
    multi_instance: true,
    auto_sync: true,
    built_in_source: None,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricFormat {
    #[default]
    Text,
    Percent,
    Countdown,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CliMetricItemConfig {
    pub label: String,
    pub data_expression: String,
    pub description_expression: String,
    pub progress_expression: String,
    #[serde(default)]
    pub format: MetricFormat,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CliMetricConfig {
    pub id: String,
    pub enabled: bool,
    pub title: String,
    pub command: String,
    pub items: Vec<CliMetricItemConfig>,
}

impl Default for CliMetricConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: false,
            title: "CLI 数据".into(),
            command: String::new(),
            items: vec![CliMetricItemConfig {
                label: "数据".into(),
                ..Default::default()
            }],
        }
    }
}

impl CliMetricConfig {
    fn resource_key(&self) -> String {
        format!("cli/{}", self.id)
    }

    fn configured(&self) -> bool {
        !self.title.trim().is_empty()
            && !self.command.trim().is_empty()
            && !self.items.is_empty()
            && self.items.iter().all(|item| {
                !item.label.trim().is_empty() && !item.data_expression.trim().is_empty()
            })
    }

    fn validate(&self, require_complete: bool) -> Result<()> {
        validate_source_id(&self.id)?;
        if self.title.chars().count() > MAX_TITLE_CHARS {
            bail!("标题最多 {MAX_TITLE_CHARS} 个字符");
        }
        if self.command.len() > MAX_COMMAND_BYTES {
            bail!("CLI 命令不能超过 {MAX_COMMAND_BYTES} bytes");
        }
        if self.items.len() > MAX_ITEMS {
            bail!("最多配置 {MAX_ITEMS} 个数据项");
        }
        for (index, item) in self.items.iter().enumerate() {
            if item.label.chars().count() > MAX_LABEL_CHARS {
                bail!(
                    "数据项 {} 的 label 最多 {MAX_LABEL_CHARS} 个字符",
                    index + 1
                );
            }
            for expression in [
                &item.data_expression,
                &item.description_expression,
                &item.progress_expression,
            ] {
                if expression.len() > MAX_EXPRESSION_BYTES {
                    bail!("JMESPath 表达式不能超过 {MAX_EXPRESSION_BYTES} bytes");
                }
            }
        }
        if require_complete || self.enabled || self.configured() {
            if self.title.trim().is_empty() {
                bail!("标题不能为空");
            }
            parse_command(&self.command)?;
            if self.items.is_empty() {
                bail!("至少需要一个数据项");
            }
            for (index, item) in self.items.iter().enumerate() {
                if item.label.trim().is_empty() {
                    bail!("数据项 {} 的 label 不能为空", index + 1);
                }
                compile_expression("data", &item.data_expression)?;
                if !item.description_expression.trim().is_empty() {
                    compile_expression("description", &item.description_expression)?;
                }
                if !item.progress_expression.trim().is_empty() {
                    compile_expression("progress", &item.progress_expression)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CliMetricSourcesFile {
    sources: Vec<CliMetricConfig>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CliMetricPreviewItem {
    pub label: String,
    pub data: Value,
    pub description: Option<String>,
    pub progress: Option<f64>,
    pub format: MetricFormat,
}

#[derive(Clone, Debug, Serialize)]
pub struct CliMetricPreview {
    pub source_status: &'static str,
    pub title: String,
    pub items: Vec<CliMetricPreviewItem>,
    pub elapsed_ms: u128,
    pub output_bytes: usize,
}

#[derive(Clone)]
pub struct CliMetricControl {
    sources: Arc<RwLock<Vec<CliMetricConfig>>>,
    config_path: Arc<PathBuf>,
    trigger: mpsc::Sender<ProducerTrigger>,
    source_trigger: mpsc::Sender<String>,
    publisher: ResourcePublisher,
}

impl CliMetricControl {
    pub fn spawn(context: ProducerContext) -> Result<Self> {
        let config_path = config_path()?;
        let sources = Arc::new(RwLock::new(load_sources(&config_path)?));
        let publisher = context.publisher.clone();
        let (trigger, receiver) = mpsc::channel(8);
        let (source_trigger, source_receiver) = mpsc::channel(16);
        tokio::spawn(run(
            context.clone(),
            sources.clone(),
            receiver,
            source_receiver,
        ));
        Ok(Self {
            sources,
            config_path: Arc::new(config_path),
            trigger,
            source_trigger,
            publisher,
        })
    }

    pub fn control(&self) -> ProducerControl {
        ProducerControl::new(&MANIFEST, self.trigger.clone())
    }

    pub async fn sources(&self) -> Vec<CliMetricConfig> {
        self.sources.read().await.clone()
    }

    pub async fn create_source(
        &self,
        state: &SharedState,
        config: CliMetricConfig,
    ) -> Result<CliMetricConfig> {
        config.validate(false)?;
        if state
            .snapshot()
            .await
            .sources
            .iter()
            .any(|source| source.id == config.id)
        {
            bail!("数据源 ID 已存在：{}", config.id);
        }
        {
            let mut sources = self.sources.write().await;
            if sources.iter().any(|source| source.id == config.id) {
                bail!("数据源 ID 已存在：{}", config.id);
            }
            let mut next = sources.clone();
            next.push(config.clone());
            save_sources(&self.config_path, &next)?;
            *sources = next;
        }
        state.register_source(source_status(&config)).await;
        self.refresh_source(&config.id).await?;
        Ok(config)
    }

    pub async fn update_source(
        &self,
        state: &SharedState,
        id: &str,
        config: CliMetricConfig,
    ) -> Result<CliMetricConfig> {
        if config.id != id {
            bail!("数据源 ID 创建后不可修改");
        }
        config.validate(false)?;
        {
            let mut sources = self.sources.write().await;
            let mut next = sources.clone();
            let current = next
                .iter_mut()
                .find(|source| source.id == id)
                .ok_or_else(|| anyhow!("未知 CLI 数据源：{id}"))?;
            *current = config.clone();
            save_sources(&self.config_path, &next)?;
            *sources = next;
        }
        state.register_source(source_status(&config)).await;
        self.refresh_source(id).await?;
        Ok(config)
    }

    pub async fn delete_source(&self, state: &SharedState, id: &str) -> Result<()> {
        let key = {
            let mut sources = self.sources.write().await;
            let mut next = sources.clone();
            let index = next
                .iter()
                .position(|source| source.id == id)
                .ok_or_else(|| anyhow!("未知 CLI 数据源：{id}"))?;
            let key = next[index].resource_key();
            next.remove(index);
            save_sources(&self.config_path, &next)?;
            *sources = next;
            key
        };
        state.remove_source(id).await;
        self.publisher.delete(key).await
    }

    pub async fn refresh_source(&self, id: &str) -> Result<()> {
        if !self
            .sources
            .read()
            .await
            .iter()
            .any(|source| source.id == id)
        {
            bail!("未知 CLI 数据源：{id}");
        }
        self.source_trigger
            .send(id.to_owned())
            .await
            .map_err(|_| anyhow!("CLI 数据源管理器已停止"))
    }

    pub async fn test_config(&self, config: CliMetricConfig) -> Result<CliMetricPreview> {
        config.validate(true)?;
        execute_and_project(&config).await
    }
}

async fn run(
    context: ProducerContext,
    sources: Arc<RwLock<Vec<CliMetricConfig>>>,
    mut triggers: mpsc::Receiver<ProducerTrigger>,
    mut source_triggers: mpsc::Receiver<String>,
) {
    for source in sources.read().await.iter() {
        context.state.register_source(source_status(source)).await;
    }
    let mut next_poll = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(POLL_SEC),
    );
    next_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        enum Request {
            All(Option<u64>),
            One(String),
        }
        let request = tokio::select! {
            _ = next_poll.tick() => Request::All(None),
            trigger = triggers.recv() => match trigger {
                Some(ProducerTrigger::Manual) => Request::All(None),
                Some(ProducerTrigger::SyncCycle(id)) => Request::All(Some(id)),
                None => return,
            },
            source_id = source_triggers.recv() => match source_id {
                Some(id) => Request::One(id),
                None => return,
            },
        };
        match request {
            Request::All(cycle_id) => {
                let current = sources.read().await.clone();
                let success = collect_all(&context, &current).await;
                if let Some(cycle_id) = cycle_id {
                    let _ = context
                        .publisher
                        .complete_cycle(cycle_id, MANIFEST.id, success)
                        .await;
                }
            }
            Request::One(id) => {
                let current = sources
                    .read()
                    .await
                    .iter()
                    .find(|source| source.id == id)
                    .cloned();
                if let Some(config) = current {
                    collect_with_status(&context, &config).await;
                }
            }
        }
    }
}

async fn collect_all(context: &ProducerContext, sources: &[CliMetricConfig]) -> bool {
    let mut success = true;
    for config in sources {
        success &= collect_with_status(context, config).await;
    }
    success
}

async fn collect_with_status(context: &ProducerContext, config: &CliMetricConfig) -> bool {
    match collect_and_publish(&context.state, &context.publisher, config).await {
        Ok(()) => true,
        Err(error) => {
            let message = error.to_string();
            let phase = if message.contains("找不到可执行文件") {
                "missing"
            } else {
                "degraded"
            };
            context
                .state
                .update_source(&config.id, |source| {
                    source.phase = phase.into();
                    source.last_error = Some(message.clone());
                    source.next_sync_at = Some(unix_now() + POLL_SEC);
                })
                .await;
            context
                .state
                .log("warn", "cli", format!("{}: {message}", config.id))
                .await;
            false
        }
    }
}

async fn collect_and_publish(
    state: &SharedState,
    publisher: &ResourcePublisher,
    config: &CliMetricConfig,
) -> Result<()> {
    config.validate(false)?;
    if !config.configured() {
        publish_status(publisher, config, "unconfigured").await?;
        update_non_running(state, "unconfigured", config).await;
        return Ok(());
    }
    if !config.enabled {
        publish_status(publisher, config, "disabled").await?;
        update_non_running(state, "disabled", config).await;
        return Ok(());
    }

    state
        .update_source(&config.id, |source| {
            source.phase = "syncing".into();
            source.last_error = None;
            source.details["item_count"] = json!(config.items.len());
        })
        .await;
    let preview = execute_and_project(config).await?;
    publisher
        .publish(SemanticResource {
            source_id: config.id.clone(),
            key: config.resource_key(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload: json!({
                "source_status": preview.source_status,
                "title": preview.title,
                "items": preview.items,
            }),
        })
        .await?;

    let now = unix_now();
    state
        .update_source(&config.id, |source| {
            source.phase = "ready".into();
            source.last_sync_at = Some(now);
            source.next_sync_at = Some(now + POLL_SEC);
            source.last_error = None;
            source.details["item_count"] = json!(config.items.len());
            source.details["elapsed_ms"] = json!(preview.elapsed_ms);
            source.details["output_bytes"] = json!(preview.output_bytes);
        })
        .await;
    state
        .log(
            "info",
            "cli",
            format!(
                "{} ready; items={} elapsed_ms={}",
                config.id,
                preview.items.len(),
                preview.elapsed_ms
            ),
        )
        .await;
    Ok(())
}

async fn update_non_running(state: &SharedState, phase: &str, config: &CliMetricConfig) {
    state
        .update_source(&config.id, |source| {
            source.phase = phase.into();
            source.next_sync_at = None;
            source.last_error = None;
            source.details = json!({
                "item_count": config.items.len(),
            });
        })
        .await;
}

async fn publish_status(
    publisher: &ResourcePublisher,
    config: &CliMetricConfig,
    source_status: &'static str,
) -> Result<()> {
    publisher
        .publish(SemanticResource {
            source_id: config.id.clone(),
            key: config.resource_key(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload: json!({
                "source_status": source_status,
                "title": config.title,
                "items": [],
            }),
        })
        .await?;
    Ok(())
}

fn source_status(config: &CliMetricConfig) -> crate::state::SourceStatus {
    crate::state::SourceStatus {
        id: config.id.clone(),
        type_id: MANIFEST.id.into(),
        title: if config.title.trim().is_empty() {
            config.id.clone()
        } else {
            config.title.trim().to_owned()
        },
        enabled: config.enabled,
        phase: if !config.configured() {
            "unconfigured".into()
        } else if config.enabled {
            "starting".into()
        } else {
            "disabled".into()
        },
        resource_keys: vec![config.resource_key()],
        details: json!({ "item_count": config.items.len() }),
        ..Default::default()
    }
}

async fn execute_and_project(config: &CliMetricConfig) -> Result<CliMetricPreview> {
    let spec = parse_command(&config.command)?;
    let started = Instant::now();
    let stdout = run_command(&spec).await?;
    let output_bytes = stdout.len();
    let input: Value = serde_json::from_str(&stdout).context("CLI stdout 不是有效 JSON")?;
    let mut items = Vec::with_capacity(config.items.len());
    for item in &config.items {
        let data = evaluate_value(&input, &item.data_expression, MAX_DATA_CHARS)?;
        let description = if item.description_expression.trim().is_empty() {
            None
        } else {
            let value =
                evaluate_value(&input, &item.description_expression, MAX_DESCRIPTION_CHARS)?;
            display_text(value).filter(|value| !value.is_empty() && value != "--")
        };
        let progress = if item.progress_expression.trim().is_empty() {
            None
        } else {
            Some(evaluate_number(&input, &item.progress_expression)?.clamp(0.0, 100.0))
        };
        items.push(CliMetricPreviewItem {
            label: item.label.trim().to_owned(),
            data,
            description,
            progress,
            format: item.format.clone(),
        });
    }
    Ok(CliMetricPreview {
        source_status: "ok",
        title: config.title.trim().to_owned(),
        items,
        elapsed_ms: started.elapsed().as_millis(),
        output_bytes,
    })
}

#[derive(Debug)]
struct CommandSpec {
    executable: String,
    args: Vec<String>,
}

fn parse_command(command: &str) -> Result<CommandSpec> {
    let words = shell_words::split(command).context("CLI 命令引号不完整")?;
    let executable = words.first().ok_or_else(|| anyhow!("CLI 命令不能为空"))?;
    Ok(CommandSpec {
        executable: executable.clone(),
        args: words[1..].to_vec(),
    })
}

fn validate_source_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 32 {
        bail!("数据源 ID 长度必须为 1-32 个字符");
    }
    if !id.bytes().enumerate().all(|(index, byte)| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || (index > 0 && matches!(byte, b'-' | b'_'))
    }) {
        bail!("数据源 ID 只能包含小写字母、数字、- 和 _，且必须以字母或数字开头");
    }
    if id == "codex" {
        bail!("数据源 ID codex 为内置实例保留");
    }
    Ok(())
}

async fn run_command(spec: &CommandSpec) -> Result<String> {
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(COMMAND_TIMEOUT_SEC), command.output())
        .await
        .map_err(|_| anyhow!("CLI 命令执行超过 {COMMAND_TIMEOUT_SEC} 秒"))?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow!("找不到可执行文件：{}", spec.executable)
            } else {
                anyhow!(error)
            }
        })?;
    if output.stdout.len() > MAX_OUTPUT_BYTES {
        bail!("CLI stdout 超过 {} KiB", MAX_OUTPUT_BYTES / 1024);
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = truncate_text(stderr.trim(), 500);
        bail!("CLI 命令失败 ({}): {}", output.status, message);
    }
    String::from_utf8(output.stdout).context("CLI stdout 不是 UTF-8")
}

fn compile_expression(label: &str, expression: &str) -> Result<()> {
    if expression.trim().is_empty() {
        bail!("{label} 表达式不能为空");
    }
    jmespath::compile(expression)
        .map(|_| ())
        .with_context(|| format!("{label} JMESPath 无效"))
}

fn evaluate_value(input: &Value, expression: &str, max_chars: usize) -> Result<Value> {
    let compiled =
        jmespath::compile(expression).with_context(|| format!("JMESPath 无效: {expression}"))?;
    let data = jmespath::Variable::from_json(&serde_json::to_string(input)?)
        .map_err(|error| anyhow!("无法构造 JMESPath 输入: {error}"))?;
    let projected = compiled
        .search(data)
        .with_context(|| format!("JMESPath 执行失败: {expression}"))?;
    let value: Value =
        serde_json::from_str(&projected.to_string()).context("无法编码 JMESPath 结果")?;
    Ok(match value {
        Value::Null => Value::String("--".into()),
        Value::String(value) => Value::String(truncate_text(value.trim(), max_chars)),
        Value::Bool(_) | Value::Number(_) => value,
        value => Value::String(truncate_text(&serde_json::to_string(&value)?, max_chars)),
    })
}

fn evaluate_number(input: &Value, expression: &str) -> Result<f64> {
    let value = evaluate_value(input, expression, MAX_DATA_CHARS)?;
    match value {
        Value::Number(value) => value
            .as_f64()
            .ok_or_else(|| anyhow!("progress 不是有限数字")),
        Value::String(value) => value
            .parse::<f64>()
            .with_context(|| format!("progress 不是数字: {value}")),
        _ => bail!("progress 必须投影为数字"),
    }
}

fn display_text(value: Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Null => None,
        value => serde_json::to_string(&value).ok(),
    }
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
    Ok(directory.join("cli-sources.json"))
}

fn load_sources(path: &Path) -> Result<Vec<CliMetricConfig>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read(path).context("读取 CLI 数据源实例失败")?;
    let file: CliMetricSourcesFile =
        serde_json::from_slice(&contents).context("CLI 数据源配置 JSON 无效")?;
    for (index, source) in file.sources.iter().enumerate() {
        source
            .validate(false)
            .with_context(|| format!("CLI 数据源实例 {} 无效", index + 1))?;
        if file.sources[..index]
            .iter()
            .any(|registered| registered.id == source.id)
        {
            bail!("重复的 CLI 数据源 ID：{}", source.id);
        }
    }
    Ok(file.sources)
}

fn save_sources(path: &Path, sources: &[CliMetricConfig]) -> Result<()> {
    let contents = serde_json::to_vec_pretty(&CliMetricSourcesFile {
        sources: sources.to_vec(),
    })?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).context("打开 CLI 数据源实例配置失败")?;
    file.write_all(&contents)
        .context("写入 CLI 数据源实例配置失败")?;
    file.sync_all().context("同步 CLI 数据源实例配置失败")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

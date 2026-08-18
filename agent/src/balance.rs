use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{RwLock, mpsc};

use crate::{
    metrics::{MetricFormat, MetricItemConfig, MetricPreview, project_metrics},
    producer::{ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger},
    publisher::{ResourcePublisher, SemanticResource},
    state::{SharedState, SourceStatus, unix_now},
};

const MIN_INTERVAL_SEC: u64 = 60;
const MAX_INTERVAL_SEC: u64 = 86_400;
const MIN_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_SECRET_BYTES: usize = 8192;
const MAX_SOURCES: usize = 16;
const SECRET_SERVICE: &str = "dev.epd-kit.agent.balance";

pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "platform.balance",
    title: "平台余额",
    description: "查询 AI 平台账户余额",
    configurable: true,
    multi_instance: true,
    auto_sync: true,
    built_in_source: None,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BalancePlatform {
    Deepseek,
    Moonshot,
}

impl BalancePlatform {
    fn url(self) -> &'static str {
        match self {
            Self::Deepseek => "https://api.deepseek.com/user/balance",
            Self::Moonshot => "https://api.moonshot.cn/v1/users/me/balance",
        }
    }

    fn items(self) -> Vec<MetricItemConfig> {
        match self {
            Self::Deepseek => vec![
                item(
                    "总余额",
                    "balance_infos[0].total_balance",
                    "balance_infos[0].currency",
                ),
                item(
                    "充值",
                    "balance_infos[0].topped_up_balance",
                    "balance_infos[0].currency",
                ),
                item(
                    "赠送",
                    "balance_infos[0].granted_balance",
                    "balance_infos[0].currency",
                ),
            ],
            Self::Moonshot => vec![
                item("可用", "data.available_balance", ""),
                item("现金", "data.cash_balance", ""),
                item("赠送", "data.voucher_balance", ""),
            ],
        }
    }
}

fn item(label: &str, data_expression: &str, description_expression: &str) -> MetricItemConfig {
    MetricItemConfig {
        label: label.into(),
        data_expression: data_expression.into(),
        description_expression: description_expression.into(),
        progress_expression: String::new(),
        format: MetricFormat::Text,
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BalanceInput {
    pub id: String,
    pub enabled: bool,
    pub title: String,
    pub platform: BalancePlatform,
    pub interval_sec: u64,
    pub timeout_ms: u64,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_secret: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BalanceConfig {
    id: String,
    enabled: bool,
    title: String,
    platform: BalancePlatform,
    interval_sec: u64,
    timeout_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BalanceView {
    pub id: String,
    pub enabled: bool,
    pub title: String,
    pub platform: BalancePlatform,
    pub interval_sec: u64,
    pub timeout_ms: u64,
    pub secret_configured: bool,
}

impl BalanceConfig {
    fn from_input(input: BalanceInput) -> Result<(Self, SecretUpdate)> {
        if input.clear_secret && input.api_key.is_some() {
            bail!("api_key 与 clear_secret 不能同时提交");
        }
        if let Some(secret) = input.api_key.as_ref()
            && (secret.is_empty() || secret.len() > MAX_SECRET_BYTES)
        {
            bail!("API Key 长度必须为 1-{MAX_SECRET_BYTES} bytes");
        }
        let update = if input.clear_secret {
            SecretUpdate::Clear
        } else if let Some(secret) = input.api_key {
            SecretUpdate::Set(secret)
        } else {
            SecretUpdate::Keep
        };
        let config = Self {
            id: input.id,
            enabled: input.enabled,
            title: input.title,
            platform: input.platform,
            interval_sec: input.interval_sec,
            timeout_ms: input.timeout_ms,
        };
        config.validate()?;
        Ok((config, update))
    }

    fn validate(&self) -> Result<()> {
        validate_source_id(&self.id)?;
        if self.title.trim().is_empty() || self.title.chars().count() > 32 {
            bail!("名称长度必须为 1-32 个字符");
        }
        if !(MIN_INTERVAL_SEC..=MAX_INTERVAL_SEC).contains(&self.interval_sec) {
            bail!("interval_sec 必须在 {MIN_INTERVAL_SEC}-{MAX_INTERVAL_SEC} 之间");
        }
        if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&self.timeout_ms) {
            bail!("timeout_ms 必须在 {MIN_TIMEOUT_MS}-{MAX_TIMEOUT_MS} 之间");
        }
        Ok(())
    }

    fn resource_key(&self) -> String {
        format!("balance/{}", self.id)
    }

    fn ttl_sec(&self) -> u64 {
        self.interval_sec.saturating_mul(3).clamp(300, 604_800)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SecretUpdate {
    Keep,
    Set(String),
    Clear,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct BalanceFile {
    sources: Vec<BalanceConfig>,
}

#[derive(Clone)]
pub struct BalanceControl {
    sources: Arc<RwLock<Vec<BalanceConfig>>>,
    config_path: Arc<PathBuf>,
    trigger: mpsc::Sender<ProducerTrigger>,
    source_trigger: mpsc::Sender<String>,
    publisher: ResourcePublisher,
}

impl BalanceControl {
    pub fn spawn(context: ProducerContext) -> Result<Self> {
        let config_path = config_path()?;
        let sources = Arc::new(RwLock::new(load_sources(&config_path)?));
        let publisher = context.publisher.clone();
        let (trigger, receiver) = mpsc::channel(8);
        let (source_trigger, source_receiver) = mpsc::channel(16);
        tokio::spawn(run(context, sources.clone(), receiver, source_receiver));
        Ok(Self {
            sources,
            config_path: Arc::new(config_path),
            trigger,
            source_trigger,
            publisher,
        })
    }

    pub fn control(&self) -> ProducerControl {
        ProducerControl::with_instance_refresh(
            &MANIFEST,
            self.trigger.clone(),
            self.source_trigger.clone(),
        )
    }

    pub async fn sources(&self) -> Result<Vec<BalanceView>> {
        let mut views = Vec::new();
        for config in self.sources.read().await.iter().cloned() {
            let configured = secret_get(config.id.clone()).await?.is_some();
            views.push(view(config, configured));
        }
        Ok(views)
    }

    pub async fn create_source(
        &self,
        state: &SharedState,
        input: BalanceInput,
    ) -> Result<BalanceView> {
        let (config, update) = BalanceConfig::from_input(input)?;
        if !state
            .register_source_if_absent(source_status(&config, false))
            .await
        {
            bail!("数据源 ID 已存在：{}", config.id);
        }
        let result: Result<bool> = async {
            let mut sources = self.sources.write().await;
            if sources.iter().any(|source| source.id == config.id) {
                bail!("数据源 ID 已存在：{}", config.id);
            }
            if sources.len() >= MAX_SOURCES {
                bail!("最多配置 {MAX_SOURCES} 个平台余额数据源");
            }
            let old_secret = secret_get(config.id.clone()).await?;
            let update = match update {
                SecretUpdate::Keep => SecretUpdate::Clear,
                update => update,
            };
            let secret = secret_apply(config.id.clone(), update).await?;
            let mut next = sources.clone();
            next.push(config.clone());
            if let Err(error) = save_sources(&self.config_path, &next) {
                secret_restore(config.id.clone(), old_secret).await?;
                return Err(error);
            }
            *sources = next;
            Ok(secret.is_some())
        }
        .await;
        let configured = match result {
            Ok(configured) => configured,
            Err(error) => {
                state.remove_source(&config.id).await;
                return Err(error);
            }
        };
        state
            .register_source(source_status(&config, configured))
            .await;
        self.refresh_source(&config.id).await?;
        Ok(view(config, configured))
    }

    pub async fn update_source(
        &self,
        state: &SharedState,
        id: &str,
        input: BalanceInput,
    ) -> Result<BalanceView> {
        let (config, update) = BalanceConfig::from_input(input)?;
        if config.id != id {
            bail!("数据源 ID 创建后不可修改");
        }
        let configured = {
            let mut sources = self.sources.write().await;
            let previous = sources
                .iter()
                .find(|source| source.id == id)
                .cloned()
                .ok_or_else(|| anyhow!("未知平台余额数据源：{id}"))?;
            let old_secret = secret_get(id.to_owned()).await?;
            if old_secret.is_some()
                && update == SecretUpdate::Keep
                && previous.platform != config.platform
            {
                bail!("平台已变化，请重新输入或清除 API Key");
            }
            let secret = secret_apply(id.to_owned(), update).await?;
            let mut next = sources.clone();
            let current = next
                .iter_mut()
                .find(|source| source.id == id)
                .ok_or_else(|| anyhow!("未知平台余额数据源：{id}"))?;
            *current = config.clone();
            if let Err(error) = save_sources(&self.config_path, &next) {
                secret_restore(id.to_owned(), old_secret).await?;
                return Err(error);
            }
            *sources = next;
            secret.is_some()
        };
        state
            .register_source(source_status(&config, configured))
            .await;
        self.refresh_source(id).await?;
        Ok(view(config, configured))
    }

    pub async fn delete_source(&self, state: &SharedState, id: &str) -> Result<()> {
        let key = {
            let mut sources = self.sources.write().await;
            let mut next = sources.clone();
            let index = next
                .iter()
                .position(|source| source.id == id)
                .ok_or_else(|| anyhow!("未知平台余额数据源：{id}"))?;
            let key = next[index].resource_key();
            next.remove(index);
            let old_secret = secret_get(id.to_owned()).await?;
            secret_apply(id.to_owned(), SecretUpdate::Clear).await?;
            if let Err(error) = save_sources(&self.config_path, &next) {
                secret_restore(id.to_owned(), old_secret).await?;
                return Err(error);
            }
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
            bail!("未知平台余额数据源：{id}");
        }
        self.source_trigger
            .send(id.to_owned())
            .await
            .map_err(|_| anyhow!("平台余额数据源管理器已停止"))
    }

    pub async fn test_config(&self, input: BalanceInput) -> Result<MetricPreview> {
        let (config, update) = BalanceConfig::from_input(input)?;
        let secret = match update {
            SecretUpdate::Set(secret) => Some(secret),
            SecretUpdate::Clear => None,
            SecretUpdate::Keep => secret_get(config.id.clone()).await?,
        }
        .ok_or_else(|| anyhow!("API Key 未配置"))?;
        execute(&config, &secret).await
    }
}

async fn run(
    context: ProducerContext,
    sources: Arc<RwLock<Vec<BalanceConfig>>>,
    mut triggers: mpsc::Receiver<ProducerTrigger>,
    mut source_triggers: mpsc::Receiver<String>,
) {
    let mut due = HashMap::<String, Instant>::new();
    for source in sources.read().await.iter() {
        let configured = secret_get(source.id.clone()).await.ok().flatten().is_some();
        context
            .state
            .register_source(source_status(source, configured))
            .await;
        due.insert(source.id.clone(), Instant::now());
    }
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        enum Request {
            Due,
            All(Option<u64>),
            One(String),
        }
        let request = tokio::select! {
            _ = tick.tick() => Request::Due,
            trigger = triggers.recv() => match trigger {
                Some(ProducerTrigger::Manual) => Request::All(None),
                Some(ProducerTrigger::SyncCycle(id)) => Request::All(Some(id)),
                None => return,
            },
            source = source_triggers.recv() => match source {
                Some(id) => Request::One(id),
                None => return,
            },
        };
        let current = sources.read().await.clone();
        match request {
            Request::Due => {
                let now = Instant::now();
                let ready = current
                    .iter()
                    .filter(|config| {
                        config.enabled && due.get(&config.id).is_none_or(|at| *at <= now)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for config in ready {
                    collect(&context, &config).await;
                    due.insert(
                        config.id.clone(),
                        Instant::now() + Duration::from_secs(config.interval_sec),
                    );
                }
            }
            Request::All(cycle_id) => {
                let mut success = true;
                for config in &current {
                    success &= collect(&context, config).await;
                }
                if let Some(cycle_id) = cycle_id {
                    let _ = context
                        .publisher
                        .complete_cycle(cycle_id, MANIFEST.id, success)
                        .await;
                }
            }
            Request::One(id) => {
                if let Some(config) = current.iter().find(|config| config.id == id) {
                    collect(&context, config).await;
                    due.insert(
                        id,
                        Instant::now() + Duration::from_secs(config.interval_sec),
                    );
                }
            }
        }
    }
}

async fn collect(context: &ProducerContext, config: &BalanceConfig) -> bool {
    match collect_inner(context, config).await {
        Ok(()) => true,
        Err(error) => {
            let message = error.to_string();
            context
                .state
                .update_source(&config.id, |source| {
                    source.phase = if message.contains("API Key 未配置") {
                        "auth_required"
                    } else {
                        "degraded"
                    }
                    .into();
                    source.last_error = Some(message.clone());
                    source.next_sync_at = Some(unix_now() + config.interval_sec);
                })
                .await;
            context
                .state
                .log("warn", "balance", format!("{}: {message}", config.id))
                .await;
            false
        }
    }
}

async fn collect_inner(context: &ProducerContext, config: &BalanceConfig) -> Result<()> {
    if !context
        .state
        .snapshot()
        .await
        .sources
        .iter()
        .find(|source| source.id == config.id)
        .is_some_and(|source| source.enabled)
    {
        context
            .state
            .update_source(&config.id, |source| {
                source.phase = "disabled".into();
                source.next_sync_at = None;
                source.last_error = None;
            })
            .await;
        return Ok(());
    }
    let secret = secret_get(config.id.clone())
        .await?
        .ok_or_else(|| anyhow!("API Key 未配置"))?;
    context
        .state
        .update_source(&config.id, |source| {
            source.phase = "syncing".into();
            source.last_error = None;
        })
        .await;
    let preview = execute(config, &secret).await?;
    context
        .publisher
        .publish(SemanticResource {
            source_id: config.id.clone(),
            key: config.resource_key(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: config.ttl_sec(),
            persistence: "snapshot",
            payload: json!({
                "source_status": preview.source_status,
                "title": preview.title,
                "items": preview.items,
            }),
        })
        .await?;
    let now = unix_now();
    context
        .state
        .update_source(&config.id, |source| {
            source.phase = "ready".into();
            source.last_sync_at = Some(now);
            source.next_sync_at = Some(now + config.interval_sec);
            source.last_error = None;
            source.details["elapsed_ms"] = json!(preview.elapsed_ms);
            source.details["output_bytes"] = json!(preview.output_bytes);
        })
        .await;
    Ok(())
}

async fn execute(config: &BalanceConfig, secret: &str) -> Result<MetricPreview> {
    let started = Instant::now();
    let client = Client::builder()
        .timeout(Duration::from_millis(config.timeout_ms))
        .redirect(Policy::none())
        .user_agent(concat!("epd-agent/", env!("EPD_AGENT_VERSION")))
        .build()
        .context("HTTP 客户端初始化失败")?;
    let response = client
        .get(config.platform.url())
        .bearer_auth(secret)
        .send()
        .await
        .context("平台余额请求失败")?;
    let status = response.status();
    let bytes = response.bytes().await.context("读取平台响应失败")?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!("平台响应超过 {} KiB", MAX_RESPONSE_BYTES / 1024);
    }
    if !status.is_success() {
        let message = String::from_utf8_lossy(&bytes);
        bail!("平台接口返回 {status}: {}", truncate(&message, 500));
    }
    let input: Value = serde_json::from_slice(&bytes).context("平台响应不是有效 JSON")?;
    project_metrics(
        &config.title,
        &config.platform.items(),
        &input,
        started.elapsed().as_millis(),
        bytes.len(),
    )
}

fn view(config: BalanceConfig, secret_configured: bool) -> BalanceView {
    BalanceView {
        id: config.id,
        enabled: config.enabled,
        title: config.title,
        platform: config.platform,
        interval_sec: config.interval_sec,
        timeout_ms: config.timeout_ms,
        secret_configured,
    }
}

fn source_status(config: &BalanceConfig, secret_configured: bool) -> SourceStatus {
    SourceStatus {
        id: config.id.clone(),
        type_id: MANIFEST.id.into(),
        title: config.title.clone(),
        enabled: config.enabled,
        interval_sec: Some(config.interval_sec),
        phase: if !config.enabled {
            "disabled"
        } else if !secret_configured {
            "auth_required"
        } else {
            "starting"
        }
        .into(),
        resource_keys: vec![config.resource_key()],
        next_sync_at: (config.enabled && secret_configured).then(|| unix_now() + 1),
        last_error: (!secret_configured).then(|| "API Key 未配置".into()),
        details: json!({
            "platform": config.platform,
            "interval_sec": config.interval_sec,
            "item_count": config.platform.items().len(),
        }),
        ..Default::default()
    }
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
    if matches!(id, "codex" | "cc-switch") {
        bail!("数据源 ID {id} 为内置实例保留");
    }
    Ok(())
}

fn config_path() -> Result<PathBuf> {
    let directory = dirs::config_dir()
        .ok_or_else(|| anyhow!("config directory unavailable"))?
        .join("epd-agent");
    std::fs::create_dir_all(&directory)?;
    Ok(directory.join("balance-sources.json"))
}

fn load_sources(path: &Path) -> Result<Vec<BalanceConfig>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file: BalanceFile =
        serde_json::from_slice(&std::fs::read(path).context("读取平台余额配置失败")?)
            .context("平台余额配置 JSON 无效")?;
    if file.sources.len() > MAX_SOURCES {
        bail!("平台余额数据源数量超过上限 {MAX_SOURCES}");
    }
    for (index, source) in file.sources.iter().enumerate() {
        source.validate()?;
        if file.sources[..index]
            .iter()
            .any(|registered| registered.id == source.id)
        {
            bail!("重复的平台余额数据源 ID：{}", source.id);
        }
    }
    Ok(file.sources)
}

fn save_sources(path: &Path, sources: &[BalanceConfig]) -> Result<()> {
    let contents = serde_json::to_vec_pretty(&BalanceFile {
        sources: sources.to_vec(),
    })?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).context("打开平台余额配置失败")?;
    file.write_all(&contents).context("写入平台余额配置失败")?;
    file.sync_all().context("同步平台余额配置失败")
}

fn entry(id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SECRET_SERVICE, id).context("打开系统凭据库失败")
}

async fn secret_get(id: String) -> Result<Option<String>> {
    tokio::task::spawn_blocking(move || match entry(&id)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => bail!("读取系统凭据库失败"),
    })
    .await
    .context("系统凭据任务被取消")?
}

async fn secret_apply(id: String, update: SecretUpdate) -> Result<Option<String>> {
    tokio::task::spawn_blocking(move || match update {
        SecretUpdate::Keep => match entry(&id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => bail!("读取系统凭据库失败"),
        },
        SecretUpdate::Set(secret) => {
            entry(&id)?
                .set_password(&secret)
                .map_err(|_| anyhow!("写入系统凭据库失败"))?;
            Ok(Some(secret))
        }
        SecretUpdate::Clear => {
            match entry(&id)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(_) => bail!("删除系统凭据失败"),
            }
            Ok(None)
        }
    })
    .await
    .context("系统凭据任务被取消")?
}

async fn secret_restore(id: String, secret: Option<String>) -> Result<()> {
    secret_apply(id, secret.map_or(SecretUpdate::Clear, SecretUpdate::Set))
        .await
        .map(|_| ())
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

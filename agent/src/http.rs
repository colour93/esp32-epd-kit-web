use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures::{StreamExt, stream};
use reqwest::{
    Client, Method, Url,
    header::{HeaderMap, HeaderName, HeaderValue},
    redirect::Policy,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{RwLock, mpsc};

use crate::{
    metrics::{MetricItemConfig, MetricPreview, project_metrics, validate_metric_config},
    producer::{ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger},
    publisher::{ResourcePublisher, SemanticResource},
    state::{SharedState, SourceStatus, unix_now},
};

const CONFIG_VERSION: u16 = 1;
const MAX_URL_BYTES: usize = 4096;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_HEADERS: usize = 32;
const MAX_SOURCES: usize = 16;
const MAX_HEADER_NAME_BYTES: usize = 128;
const MAX_HEADER_VALUE_BYTES: usize = 4096;
const MAX_SECRET_BYTES: usize = 8192;
const MIN_INTERVAL_SEC: u64 = 60;
const MAX_INTERVAL_SEC: u64 = 86_400;
const MIN_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const MAX_CONCURRENT_REQUESTS: usize = 4;
const CREDENTIAL_TIMEOUT_SEC: u64 = 5;
const SECRET_SERVICE: &str = "dev.epd-kit.agent.http";

pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "http.jmespath",
    title: "HTTP + JMESPath",
    description: "请求 HTTP JSON 接口并用 JMESPath 投影指标",
    configurable: true,
    multi_instance: true,
    auto_sync: true,
    built_in_source: None,
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum HttpMethod {
    #[default]
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
}

impl HttpMethod {
    fn reqwest(self) -> Method {
        match self {
            Self::Get => Method::GET,
            Self::Post => Method::POST,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccess {
    #[default]
    Public,
    Private,
    Localhost,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HttpHeaderConfig {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpAuthType {
    #[default]
    None,
    Bearer,
    Header,
}

impl HttpAuthType {
    fn requires_secret(self) -> bool {
        self != Self::None
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HttpAuthConfig {
    #[serde(rename = "type")]
    pub kind: HttpAuthType,
    #[serde(default)]
    pub header_name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct HttpAuthInput {
    #[serde(rename = "type")]
    pub kind: HttpAuthType,
    #[serde(default)]
    pub header_name: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub clear_secret: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct HttpAuthView {
    #[serde(rename = "type")]
    pub kind: HttpAuthType,
    pub header_name: String,
    pub secret_configured: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HttpMetricInput {
    pub id: String,
    pub enabled: bool,
    pub title: String,
    pub interval_sec: u64,
    pub timeout_ms: u64,
    #[serde(default)]
    pub method: HttpMethod,
    pub url: String,
    #[serde(default)]
    pub network_access: NetworkAccess,
    #[serde(default)]
    pub headers: Vec<HttpHeaderConfig>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub auth: HttpAuthInput,
    #[serde(default)]
    pub items: Vec<MetricItemConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HttpMetricConfig {
    id: String,
    enabled: bool,
    title: String,
    interval_sec: u64,
    timeout_ms: u64,
    #[serde(default)]
    method: HttpMethod,
    url: String,
    #[serde(default)]
    network_access: NetworkAccess,
    #[serde(default)]
    headers: Vec<HttpHeaderConfig>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    auth: HttpAuthConfig,
    #[serde(default)]
    items: Vec<MetricItemConfig>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HttpMetricView {
    pub id: String,
    pub enabled: bool,
    pub title: String,
    pub interval_sec: u64,
    pub timeout_ms: u64,
    pub method: HttpMethod,
    pub url: String,
    pub network_access: NetworkAccess,
    pub headers: Vec<HttpHeaderConfig>,
    pub body: String,
    pub auth: HttpAuthView,
    pub items: Vec<MetricItemConfig>,
}

impl HttpMetricConfig {
    fn from_input(mut input: HttpMetricInput) -> Result<(Self, SecretUpdate)> {
        if input.auth.clear_secret && input.auth.secret.is_some() {
            bail!("secret 与 clear_secret 不能同时提交");
        };
        if let Some(secret) = input.auth.secret.as_ref() {
            if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
                bail!("HTTP 凭据长度必须为 1-{MAX_SECRET_BYTES} bytes");
            }
        }
        let secret_update = if input.auth.kind == HttpAuthType::None || input.auth.clear_secret {
            SecretUpdate::Clear
        } else if let Some(secret) = input.auth.secret.take() {
            SecretUpdate::Set(secret)
        } else {
            SecretUpdate::Keep
        };
        let config = Self {
            id: input.id,
            enabled: input.enabled,
            title: input.title,
            interval_sec: input.interval_sec,
            timeout_ms: input.timeout_ms,
            method: input.method,
            url: input.url,
            network_access: input.network_access,
            headers: input.headers,
            body: input.body,
            auth: HttpAuthConfig {
                kind: input.auth.kind,
                header_name: input.auth.header_name,
            },
            items: input.items,
        };
        Ok((config, secret_update))
    }

    fn resource_key(&self) -> String {
        format!("http/{}", self.id)
    }

    fn configured(&self) -> bool {
        !self.title.trim().is_empty()
            && !self.url.trim().is_empty()
            && !self.items.is_empty()
            && self.items.iter().all(|item| {
                !item.label.trim().is_empty() && !item.data_expression.trim().is_empty()
            })
    }

    fn validate(&self, require_complete: bool) -> Result<()> {
        validate_source_id(&self.id)?;
        if !(MIN_INTERVAL_SEC..=MAX_INTERVAL_SEC).contains(&self.interval_sec) {
            bail!("interval_sec 必须在 {MIN_INTERVAL_SEC}-{MAX_INTERVAL_SEC} 之间");
        }
        if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&self.timeout_ms) {
            bail!("timeout_ms 必须在 {MIN_TIMEOUT_MS}-{MAX_TIMEOUT_MS} 之间");
        }
        if self.url.len() > MAX_URL_BYTES {
            bail!("HTTP URL 不能超过 {MAX_URL_BYTES} bytes");
        }
        if self.body.len() > MAX_BODY_BYTES {
            bail!("HTTP body 不能超过 {} KiB", MAX_BODY_BYTES / 1024);
        }
        validate_metric_config(&self.title, &self.items, false)?;
        validate_headers(&self.headers, &self.auth)?;
        if self.auth.kind == HttpAuthType::Header {
            validate_auth_header_name(&self.auth.header_name)?;
        } else if !self.auth.header_name.trim().is_empty() {
            bail!("只有 header 认证可以设置 header_name");
        }
        if self.method == HttpMethod::Get && !self.body.trim().is_empty() {
            bail!("GET 请求不能包含 body");
        }
        if self.method == HttpMethod::Post && !self.body.trim().is_empty() {
            let body: Value =
                serde_json::from_str(&self.body).context("POST body 不是有效 JSON")?;
            if contains_sensitive_json_key(&body) {
                bail!("POST body 不得包含凭据字段，请使用 auth 配置");
            }
        }
        if require_complete || self.enabled || self.configured() {
            validate_url(&self.url, self.network_access)?;
            validate_metric_config(&self.title, &self.items, true)?;
        }
        Ok(())
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HttpMetricSourcesFile {
    #[serde(default = "config_version")]
    version: u16,
    sources: Vec<HttpMetricConfig>,
}

fn config_version() -> u16 {
    CONFIG_VERSION
}

trait CredentialStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>>;
    fn set(&self, id: &str, secret: &str) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

struct SystemCredentialStore;

impl SystemCredentialStore {
    fn entry(id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SECRET_SERVICE, id).context("打开系统凭据库失败")
    }
}

impl CredentialStore for SystemCredentialStore {
    fn get(&self, id: &str) -> Result<Option<String>> {
        match Self::entry(id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => bail!("读取系统凭据库失败"),
        }
    }

    fn set(&self, id: &str, secret: &str) -> Result<()> {
        Self::entry(id)?
            .set_password(secret)
            .map_err(|_| anyhow!("写入系统凭据库失败"))
    }

    fn delete(&self, id: &str) -> Result<()> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => bail!("删除系统凭据失败"),
        }
    }
}

async fn secret_get(store: Arc<dyn CredentialStore>, id: String) -> Result<Option<String>> {
    tokio::time::timeout(
        Duration::from_secs(CREDENTIAL_TIMEOUT_SEC),
        tokio::task::spawn_blocking(move || store.get(&id)),
    )
    .await
    .map_err(|_| anyhow!("读取系统凭据库超时"))?
    .context("系统凭据任务被取消")?
}

async fn secret_apply(
    store: Arc<dyn CredentialStore>,
    id: String,
    update: SecretUpdate,
) -> Result<Option<String>> {
    tokio::task::spawn_blocking(move || match update {
        SecretUpdate::Keep => store.get(&id),
        SecretUpdate::Set(secret) => {
            store.set(&id, &secret)?;
            Ok(Some(secret))
        }
        SecretUpdate::Clear => {
            store.delete(&id)?;
            Ok(None)
        }
    })
    .await
    .context("系统凭据任务被取消")?
}

async fn configured_secret(
    store: Arc<dyn CredentialStore>,
    config: &HttpMetricConfig,
) -> Result<Option<String>> {
    if !config.auth.kind.requires_secret() {
        return Ok(None);
    }
    secret_get(store, config.id.clone()).await
}

fn credential_scope_changed(previous: &HttpMetricConfig, next: &HttpMetricConfig) -> Result<bool> {
    if previous.auth.kind != next.auth.kind
        || !previous
            .auth
            .header_name
            .eq_ignore_ascii_case(&next.auth.header_name)
    {
        return Ok(true);
    }
    let previous_url = Url::parse(&previous.url).context("已有 HTTP URL 无效")?;
    let next_url = Url::parse(&next.url).context("HTTP URL 无效")?;
    Ok(previous_url.scheme() != next_url.scheme()
        || previous_url.host_str() != next_url.host_str()
        || previous_url.port_or_known_default() != next_url.port_or_known_default())
}

async fn secret_restore(
    store: Arc<dyn CredentialStore>,
    id: String,
    secret: Option<String>,
) -> Result<()> {
    let update = secret.map_or(SecretUpdate::Clear, SecretUpdate::Set);
    secret_apply(store, id, update).await.map(|_| ())
}

#[derive(Clone)]
pub struct HttpMetricControl {
    sources: Arc<RwLock<Vec<HttpMetricConfig>>>,
    config_path: Arc<PathBuf>,
    trigger: mpsc::Sender<ProducerTrigger>,
    source_trigger: mpsc::Sender<String>,
    publisher: ResourcePublisher,
    credentials: Arc<dyn CredentialStore>,
}

impl HttpMetricControl {
    pub fn spawn(context: ProducerContext) -> Result<Self> {
        Self::spawn_with_credentials(context, Arc::new(SystemCredentialStore))
    }

    fn spawn_with_credentials(
        context: ProducerContext,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self> {
        let config_path = config_path()?;
        let sources = Arc::new(RwLock::new(load_sources(&config_path)?));
        let publisher = context.publisher.clone();
        let (trigger, receiver) = mpsc::channel(8);
        let (source_trigger, source_receiver) = mpsc::channel(16);
        tokio::spawn(run(
            context,
            sources.clone(),
            credentials.clone(),
            receiver,
            source_receiver,
        ));
        Ok(Self {
            sources,
            config_path: Arc::new(config_path),
            trigger,
            source_trigger,
            publisher,
            credentials,
        })
    }

    pub fn control(&self) -> ProducerControl {
        ProducerControl::with_instance_refresh(
            &MANIFEST,
            self.trigger.clone(),
            self.source_trigger.clone(),
        )
    }

    pub async fn sources(&self) -> Result<Vec<HttpMetricView>> {
        let sources = self.sources.read().await.clone();
        let mut views = Vec::with_capacity(sources.len());
        for config in sources {
            let configured = configured_secret(self.credentials.clone(), &config)
                .await?
                .is_some();
            views.push(source_view(config, configured));
        }
        Ok(views)
    }

    pub async fn create_source(
        &self,
        state: &SharedState,
        input: HttpMetricInput,
    ) -> Result<HttpMetricView> {
        let (config, secret_update) = HttpMetricConfig::from_input(input)?;
        config.validate(false)?;
        if !state
            .register_source_if_absent(source_status(&config, false))
            .await
        {
            bail!("数据源 ID 已存在：{}", config.id);
        }
        let result: Result<Option<String>> = async {
            let mut sources = self.sources.write().await;
            if sources.iter().any(|source| source.id == config.id) {
                bail!("数据源 ID 已存在：{}", config.id);
            }
            if sources.len() >= MAX_SOURCES {
                bail!("最多配置 {MAX_SOURCES} 个 HTTP 数据源");
            }
            let (old_secret, secret) = if config.auth.kind.requires_secret() {
                let old_secret = secret_get(self.credentials.clone(), config.id.clone()).await?;
                let create_update = match secret_update {
                    SecretUpdate::Keep => SecretUpdate::Clear,
                    update => update,
                };
                let secret =
                    secret_apply(self.credentials.clone(), config.id.clone(), create_update)
                        .await?;
                (old_secret, secret)
            } else {
                (None, None)
            };
            let mut next = sources.clone();
            next.push(config.clone());
            if let Err(error) = save_sources(&self.config_path, &next) {
                if config.auth.kind.requires_secret() {
                    secret_restore(self.credentials.clone(), config.id.clone(), old_secret)
                        .await
                        .context("HTTP 配置写入失败且凭据回滚失败")?;
                }
                return Err(error);
            }
            *sources = next;
            Ok(secret)
        }
        .await;
        let secret = match result {
            Ok(secret) => secret,
            Err(error) => {
                state.remove_source(&config.id).await;
                return Err(error);
            }
        };
        state
            .register_source(source_status(&config, secret.is_some()))
            .await;
        self.refresh_source(&config.id).await?;
        Ok(source_view(config, secret.is_some()))
    }

    pub async fn update_source(
        &self,
        state: &SharedState,
        id: &str,
        input: HttpMetricInput,
    ) -> Result<HttpMetricView> {
        let (config, secret_update) = HttpMetricConfig::from_input(input)?;
        if config.id != id {
            bail!("数据源 ID 创建后不可修改");
        }
        config.validate(false)?;
        let secret = {
            let mut sources = self.sources.write().await;
            let previous = sources
                .iter()
                .find(|source| source.id == id)
                .cloned()
                .ok_or_else(|| anyhow!("未知 HTTP 数据源：{id}"))?;
            let touches_credentials =
                previous.auth.kind.requires_secret() || config.auth.kind.requires_secret();
            let old_secret = if touches_credentials {
                secret_get(self.credentials.clone(), config.id.clone()).await?
            } else {
                None
            };
            if old_secret.is_some()
                && secret_update == SecretUpdate::Keep
                && credential_scope_changed(&previous, &config)?
            {
                bail!("HTTP 凭据目标已变化，请重新输入或清除密钥");
            }
            let secret = if touches_credentials {
                secret_apply(self.credentials.clone(), config.id.clone(), secret_update).await?
            } else {
                None
            };
            let mut next = sources.clone();
            let current = next
                .iter_mut()
                .find(|source| source.id == id)
                .ok_or_else(|| anyhow!("未知 HTTP 数据源：{id}"))?;
            *current = config.clone();
            if let Err(error) = save_sources(&self.config_path, &next) {
                if touches_credentials {
                    secret_restore(self.credentials.clone(), config.id.clone(), old_secret)
                        .await
                        .context("HTTP 配置写入失败且凭据回滚失败")?;
                }
                return Err(error);
            }
            *sources = next;
            secret
        };
        state
            .register_source(source_status(&config, secret.is_some()))
            .await;
        self.refresh_source(id).await?;
        Ok(source_view(config, secret.is_some()))
    }

    pub async fn delete_source(&self, state: &SharedState, id: &str) -> Result<()> {
        let key = {
            let mut sources = self.sources.write().await;
            let mut next = sources.clone();
            let index = next
                .iter()
                .position(|source| source.id == id)
                .ok_or_else(|| anyhow!("未知 HTTP 数据源：{id}"))?;
            let key = next[index].resource_key();
            let requires_secret = next[index].auth.kind.requires_secret();
            next.remove(index);
            let old_secret = if requires_secret {
                let old_secret = secret_get(self.credentials.clone(), id.to_owned()).await?;
                secret_apply(self.credentials.clone(), id.to_owned(), SecretUpdate::Clear).await?;
                old_secret
            } else {
                None
            };
            if let Err(error) = save_sources(&self.config_path, &next) {
                if requires_secret {
                    secret_restore(self.credentials.clone(), id.to_owned(), old_secret)
                        .await
                        .context("HTTP 配置写入失败且凭据回滚失败")?;
                }
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
            bail!("未知 HTTP 数据源：{id}");
        }
        self.source_trigger
            .send(id.to_owned())
            .await
            .map_err(|_| anyhow!("HTTP 数据源管理器已停止"))
    }

    pub async fn test_config(&self, input: HttpMetricInput) -> Result<MetricPreview> {
        let (config, secret_update) = HttpMetricConfig::from_input(input)?;
        config.validate(true)?;
        let secret = match secret_update {
            SecretUpdate::Set(secret) => Some(secret),
            SecretUpdate::Clear => None,
            SecretUpdate::Keep => configured_secret(self.credentials.clone(), &config).await?,
        };
        if config.auth.kind.requires_secret() && secret.is_none() {
            bail!("HTTP 凭据未配置");
        }
        execute_and_project(&config, secret.as_deref()).await
    }
}

fn source_view(config: HttpMetricConfig, secret_configured: bool) -> HttpMetricView {
    HttpMetricView {
        id: config.id,
        enabled: config.enabled,
        title: config.title,
        interval_sec: config.interval_sec,
        timeout_ms: config.timeout_ms,
        method: config.method,
        url: config.url,
        network_access: config.network_access,
        headers: config.headers,
        body: config.body,
        auth: HttpAuthView {
            kind: config.auth.kind,
            header_name: config.auth.header_name,
            secret_configured,
        },
        items: config.items,
    }
}

async fn run(
    context: ProducerContext,
    sources: Arc<RwLock<Vec<HttpMetricConfig>>>,
    credentials: Arc<dyn CredentialStore>,
    mut triggers: mpsc::Receiver<ProducerTrigger>,
    mut source_triggers: mpsc::Receiver<String>,
) {
    for source in sources.read().await.iter() {
        let configured = configured_secret(credentials.clone(), source)
            .await
            .ok()
            .flatten()
            .is_some();
        context
            .state
            .register_source(source_status(source, configured))
            .await;
    }
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        enum Request {
            Due,
            All(Option<u64>),
            One(String),
        }
        let request = tokio::select! {
            _ = poll.tick() => Request::Due,
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
            Request::Due => {
                let now = unix_now();
                let due_ids = context
                    .state
                    .snapshot()
                    .await
                    .sources
                    .into_iter()
                    .filter(|source| {
                        source.type_id == MANIFEST.id
                            && source.enabled
                            && source.next_sync_at.is_some_and(|next| next <= now)
                    })
                    .map(|source| source.id)
                    .collect::<HashSet<_>>();
                if due_ids.is_empty() {
                    continue;
                }
                let current = sources
                    .read()
                    .await
                    .iter()
                    .filter(|source| due_ids.contains(&source.id))
                    .cloned()
                    .collect::<Vec<_>>();
                collect_all(&context, credentials.clone(), current).await;
            }
            Request::All(cycle_id) => {
                let current = sources.read().await.clone();
                let success = collect_all(&context, credentials.clone(), current).await;
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
                    collect_with_status(&context, credentials.clone(), config).await;
                }
            }
        }
    }
}

async fn collect_all(
    context: &ProducerContext,
    credentials: Arc<dyn CredentialStore>,
    sources: Vec<HttpMetricConfig>,
) -> bool {
    stream::iter(sources)
        .map(|config| {
            let context = context.clone();
            let credentials = credentials.clone();
            async move { collect_with_status(&context, credentials, config).await }
        })
        .buffer_unordered(MAX_CONCURRENT_REQUESTS)
        .fold(true, |success, result| async move { success && result })
        .await
}

async fn collect_with_status(
    context: &ProducerContext,
    credentials: Arc<dyn CredentialStore>,
    config: HttpMetricConfig,
) -> bool {
    match collect_and_publish(context, credentials, &config).await {
        Ok(()) => true,
        Err(error) => {
            let message = error.to_string();
            let phase = if message == "HTTP 凭据未配置" {
                "auth_required"
            } else {
                "degraded"
            };
            context
                .state
                .update_source(&config.id, |source| {
                    source.phase = phase.into();
                    source.last_error = Some(message.clone());
                    source.next_sync_at = Some(unix_now() + config.interval_sec);
                })
                .await;
            context
                .state
                .log("warn", "http", format!("{}: {message}", config.id))
                .await;
            false
        }
    }
}

async fn collect_and_publish(
    context: &ProducerContext,
    credentials: Arc<dyn CredentialStore>,
    config: &HttpMetricConfig,
) -> Result<()> {
    config.validate(false)?;
    if !config.configured() {
        publish_status(&context.publisher, config, "unconfigured").await?;
        update_non_running(&context.state, "unconfigured", config, false).await;
        return Ok(());
    }
    if !config.enabled {
        publish_status(&context.publisher, config, "disabled").await?;
        update_non_running(&context.state, "disabled", config, false).await;
        return Ok(());
    }
    let secret = configured_secret(credentials, config).await?;
    if config.auth.kind.requires_secret() && secret.is_none() {
        bail!("HTTP 凭据未配置");
    }
    context
        .state
        .update_source(&config.id, |source| {
            source.phase = "syncing".into();
            source.last_error = None;
        })
        .await;
    let preview = execute_and_project(config, secret.as_deref()).await?;
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
    context
        .state
        .log(
            "info",
            "http",
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

async fn update_non_running(
    state: &SharedState,
    phase: &str,
    config: &HttpMetricConfig,
    secret_configured: bool,
) {
    state
        .register_source(source_status(config, secret_configured))
        .await;
    state
        .update_source(&config.id, |source| {
            source.phase = phase.into();
            source.next_sync_at = None;
            source.last_error = None;
        })
        .await;
}

async fn publish_status(
    publisher: &ResourcePublisher,
    config: &HttpMetricConfig,
    source_status: &'static str,
) -> Result<()> {
    publisher
        .publish(SemanticResource {
            source_id: config.id.clone(),
            key: config.resource_key(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: config.ttl_sec(),
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

fn source_status(config: &HttpMetricConfig, secret_configured: bool) -> SourceStatus {
    let configured = config.configured();
    let auth_ready = !config.auth.kind.requires_secret() || secret_configured;
    SourceStatus {
        id: config.id.clone(),
        type_id: MANIFEST.id.into(),
        title: if config.title.trim().is_empty() {
            config.id.clone()
        } else {
            config.title.trim().to_owned()
        },
        enabled: config.enabled,
        phase: if !configured {
            "unconfigured".into()
        } else if !config.enabled {
            "disabled".into()
        } else if !auth_ready {
            "auth_required".into()
        } else {
            "starting".into()
        },
        resource_keys: vec![config.resource_key()],
        next_sync_at: (configured && config.enabled && auth_ready).then(|| unix_now() + 1),
        last_error: (!auth_ready).then(|| "HTTP 凭据未配置".into()),
        details: json!({
            "item_count": config.items.len(),
            "interval_sec": config.interval_sec,
        }),
        ..Default::default()
    }
}

async fn execute_and_project(
    config: &HttpMetricConfig,
    secret: Option<&str>,
) -> Result<MetricPreview> {
    tokio::time::timeout(
        Duration::from_millis(config.timeout_ms),
        execute_and_project_inner(config, secret),
    )
    .await
    .map_err(|_| anyhow!("HTTP 请求超时"))?
}

async fn execute_and_project_inner(
    config: &HttpMetricConfig,
    secret: Option<&str>,
) -> Result<MetricPreview> {
    let started = Instant::now();
    let url = validate_url(&config.url, config.network_access)?;
    let pinned = resolve_and_validate(
        &url,
        config.network_access,
        Duration::from_millis(config.timeout_ms),
    )
    .await?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("HTTP URL 缺少 host"))?;
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_millis(config.timeout_ms))
        .connect_timeout(Duration::from_millis(config.timeout_ms))
        .resolve(host, pinned)
        .user_agent(concat!("epd-agent/", env!("EPD_AGENT_VERSION")))
        .build()
        .map_err(|_| anyhow!("HTTP 客户端初始化失败"))?;
    let mut request = client
        .request(config.method.reqwest(), url)
        .headers(build_headers(config, secret)?);
    if config.method == HttpMethod::Post && !config.body.trim().is_empty() {
        let body: Value = serde_json::from_str(&config.body).context("POST body 不是有效 JSON")?;
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            anyhow!("HTTP 请求超时")
        } else {
            anyhow!("HTTP 请求失败")
        }
    })?;
    if !response.status().is_success() {
        bail!("HTTP 响应状态 {}", response.status().as_u16());
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(0)
            .min(MAX_RESPONSE_BYTES as u64) as usize,
    );
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| anyhow!("HTTP 响应读取失败"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            bail!("HTTP 响应超过 {} KiB", MAX_RESPONSE_BYTES / 1024);
        }
        bytes.extend_from_slice(&chunk);
    }
    let input: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("HTTP 响应不是有效 JSON"))?;
    project_metrics(
        &config.title,
        &config.items,
        &input,
        started.elapsed().as_millis(),
        bytes.len(),
    )
}

fn build_headers(config: &HttpMetricConfig, secret: Option<&str>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for header in &config.headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| anyhow!("HTTP header name 无效"))?;
        let value =
            HeaderValue::from_str(&header.value).map_err(|_| anyhow!("HTTP header value 无效"))?;
        headers.append(name, value);
    }
    match config.auth.kind {
        HttpAuthType::None => {}
        HttpAuthType::Bearer => {
            let secret = secret.ok_or_else(|| anyhow!("HTTP 凭据未配置"))?;
            let value = HeaderValue::from_str(&format!("Bearer {secret}"))
                .map_err(|_| anyhow!("HTTP 凭据包含无效字符"))?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        HttpAuthType::Header => {
            let secret = secret.ok_or_else(|| anyhow!("HTTP 凭据未配置"))?;
            let name = HeaderName::from_bytes(config.auth.header_name.as_bytes())
                .map_err(|_| anyhow!("认证 header_name 无效"))?;
            let value =
                HeaderValue::from_str(secret).map_err(|_| anyhow!("HTTP 凭据包含无效字符"))?;
            headers.insert(name, value);
        }
    }
    Ok(headers)
}

fn validate_headers(headers: &[HttpHeaderConfig], auth: &HttpAuthConfig) -> Result<()> {
    if headers.len() > MAX_HEADERS {
        bail!("最多配置 {MAX_HEADERS} 个 HTTP headers");
    }
    let mut names = HashSet::new();
    for header in headers {
        if header.name.len() > MAX_HEADER_NAME_BYTES || header.value.len() > MAX_HEADER_VALUE_BYTES
        {
            bail!("HTTP header 超过长度限制");
        }
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| anyhow!("HTTP header name 无效"))?;
        HeaderValue::from_str(&header.value).map_err(|_| anyhow!("HTTP header value 无效"))?;
        let lower = name.as_str().to_ascii_lowercase();
        if is_forbidden_header(&lower) {
            bail!("HTTP header {lower} 不允许直接配置");
        }
        if auth.kind == HttpAuthType::Header
            && lower == auth.header_name.trim().to_ascii_lowercase()
        {
            bail!("认证 header 不能在 headers 中重复配置");
        }
        if !names.insert(lower) {
            bail!("HTTP header name 不能重复");
        }
    }
    Ok(())
}

fn validate_auth_header_name(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_HEADER_NAME_BYTES {
        bail!("header 认证必须设置有效的 header_name");
    }
    let name =
        HeaderName::from_bytes(value.as_bytes()).map_err(|_| anyhow!("认证 header_name 无效"))?;
    let lower = name.as_str().to_ascii_lowercase();
    if is_hop_by_hop_header(&lower) {
        bail!("认证 header_name 不允许使用 {lower}");
    }
    Ok(())
}

fn is_forbidden_header(name: &str) -> bool {
    is_hop_by_hop_header(name)
        || is_sensitive_name(name)
        || matches!(
            name,
            "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key"
        )
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

fn validate_url(value: &str, network_access: NetworkAccess) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| anyhow!("HTTP URL 无效"))?;
    if url.username() != "" || url.password().is_some() {
        bail!("HTTP URL 不得包含用户名或密码");
    }
    if url.fragment().is_some() {
        bail!("HTTP URL 不得包含 fragment");
    }
    match url.scheme() {
        "https" => {}
        "http" if network_access != NetworkAccess::Public => {}
        "http" => bail!("public 网络请求必须使用 HTTPS"),
        _ => bail!("HTTP URL 只支持 http 或 https"),
    }
    if url.host_str().is_none() {
        bail!("HTTP URL 缺少 host");
    }
    if url
        .query_pairs()
        .any(|(name, _)| is_sensitive_name(&name.to_ascii_lowercase()))
    {
        bail!("HTTP URL query 不得包含凭据，请使用 auth 配置");
    }
    Ok(url)
}

async fn resolve_and_validate(
    url: &Url,
    access: NetworkAccess,
    timeout: Duration,
) -> Result<SocketAddr> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("HTTP URL 缺少 host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("HTTP URL 缺少端口"))?;
    let addresses = if let Ok(ip) = IpAddr::from_str(host.trim_matches(['[', ']'])) {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::time::timeout(timeout, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| anyhow!("HTTP host DNS 解析超时"))?
            .map_err(|_| anyhow!("HTTP host DNS 解析失败"))?
            .collect::<Vec<_>>()
    };
    if addresses.is_empty() {
        bail!("HTTP host DNS 未返回地址");
    }
    if addresses
        .iter()
        .any(|address| !network_access_allows(access, address.ip()))
    {
        bail!("HTTP host 地址不符合 network_access");
    }
    Ok(addresses[0])
}

fn network_access_allows(access: NetworkAccess, ip: IpAddr) -> bool {
    if let IpAddr::V6(ipv6) = ip {
        if let Some(ipv4) = ipv6.to_ipv4_mapped() {
            return network_access_allows(access, IpAddr::V4(ipv4));
        }
    }
    match access {
        NetworkAccess::Public => is_public_ip(ip),
        NetworkAccess::Private => is_private_ip(ip),
        NetworkAccess::Localhost => ip.is_loopback(),
    }
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || in_ipv4_prefix(ip, [100, 64, 0, 0], 10),
        IpAddr::V6(ip) => (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || in_ipv4_prefix(ip, [0, 0, 0, 0], 8)
                || in_ipv4_prefix(ip, [100, 64, 0, 0], 10)
                || in_ipv4_prefix(ip, [192, 0, 0, 0], 24)
                || in_ipv4_prefix(ip, [198, 18, 0, 0], 15)
                || in_ipv4_prefix(ip, [240, 0, 0, 0], 4))
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            (first & 0xe000) == 0x2000
                && !ip.is_multicast()
                && !ip.is_unspecified()
                && !ip.is_loopback()
                && !is_ipv6_documentation(ip)
        }
    }
}

fn in_ipv4_prefix(ip: Ipv4Addr, base: [u8; 4], prefix: u32) -> bool {
    let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
    u32::from(ip) & mask == u32::from(Ipv4Addr::from(base)) & mask
}

fn is_ipv6_documentation(ip: Ipv6Addr) -> bool {
    ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8
}

fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let words = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.iter().any(|word| {
        matches!(
            *word,
            "authorization"
                | "authentication"
                | "auth"
                | "secret"
                | "password"
                | "passwd"
                | "credential"
                | "credentials"
                | "cookie"
        )
    }) {
        return true;
    }
    let has = |value: &str| words.contains(&value);
    if has("token")
        && (words.len() == 1
            || ["access", "refresh", "auth", "bearer", "session", "api"]
                .iter()
                .any(|prefix| has(prefix)))
    {
        return true;
    }
    if has("key")
        && ["access", "api", "session", "private", "client"]
            .iter()
            .any(|prefix| has(prefix))
    {
        return true;
    }
    let compact = lower
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    [
        "authorization",
        "authentication",
        "authtoken",
        "accesstoken",
        "refreshtoken",
        "bearertoken",
        "sessiontoken",
        "sessionkey",
        "sessionid",
        "accesskey",
        "apikey",
        "privatekey",
        "clientsecret",
        "password",
        "passwd",
        "credential",
        "credentials",
        "cookie",
    ]
    .iter()
    .any(|suffix| compact.ends_with(suffix))
}

fn contains_sensitive_json_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(name, value)| {
            is_sensitive_name(&name.to_ascii_lowercase()) || contains_sensitive_json_key(value)
        }),
        Value::Array(items) => items.iter().any(contains_sensitive_json_key),
        _ => false,
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory.join("http-sources.json"))
}

fn load_sources(path: &Path) -> Result<Vec<HttpMetricConfig>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read(path).context("读取 HTTP 数据源实例失败")?;
    let file: HttpMetricSourcesFile =
        serde_json::from_slice(&contents).context("HTTP 数据源配置 JSON 无效")?;
    if file.version != CONFIG_VERSION {
        bail!("不支持的 HTTP 数据源配置版本：{}", file.version);
    }
    if file.sources.len() > MAX_SOURCES {
        bail!("HTTP 数据源数量超过上限 {MAX_SOURCES}");
    }
    for (index, source) in file.sources.iter().enumerate() {
        source
            .validate(false)
            .with_context(|| format!("HTTP 数据源实例 {} 无效", index + 1))?;
        if file.sources[..index]
            .iter()
            .any(|registered| registered.id == source.id)
        {
            bail!("重复的 HTTP 数据源 ID：{}", source.id);
        }
    }
    Ok(file.sources)
}

fn save_sources(path: &Path, sources: &[HttpMetricConfig]) -> Result<()> {
    let contents = serde_json::to_vec_pretty(&HttpMetricSourcesFile {
        version: CONFIG_VERSION,
        sources: sources.to_vec(),
    })?;
    let temporary = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .context("创建 HTTP 数据源临时配置失败")?;
        file.write_all(&contents)
            .context("写入 HTTP 数据源临时配置失败")?;
        file.sync_all().context("同步 HTTP 数据源临时配置失败")?;
        drop(file);
        replace_config_file(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_config_file(temporary: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(temporary, destination).context("原子替换 HTTP 数据源配置失败")?;
    if let Some(directory) = destination.parent() {
        std::fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .context("同步 HTTP 数据源配置目录失败")?;
    }
    Ok(())
}

#[cfg(windows)]
fn replace_config_file(temporary: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            REPLACEFILE_WRITE_THROUGH, ReplaceFileW,
        },
        core::PCWSTR,
    };

    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let temporary = PCWSTR(temporary.as_ptr());
    let destination_wide = PCWSTR(destination_wide.as_ptr());
    unsafe {
        if destination.exists() {
            ReplaceFileW(
                destination_wide,
                temporary,
                PCWSTR::null(),
                REPLACEFILE_WRITE_THROUGH,
                None,
                None,
            )
            .context("原子替换 HTTP 数据源配置失败")
        } else {
            MoveFileExW(
                temporary,
                destination_wide,
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
            .context("原子创建 HTTP 数据源配置失败")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, net::IpAddr, path::PathBuf, str::FromStr};

    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use crate::metrics::{MetricFormat, MetricItemConfig};

    use super::{
        HttpAuthConfig, HttpAuthInput, HttpAuthType, HttpHeaderConfig, HttpMethod,
        HttpMetricConfig, HttpMetricInput, MAX_RESPONSE_BYTES, NetworkAccess, SecretUpdate,
        credential_scope_changed, execute_and_project, is_sensitive_name, load_sources,
        network_access_allows, save_sources, source_view,
    };

    struct ConfigDirectory {
        path: PathBuf,
    }

    impl ConfigDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("epd-agent-http-config-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for ConfigDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn custom_config(url: String) -> HttpMetricConfig {
        HttpMetricConfig {
            id: "test-http".into(),
            enabled: true,
            title: "HTTP".into(),
            interval_sec: 300,
            timeout_ms: 5_000,
            method: HttpMethod::Get,
            url,
            network_access: NetworkAccess::Localhost,
            headers: vec![HttpHeaderConfig {
                name: "X-Test".into(),
                value: "present".into(),
            }],
            body: String::new(),
            auth: HttpAuthConfig {
                kind: HttpAuthType::Bearer,
                header_name: String::new(),
            },
            items: vec![MetricItemConfig {
                label: "余额".into(),
                data_expression: "data.balance".into(),
                description_expression: "data.currency".into(),
                progress_expression: String::new(),
                format: MetricFormat::Text,
            }],
        }
    }

    async fn mock_response(response: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let read = socket.read(&mut request).await.unwrap();
            socket.write_all(&response).await.unwrap();
            String::from_utf8_lossy(&request[..read]).into_owned()
        });
        (format!("http://{address}/metrics"), task)
    }

    #[test]
    fn omitted_secret_is_kept_for_updates_and_never_serialized() {
        let input = HttpMetricInput {
            id: "secret-main".into(),
            enabled: true,
            title: "Secret HTTP".into(),
            interval_sec: 300,
            timeout_ms: 10_000,
            method: HttpMethod::Post,
            url: "http://127.0.0.1/untrusted".into(),
            network_access: NetworkAccess::Localhost,
            headers: vec![HttpHeaderConfig {
                name: "X-Untrusted".into(),
                value: "value".into(),
            }],
            body: "{}".into(),
            auth: HttpAuthInput {
                kind: HttpAuthType::Bearer,
                header_name: String::new(),
                secret: None,
                clear_secret: false,
            },
            items: Vec::new(),
        };
        let (config, update) = HttpMetricConfig::from_input(input).unwrap();
        assert_eq!(update, SecretUpdate::Keep);
        let encoded = serde_json::to_value(source_view(config, true)).unwrap();
        let auth = encoded.get("auth").unwrap();
        assert_eq!(auth.get("secret_configured"), Some(&json!(true)));
        assert!(auth.get("secret").is_none());
        assert!(auth.get("clear_secret").is_none());
    }

    #[test]
    fn public_http_and_embedded_credentials_are_rejected() {
        let mut config = custom_config("http://8.8.8.8/metrics".into());
        config.network_access = NetworkAccess::Public;
        assert!(
            config
                .validate(true)
                .unwrap_err()
                .to_string()
                .contains("HTTPS")
        );

        config.url = "https://example.com/metrics?access_token=value".into();
        assert!(
            config
                .validate(true)
                .unwrap_err()
                .to_string()
                .contains("query")
        );
    }

    #[test]
    fn compound_credential_names_are_rejected_without_blocking_usage_fields() {
        for name in [
            "X-Auth-Token",
            "X-Access-Key",
            "authToken",
            "session_key",
            "clientSecret",
        ] {
            assert!(is_sensitive_name(name), "expected {name} to be sensitive");
        }
        for name in ["input_tokens", "max_tokens", "token_count", "X-Request-ID"] {
            assert!(
                !is_sensitive_name(name),
                "expected {name} to remain configurable"
            );
        }
    }

    #[test]
    fn changing_credential_host_requires_a_new_secret() {
        let previous = custom_config("https://api.example.com/metrics".into());
        let mut next = previous.clone();
        next.url = "https://metrics.example.net/usage".into();
        assert!(credential_scope_changed(&previous, &next).unwrap());

        next.url = "https://api.example.com/v1/other".into();
        assert!(!credential_scope_changed(&previous, &next).unwrap());
    }

    #[test]
    fn source_config_is_replaced_as_complete_json() {
        let directory = ConfigDirectory::new();
        let path = directory.path.join("http-sources.json");
        let mut config = custom_config("http://127.0.0.1/metrics".into());
        save_sources(&path, &[config.clone()]).unwrap();
        assert_eq!(load_sources(&path).unwrap()[0].title, "HTTP");

        config.title = "Updated".into();
        save_sources(&path, &[config]).unwrap();
        assert_eq!(load_sources(&path).unwrap()[0].title, "Updated");
        assert_eq!(fs::read_dir(&directory.path).unwrap().count(), 1);
    }

    #[test]
    fn network_access_is_scoped_and_blocks_link_local() {
        assert!(network_access_allows(
            NetworkAccess::Public,
            IpAddr::from_str("8.8.8.8").unwrap()
        ));
        assert!(!network_access_allows(
            NetworkAccess::Public,
            IpAddr::from_str("10.0.0.1").unwrap()
        ));
        assert!(network_access_allows(
            NetworkAccess::Private,
            IpAddr::from_str("10.0.0.1").unwrap()
        ));
        assert!(!network_access_allows(
            NetworkAccess::Private,
            IpAddr::from_str("169.254.169.254").unwrap()
        ));
        assert!(network_access_allows(
            NetworkAccess::Localhost,
            IpAddr::from_str("127.0.0.1").unwrap()
        ));
    }

    #[tokio::test]
    async fn local_http_request_projects_json_and_sends_secret() {
        let body =
            serde_json::to_vec(&json!({"data": {"balance": "18", "currency": "CNY"}})).unwrap();
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        let (url, request) = mock_response(response).await;
        let config = custom_config(url);
        let preview = execute_and_project(&config, Some("test-secret"))
            .await
            .unwrap();
        let request = request.await.unwrap();
        assert!(request.contains("authorization: Bearer test-secret"));
        assert!(request.contains("x-test: present"));
        assert_eq!(preview.items[0].data, json!("18"));
        assert_eq!(preview.items[0].description.as_deref(), Some("CNY"));
    }

    #[tokio::test]
    async fn response_limit_is_enforced_while_streaming() {
        let body = vec![b'x'; MAX_RESPONSE_BYTES + 1];
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        let (url, request) = mock_response(response).await;
        let config = custom_config(url);
        let error = execute_and_project(&config, Some("test-secret"))
            .await
            .unwrap_err();
        request.await.unwrap();
        assert!(error.to_string().contains("256 KiB"));
    }
}

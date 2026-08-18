use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{RwLock, mpsc};
use url::Url;

use crate::{
    producer::{ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger},
    publisher::{ResourcePublisher, SemanticResource},
    state::{SharedState, SourceStatus, unix_now},
};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPES: &str = "openid profile email offline_access";
const REFRESH_SCOPES: &str = "openid profile email";
const SECRET_SERVICE: &str = "dev.epd-kit.agent.codex-oauth";
const MAX_SOURCES: usize = 16;
const MIN_INTERVAL_SEC: u64 = 60;
const MAX_INTERVAL_SEC: u64 = 3600;
const SESSION_TTL_SEC: u64 = 30 * 60;
const RESOURCE_TTL_SEC: u64 = 600;

pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "codex.oauth",
    title: "Codex OAuth",
    description: "通过独立 OAuth 账号读取 Codex 额度，无需启动 Codex",
    configurable: true,
    multi_instance: true,
    auto_sync: true,
    built_in_source: None,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CodexOAuthConfig {
    id: String,
    enabled: bool,
    title: String,
    interval_sec: u64,
}

impl CodexOAuthConfig {
    fn validate(&self) -> Result<()> {
        validate_source_id(&self.id)?;
        if self.title.trim().is_empty() || self.title.chars().count() > 32 {
            bail!("名称长度必须为 1-32 个字符");
        }
        if !(MIN_INTERVAL_SEC..=MAX_INTERVAL_SEC).contains(&self.interval_sec) {
            bail!("interval_sec 必须在 {MIN_INTERVAL_SEC}-{MAX_INTERVAL_SEC} 之间");
        }
        Ok(())
    }

    fn resource_key(&self) -> String {
        format!("codex/{}", self.id)
    }

    fn metrics_key(&self) -> String {
        format!("codex/{}/metrics", self.id)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct CodexOAuthStartInput {
    pub id: String,
    pub title: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_sec: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CodexOAuthCompleteInput {
    pub session_id: String,
    pub callback_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CodexOAuthUpdateInput {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_sec: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodexOAuthStart {
    pub session_id: String,
    pub auth_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodexOAuthView {
    pub id: String,
    pub enabled: bool,
    pub title: String,
    pub interval_sec: u64,
    pub authenticated: bool,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CodexOAuthFile {
    sources: Vec<CodexOAuthConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OAuthCredential {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    expires_at: u64,
    #[serde(default)]
    email: String,
    #[serde(default)]
    account_id: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    plan_type: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    expires_in: u64,
}

#[derive(Clone)]
struct OAuthSession {
    state: String,
    verifier: String,
    config: CodexOAuthConfig,
    reauthorizing: bool,
    created_at: Instant,
}

#[derive(Clone)]
pub struct CodexOAuthControl {
    sources: Arc<RwLock<Vec<CodexOAuthConfig>>>,
    sessions: Arc<RwLock<HashMap<String, OAuthSession>>>,
    config_path: Arc<PathBuf>,
    trigger: mpsc::Sender<ProducerTrigger>,
    source_trigger: mpsc::Sender<String>,
    publisher: ResourcePublisher,
}

impl CodexOAuthControl {
    pub fn spawn(context: ProducerContext) -> Result<Self> {
        let config_path = config_path()?;
        let sources = Arc::new(RwLock::new(load_sources(&config_path)?));
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let publisher = context.publisher.clone();
        let (trigger, receiver) = mpsc::channel(8);
        let (source_trigger, source_receiver) = mpsc::channel(16);
        tokio::spawn(run(context, sources.clone(), receiver, source_receiver));
        Ok(Self {
            sources,
            sessions,
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

    pub async fn sources(&self) -> Result<Vec<CodexOAuthView>> {
        let mut views = Vec::new();
        for config in self.sources.read().await.iter().cloned() {
            views.push(view(config.clone(), credential_get(config.id).await?).await);
        }
        Ok(views)
    }

    pub async fn start_oauth(
        &self,
        state: &SharedState,
        input: CodexOAuthStartInput,
    ) -> Result<CodexOAuthStart> {
        let config = CodexOAuthConfig {
            id: input.id,
            enabled: input.enabled,
            title: input.title,
            interval_sec: input.interval_sec,
        };
        config.validate()?;
        let is_existing = self
            .sources
            .read()
            .await
            .iter()
            .any(|item| item.id == config.id);
        if !is_existing
            && state
                .snapshot()
                .await
                .sources
                .iter()
                .any(|item| item.id == config.id)
        {
            bail!("数据源 ID 已存在：{}", config.id);
        }
        if !is_existing && self.sources.read().await.len() >= MAX_SOURCES {
            bail!("最多配置 {MAX_SOURCES} 个 Codex OAuth 账号");
        }

        let session_id = random_hex(16);
        let oauth_state = random_hex(32);
        let verifier = random_hex(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut auth_url = Url::parse(AUTHORIZE_URL)?;
        auth_url
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPES)
            .append_pair("state", &oauth_state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true");
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| session.created_at.elapsed().as_secs() < SESSION_TTL_SEC);
        sessions.insert(
            session_id.clone(),
            OAuthSession {
                state: oauth_state,
                verifier,
                config,
                reauthorizing: is_existing,
                created_at: Instant::now(),
            },
        );
        Ok(CodexOAuthStart {
            session_id,
            auth_url: auth_url.into(),
        })
    }

    pub async fn complete_oauth(
        &self,
        state: &SharedState,
        input: CodexOAuthCompleteInput,
    ) -> Result<CodexOAuthView> {
        let callback = Url::parse(input.callback_url.trim()).context("回调 URL 无效")?;
        let code = callback
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned())
            .ok_or_else(|| anyhow!("回调 URL 缺少 code"))?;
        let returned_state = callback
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .ok_or_else(|| anyhow!("回调 URL 缺少 state"))?;
        let session = self
            .sessions
            .read()
            .await
            .get(&input.session_id)
            .cloned()
            .ok_or_else(|| anyhow!("OAuth 会话不存在或已过期"))?;
        if session.created_at.elapsed().as_secs() >= SESSION_TTL_SEC {
            bail!("OAuth 会话已过期");
        }
        if returned_state != session.state {
            bail!("OAuth state 校验失败");
        }
        if !session.reauthorizing
            && state
                .snapshot()
                .await
                .sources
                .iter()
                .any(|item| item.id == session.config.id)
        {
            bail!("数据源 ID 已存在：{}", session.config.id);
        }

        let token = exchange_code(&code, &session.verifier).await?;
        if token.refresh_token.is_empty() {
            bail!("OAuth 未返回 refresh token，无法自动维护登录状态");
        }
        let credential = credential_from_token(token, None)?;
        self.sessions.write().await.remove(&input.session_id);
        let old_credential = credential_get(session.config.id.clone()).await?;
        credential_set(session.config.id.clone(), credential.clone()).await?;
        let save_result = {
            let mut sources = self.sources.write().await;
            let mut next = sources.clone();
            if let Some(current) = next.iter_mut().find(|item| item.id == session.config.id) {
                *current = session.config.clone();
            } else {
                next.push(session.config.clone());
            }
            save_sources(&self.config_path, &next).map(|_| {
                *sources = next;
            })
        };
        if let Err(error) = save_result {
            credential_restore(session.config.id.clone(), old_credential).await?;
            return Err(error);
        }
        state
            .register_source(source_status(&session.config, Some(&credential)))
            .await;
        self.refresh_source(&session.config.id).await?;
        Ok(view(session.config, Some(credential)).await)
    }

    pub async fn update_source(
        &self,
        state: &SharedState,
        id: &str,
        input: CodexOAuthUpdateInput,
    ) -> Result<CodexOAuthView> {
        let config = CodexOAuthConfig {
            id: input.id,
            enabled: input.enabled,
            title: input.title,
            interval_sec: input.interval_sec,
        };
        config.validate()?;
        if config.id != id {
            bail!("数据源 ID 创建后不可修改");
        }
        {
            let mut sources = self.sources.write().await;
            let mut next = sources.clone();
            let current = next
                .iter_mut()
                .find(|item| item.id == id)
                .ok_or_else(|| anyhow!("未知 Codex OAuth 数据源：{id}"))?;
            *current = config.clone();
            save_sources(&self.config_path, &next)?;
            *sources = next;
        }
        let credential = credential_get(id.to_owned()).await?;
        state
            .register_source(source_status(&config, credential.as_ref()))
            .await;
        self.refresh_source(id).await?;
        Ok(view(config, credential).await)
    }

    pub async fn delete_source(&self, state: &SharedState, id: &str) -> Result<()> {
        let config = {
            let mut sources = self.sources.write().await;
            let mut next = sources.clone();
            let index = next
                .iter()
                .position(|item| item.id == id)
                .ok_or_else(|| anyhow!("未知 Codex OAuth 数据源：{id}"))?;
            let config = next.remove(index);
            save_sources(&self.config_path, &next)?;
            *sources = next;
            config
        };
        credential_delete(id.to_owned()).await?;
        state.remove_source(id).await;
        self.publisher.delete(config.resource_key()).await?;
        self.publisher.delete(config.metrics_key()).await
    }

    pub async fn refresh_source(&self, id: &str) -> Result<()> {
        if !self.sources.read().await.iter().any(|item| item.id == id) {
            bail!("未知 Codex OAuth 数据源：{id}");
        }
        self.source_trigger
            .send(id.to_owned())
            .await
            .map_err(|_| anyhow!("Codex OAuth 数据源管理器已停止"))
    }
}

async fn run(
    context: ProducerContext,
    sources: Arc<RwLock<Vec<CodexOAuthConfig>>>,
    mut triggers: mpsc::Receiver<ProducerTrigger>,
    mut source_triggers: mpsc::Receiver<String>,
) {
    let mut due = HashMap::<String, Instant>::new();
    for config in sources.read().await.iter() {
        let credential = credential_get(config.id.clone()).await.ok().flatten();
        context
            .state
            .register_source(source_status(config, credential.as_ref()))
            .await;
        due.insert(config.id.clone(), Instant::now());
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

async fn collect(context: &ProducerContext, config: &CodexOAuthConfig) -> bool {
    match collect_inner(context, config).await {
        Ok(()) => true,
        Err(error) => {
            let message = error.to_string();
            context
                .state
                .update_source(&config.id, |source| {
                    source.phase =
                        if message.contains("OAuth 凭据") || message.contains("refresh token") {
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
                .log("warn", "codex.oauth", format!("{}: {message}", config.id))
                .await;
            false
        }
    }
}

async fn collect_inner(context: &ProducerContext, config: &CodexOAuthConfig) -> Result<()> {
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
    context
        .state
        .update_source(&config.id, |source| {
            source.phase = "syncing".into();
            source.last_error = None;
        })
        .await;
    let mut credential = credential_get(config.id.clone())
        .await?
        .ok_or_else(|| anyhow!("OAuth 凭据未配置"))?;
    if credential.expires_at <= unix_now() + 120 {
        credential = refresh_credential(&credential).await?;
        credential_set(config.id.clone(), credential.clone()).await?;
    }
    let mut response = request_usage(&credential).await?;
    if response.0 == StatusCode::UNAUTHORIZED {
        credential = refresh_credential(&credential).await?;
        credential_set(config.id.clone(), credential.clone()).await?;
        response = request_usage(&credential).await?;
    }
    if !response.0.is_success() {
        bail!(
            "Codex 额度接口返回 {}: {}",
            response.0,
            truncate(&response.1.to_string(), 500)
        );
    }
    let usage = response.1;
    let rate_limit = usage
        .get("rate_limit")
        .filter(|value| value.is_object())
        .ok_or_else(|| anyhow!("Codex 额度响应缺少 rate_limit"))?;
    let first = rate_limit
        .get("primary_window")
        .filter(|value| !value.is_null());
    let second = rate_limit
        .get("secondary_window")
        .filter(|value| !value.is_null());
    let (five_hour, seven_day) = normalized_usage_windows(first, second);
    let now = unix_now();
    let payload = json!({
        "source_status": "ok",
        "plan_type": usage.get("plan_type").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(&credential.plan_type),
        "selected": {
            "primary": normalize_usage_window(five_hour, now),
            "secondary": normalize_usage_window(seven_day, now),
        }
    });
    let metrics_payload = json!({
        "source_status": "ok",
        "title": config.title,
        "items": [
            percentage_metric("5h", five_hour),
            percentage_metric("7d", seven_day),
            reset_metric("5h 重置", five_hour, now),
            reset_metric("7d 重置", seven_day, now),
        ]
    });
    context
        .publisher
        .publish(SemanticResource {
            source_id: config.id.clone(),
            key: config.resource_key(),
            schema_id: "codex.rate_limits",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload,
        })
        .await?;
    context
        .publisher
        .publish(SemanticResource {
            source_id: config.id.clone(),
            key: config.metrics_key(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload: metrics_payload,
        })
        .await?;
    context
        .state
        .update_source(&config.id, |source| {
            source.phase = "ready".into();
            source.last_sync_at = Some(now);
            source.next_sync_at = Some(now + config.interval_sec);
            source.last_error = None;
            source.details["email"] = optional_string(&credential.email);
            source.details["plan_type"] = optional_string(&credential.plan_type);
            source.details["token_expires_at"] = json!(credential.expires_at);
        })
        .await;
    Ok(())
}

async fn exchange_code(code: &str, verifier: &str) -> Result<TokenResponse> {
    token_request(&[
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("code", code),
        ("redirect_uri", REDIRECT_URI),
        ("code_verifier", verifier),
    ])
    .await
}

async fn refresh_credential(current: &OAuthCredential) -> Result<OAuthCredential> {
    if current.refresh_token.trim().is_empty() {
        bail!("refresh token 缺失，请重新登录");
    }
    let token = token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", &current.refresh_token),
        ("client_id", CLIENT_ID),
        ("scope", REFRESH_SCOPES),
    ])
    .await
    .context("refresh token 更新失败，请重新登录")?;
    credential_from_token(token, Some(current))
}

async fn token_request(form: &[(&str, &str)]) -> Result<TokenResponse> {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .user_agent("codex-cli/0.91.0")
        .build()?;
    let response = client
        .post(TOKEN_URL)
        .form(form)
        .send()
        .await
        .context("OpenAI token 请求失败")?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "OpenAI token 接口返回 {status}: {}",
            truncate(&String::from_utf8_lossy(&bytes), 500)
        );
    }
    serde_json::from_slice(&bytes).context("OpenAI token 响应无效")
}

async fn request_usage(credential: &OAuthCredential) -> Result<(StatusCode, Value)> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(Policy::none())
        .user_agent(concat!("epd-agent/", env!("EPD_AGENT_VERSION")))
        .build()?;
    let response = client
        .get(USAGE_URL)
        .bearer_auth(&credential.access_token)
        .header("chatgpt-account-id", &credential.account_id)
        .header("openai-beta", "codex-1")
        .header("oai-language", "zh-CN")
        .header("originator", "Codex Desktop")
        .header("sec-fetch-site", "none")
        .header("sec-fetch-mode", "no-cors")
        .header("sec-fetch-dest", "empty")
        .header("priority", "u=4, i")
        .send()
        .await
        .context("Codex 额度请求失败")?;
    let status = response.status();
    let bytes = response.bytes().await?;
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        json!({
            "error": String::from_utf8_lossy(&bytes).chars().take(500).collect::<String>()
        })
    });
    Ok((status, value))
}

fn credential_from_token(
    token: TokenResponse,
    current: Option<&OAuthCredential>,
) -> Result<OAuthCredential> {
    let claims = decode_jwt_claims(if token.id_token.is_empty() {
        &token.access_token
    } else {
        &token.id_token
    })?;
    let auth = claims
        .get("https://api.openai.com/auth")
        .cloned()
        .unwrap_or(Value::Null);
    let account_id = string_at(&auth, "chatgpt_account_id")
        .or_else(|| current.map(|item| item.account_id.clone()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("OAuth token 缺少 chatgpt_account_id"))?;
    Ok(OAuthCredential {
        access_token: token.access_token,
        refresh_token: if token.refresh_token.is_empty() {
            current
                .map(|item| item.refresh_token.clone())
                .unwrap_or_default()
        } else {
            token.refresh_token
        },
        id_token: if token.id_token.is_empty() {
            current
                .map(|item| item.id_token.clone())
                .unwrap_or_default()
        } else {
            token.id_token
        },
        expires_at: unix_now() + token.expires_in,
        email: string_at(&claims, "email")
            .or_else(|| current.map(|item| item.email.clone()))
            .unwrap_or_default(),
        account_id,
        user_id: string_at(&auth, "chatgpt_user_id")
            .or_else(|| current.map(|item| item.user_id.clone()))
            .unwrap_or_default(),
        plan_type: string_at(&auth, "chatgpt_plan_type")
            .or_else(|| current.map(|item| item.plan_type.clone()))
            .unwrap_or_else(|| "unknown".into()),
    })
}

fn decode_jwt_claims(token: &str) -> Result<Value> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow!("OAuth token 不是有效 JWT"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .context("OAuth JWT payload 无效")?;
    serde_json::from_slice(&bytes).context("OAuth JWT claims 无效")
}

fn normalize_usage_window(value: Option<&Value>, now: u64) -> Value {
    let Some(window) = value else {
        return Value::Null;
    };
    json!({
        "used_percent": window.get("used_percent").cloned().unwrap_or(json!(0)),
        "window_duration_mins": window.get("limit_window_seconds").and_then(Value::as_u64).map(|v| v / 60),
        "resets_at": usage_reset_at(window, now),
    })
}

fn normalized_usage_windows<'a>(
    first: Option<&'a Value>,
    second: Option<&'a Value>,
) -> (Option<&'a Value>, Option<&'a Value>) {
    let windows = [first, second];
    let duration = |window: Option<&Value>| {
        window
            .and_then(|value| value.get("limit_window_seconds"))
            .and_then(Value::as_u64)
    };
    let exact_index = |seconds| {
        windows
            .iter()
            .position(|window| duration(*window) == Some(seconds))
    };

    let five_hour_index = exact_index(5 * 60 * 60);
    let seven_day_index = exact_index(7 * 24 * 60 * 60);
    let five_hour_index = five_hour_index.or_else(|| {
        windows
            .iter()
            .enumerate()
            .filter(|(index, window)| window.is_some() && Some(*index) != seven_day_index)
            .min_by_key(|(_, window)| duration(**window).unwrap_or(u64::MAX))
            .map(|(index, _)| index)
    });
    let seven_day_index = seven_day_index.or_else(|| {
        windows
            .iter()
            .enumerate()
            .filter(|(index, window)| window.is_some() && Some(*index) != five_hour_index)
            .max_by_key(|(_, window)| duration(**window).unwrap_or(0))
            .map(|(index, _)| index)
    });

    (
        five_hour_index.and_then(|index| windows[index]),
        seven_day_index.and_then(|index| windows[index]),
    )
}

fn percentage_metric(label: &str, value: Option<&Value>) -> Value {
    let remaining = value
        .and_then(|window| window.get("used_percent"))
        .and_then(Value::as_f64)
        .map(|used| (100.0 - used.clamp(0.0, 100.0)).round() as u64);
    json!({
        "label": label,
        "data": remaining.map(Value::from).unwrap_or_else(|| json!("--")),
        "description": "剩余",
        "progress": remaining,
        "format": "percent",
    })
}

fn reset_metric(label: &str, value: Option<&Value>, now: u64) -> Value {
    let resets_at = value.and_then(|window| usage_reset_at(window, now));
    json!({
        "label": label,
        "data": resets_at.map(Value::from).unwrap_or_else(|| json!("--")),
        "description": "距重置",
        "format": "countdown",
    })
}

fn usage_reset_at(window: &Value, now: u64) -> Option<u64> {
    window.get("reset_at").and_then(Value::as_u64).or_else(|| {
        window
            .get("reset_after_seconds")
            .and_then(Value::as_u64)
            .map(|v| now + v)
    })
}

fn source_status(config: &CodexOAuthConfig, credential: Option<&OAuthCredential>) -> SourceStatus {
    SourceStatus {
        id: config.id.clone(),
        type_id: MANIFEST.id.into(),
        title: config.title.clone(),
        enabled: config.enabled,
        interval_sec: Some(config.interval_sec),
        phase: if !config.enabled {
            "disabled"
        } else if credential.is_none() {
            "auth_required"
        } else {
            "starting"
        }
        .into(),
        resource_keys: vec![config.resource_key(), config.metrics_key()],
        next_sync_at: (config.enabled && credential.is_some()).then(|| unix_now() + 1),
        last_error: credential.is_none().then(|| "OAuth 凭据未配置".into()),
        details: json!({
            "auth_mode": "oauth",
            "email": credential.map(|item| item.email.as_str()).filter(|value| !value.is_empty()),
            "plan_type": credential.map(|item| item.plan_type.as_str()).filter(|value| !value.is_empty()),
            "token_expires_at": credential.map(|item| item.expires_at),
            "interval_sec": config.interval_sec,
        }),
        ..Default::default()
    }
}

async fn view(config: CodexOAuthConfig, credential: Option<OAuthCredential>) -> CodexOAuthView {
    CodexOAuthView {
        id: config.id,
        enabled: config.enabled,
        title: config.title,
        interval_sec: config.interval_sec,
        authenticated: credential.is_some(),
        email: credential
            .as_ref()
            .map(|item| item.email.clone())
            .filter(|value| !value.is_empty()),
        plan_type: credential
            .as_ref()
            .map(|item| item.plan_type.clone())
            .filter(|value| !value.is_empty()),
        expires_at: credential.map(|item| item.expires_at),
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
    Ok(directory.join("codex-oauth-sources.json"))
}

fn load_sources(path: &Path) -> Result<Vec<CodexOAuthConfig>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file: CodexOAuthFile =
        serde_json::from_slice(&std::fs::read(path)?).context("Codex OAuth 配置 JSON 无效")?;
    if file.sources.len() > MAX_SOURCES {
        bail!("Codex OAuth 数据源数量超过上限 {MAX_SOURCES}");
    }
    for (index, source) in file.sources.iter().enumerate() {
        source.validate()?;
        if file.sources[..index]
            .iter()
            .any(|item| item.id == source.id)
        {
            bail!("重复的 Codex OAuth 数据源 ID：{}", source.id);
        }
    }
    Ok(file.sources)
}

fn save_sources(path: &Path, sources: &[CodexOAuthConfig]) -> Result<()> {
    let contents = serde_json::to_vec_pretty(&CodexOAuthFile {
        sources: sources.to_vec(),
    })?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).context("打开 Codex OAuth 配置失败")?;
    file.write_all(&contents)
        .context("写入 Codex OAuth 配置失败")?;
    file.sync_all().context("同步 Codex OAuth 配置失败")
}

fn credential_entry(id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SECRET_SERVICE, id).context("打开系统凭据库失败")
}

async fn credential_get(id: String) -> Result<Option<OAuthCredential>> {
    tokio::task::spawn_blocking(move || match credential_entry(&id)?.get_password() {
        Ok(secret) => serde_json::from_str(&secret)
            .map(Some)
            .context("Codex OAuth 凭据无效"),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => bail!("读取系统凭据库失败"),
    })
    .await
    .context("系统凭据任务被取消")?
}

async fn credential_set(id: String, credential: OAuthCredential) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        credential_entry(&id)?
            .set_password(&serde_json::to_string(&credential)?)
            .map_err(|_| anyhow!("写入系统凭据库失败"))
    })
    .await
    .context("系统凭据任务被取消")?
}

async fn credential_delete(id: String) -> Result<()> {
    tokio::task::spawn_blocking(move || match credential_entry(&id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => bail!("删除系统凭据失败"),
    })
    .await
    .context("系统凭据任务被取消")?
}

async fn credential_restore(id: String, credential: Option<OAuthCredential>) -> Result<()> {
    match credential {
        Some(credential) => credential_set(id, credential).await,
        None => credential_delete(id).await,
    }
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.is_empty())
}

fn optional_string(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        json!(value)
    }
}

fn default_true() -> bool {
    true
}
fn default_interval() -> u64 {
    60
}
fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

use std::{convert::Infallible, sync::Arc};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{delete, get, patch, post},
};
use futures::StreamExt;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::wrappers::BroadcastStream;

use crate::{autostart, ble::BleGateway, codex::CodexControl, state::SharedState};

include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));

#[derive(Clone)]
pub struct WebContext {
    pub state: Arc<SharedState>,
    pub ble: BleGateway,
    pub codex: CodexControl,
    auth: Arc<Auth>,
}

struct Auth {
    install_token: String,
    session_token: String,
}

impl WebContext {
    pub fn new(state: Arc<SharedState>, ble: BleGateway, codex: CodexControl) -> Result<Self> {
        Ok(Self {
            state,
            ble,
            codex,
            auth: Arc::new(Auth {
                install_token: load_install_token()?,
                session_token: random_token(),
            }),
        })
    }

    pub fn launch_url(&self, port: u16) -> String {
        format!("http://127.0.0.1:{port}/#token={}", self.auth.install_token)
    }
}

pub fn router(context: WebContext) -> Router {
    Router::new()
        .route("/api/v1/session", post(create_session))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/events", get(events))
        .route("/api/v1/device/scan", post(device_scan))
        .route("/api/v1/device/connect", post(device_connect))
        .route("/api/v1/device/disconnect", post(device_disconnect))
        .route("/api/v1/device/auto-connect", post(device_auto_connect))
        .route("/api/v1/device/reload", post(device_reload))
        .route("/api/v1/device/config", patch(config_patch))
        .route(
            "/api/v1/device/resource",
            get(resource_get).delete(resource_delete),
        )
        .route("/api/v1/device/view", post(view_set))
        .route("/api/v1/device/refresh", post(display_refresh))
        .route("/api/v1/device/restart", post(device_restart))
        .route("/api/v1/codex/refresh", post(codex_refresh))
        .route("/api/v1/agent/pause", post(agent_pause))
        .route("/api/v1/agent/autostart", post(agent_autostart))
        .route("/api/v1/security/enrollment", post(enrollment))
        .route("/api/v1/security/bonds/{id}", delete(revoke_bond))
        .route("/api/v1/security/owner/{id}", post(transfer_owner))
        .route("/api/v1/factory/prepare", post(factory_prepare))
        .route("/api/v1/factory/commit", post(factory_commit))
        .fallback(static_asset)
        .with_state(context)
}

#[derive(Deserialize)]
struct SessionInput {
    token: String,
}

async fn create_session(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<SessionInput>,
) -> ApiResult<Response> {
    validate_origin(&headers)?;
    if !constant_time_eq(
        input.token.as_bytes(),
        context.auth.install_token.as_bytes(),
    ) {
        return Err(ApiError::unauthorized("invalid install token"));
    }
    let cookie = format!(
        "epd_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400",
        context.auth.session_token
    );
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(ApiError::internal)?,
    );
    context
        .state
        .log("info", "web", "local management session established")
        .await;
    Ok(response)
}

async fn snapshot(State(context): State<WebContext>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    authenticate(&context, &headers)?;
    Ok(Json(
        serde_json::to_value(context.state.snapshot().await).map_err(ApiError::internal)?,
    ))
}

async fn events(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    authenticate(&context, &headers)?;
    let receiver = context.state.subscribe();
    context
        .state
        .log("info", "web", "SSE client connected")
        .await;
    let stream = BroadcastStream::new(receiver).filter_map(|message| async move {
        match message {
            Ok(data) => Some(Ok::<Event, Infallible>(
                Event::default().event("snapshot").data(data),
            )),
            Err(_) => None,
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn device_reload(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    reload_device(&context).await.map_err(ApiError::internal)?;
    context
        .state
        .log("info", "web", "device state reloaded")
        .await;
    Ok(Json(json!({ "ok": true })))
}

async fn device_scan(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    context.ble.scan().await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct DeviceConnectInput {
    id: String,
}

async fn device_connect(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<DeviceConnectInput>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    context
        .ble
        .connect_device(input.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true })))
}

async fn device_disconnect(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    context.ble.disconnect().await;
    Ok(Json(json!({ "ok": true })))
}

async fn device_auto_connect(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    context.ble.auto_connect().await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ConfigPatch {
    patch: Value,
}

async fn config_patch(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<ConfigPatch>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let revision = context
        .state
        .snapshot()
        .await
        .device
        .config
        .as_ref()
        .and_then(|config| config.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    context
        .ble
        .request("config.patch", json!({ "patch": input.patch }))
        .await
        .map_err(ApiError::internal)?;
    let result = match context
        .ble
        .request("config.commit", json!({ "expected_revision": revision }))
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let _ = context.ble.request("config.discard", json!({})).await;
            return Err(ApiError::internal(error));
        }
    };
    reload_device(&context).await.map_err(ApiError::internal)?;
    context
        .state
        .log(
            "info",
            "web",
            format!(
                "configuration committed; revision={} restart_required={}",
                result
                    .get("revision")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                result
                    .get("restart_required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
        )
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

#[derive(Deserialize)]
struct ResourceQuery {
    key: String,
}

async fn resource_get(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Query(input): Query<ResourceQuery>,
) -> ApiResult<Json<Value>> {
    authenticate(&context, &headers)?;
    let result = context
        .ble
        .request("resource.get", json!({ "key": input.key }))
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn resource_delete(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Query(input): Query<ResourceQuery>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request("resource.delete", json!({ "key": input.key }))
        .await
        .map_err(ApiError::internal)?;
    reload_device(&context).await.map_err(ApiError::internal)?;
    context.state.log("info", "web", "resource deleted").await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

#[derive(Deserialize)]
struct ViewInput {
    renderer_id: String,
    resource_key: String,
}

async fn view_set(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<ViewInput>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request(
            "view.set",
            json!({
                "renderer_id": input.renderer_id,
                "resource_key": input.resource_key,
            }),
        )
        .await
        .map_err(ApiError::internal)?;
    reload_device(&context).await.map_err(ApiError::internal)?;
    context
        .state
        .log("info", "web", "active renderer and resource updated")
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

#[derive(Deserialize)]
struct RefreshInput {
    #[serde(default = "auto_mode")]
    mode: String,
}
fn auto_mode() -> String {
    "auto".into()
}

async fn display_refresh(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<RefreshInput>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request("display.refresh", json!({ "mode": input.mode }))
        .await
        .map_err(ApiError::internal)?;
    context
        .state
        .log("info", "web", "display refresh scheduled")
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn device_restart(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request("system.restart", json!({}))
        .await
        .map_err(ApiError::internal)?;
    context
        .state
        .log("warn", "web", "device restart scheduled")
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn codex_refresh(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    context.codex.refresh().await.map_err(ApiError::internal)?;
    context
        .state
        .log("info", "web", "manual Codex refresh queued")
        .await;
    Ok(Json(json!({ "ok": true, "queued": true })))
}

#[derive(Deserialize)]
struct ToggleInput {
    enabled: bool,
}

async fn agent_pause(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<ToggleInput>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    context.state.set_paused(input.enabled).await;
    context
        .state
        .log(
            "info",
            "agent",
            if input.enabled {
                "BLE synchronization paused"
            } else {
                "BLE synchronization resumed"
            },
        )
        .await;
    Ok(Json(json!({ "ok": true, "paused": input.enabled })))
}

async fn agent_autostart(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<ToggleInput>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    autostart::set_enabled(input.enabled).map_err(ApiError::internal)?;
    context.state.set_autostart(input.enabled).await;
    context
        .state
        .log(
            "info",
            "agent",
            if input.enabled {
                "login autostart enabled"
            } else {
                "login autostart disabled"
            },
        )
        .await;
    Ok(Json(json!({ "ok": true, "enabled": input.enabled })))
}

async fn enrollment(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<ToggleInput>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let op = if input.enabled {
        "security.enrollment.open"
    } else {
        "security.enrollment.close"
    };
    let result = context
        .ble
        .request(op, json!({}))
        .await
        .map_err(ApiError::internal)?;
    context
        .state
        .log(
            "info",
            "web",
            if input.enabled {
                "trusted-host enrollment opened"
            } else {
                "trusted-host enrollment closed"
            },
        )
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn revoke_bond(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request("security.bonds.revoke", json!({ "bond_id": id }))
        .await
        .map_err(ApiError::internal)?;
    reload_device(&context).await.map_err(ApiError::internal)?;
    context
        .state
        .log("warn", "web", "trusted bond revoked")
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn transfer_owner(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request("security.owner.transfer", json!({ "bond_id": id }))
        .await
        .map_err(ApiError::internal)?;
    context
        .state
        .log("warn", "web", "device ownership transferred")
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn factory_prepare(
    State(context): State<WebContext>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request("factory_reset.prepare", json!({}))
        .await
        .map_err(ApiError::internal)?;
    context
        .state
        .log("warn", "web", "factory reset confirmation requested")
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

#[derive(Deserialize)]
struct FactoryCommit {
    code: u32,
}

async fn factory_commit(
    State(context): State<WebContext>,
    headers: HeaderMap,
    Json(input): Json<FactoryCommit>,
) -> ApiResult<Json<Value>> {
    mutation_auth(&context, &headers)?;
    let result = context
        .ble
        .request("factory_reset.commit", json!({ "code": input.code }))
        .await
        .map_err(ApiError::internal)?;
    context
        .state
        .log("warn", "web", "factory reset committed")
        .await;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn reload_device(context: &WebContext) -> Result<()> {
    let config = context.ble.request("config.get", json!({})).await?;
    let capabilities = context.ble.request("capabilities.get", json!({})).await?;
    let resources = context.ble.request("resource.list", json!({})).await?;
    let bonds = context
        .ble
        .request("security.bonds.list", json!({}))
        .await?;
    let diagnostics = context.ble.request("diagnostics.get", json!({})).await.ok();
    context
        .state
        .update_device(|device| {
            device.config = config.get("config").cloned();
            device.capabilities = Some(capabilities);
            device.resources = resources
                .get("resources")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            device.bonds = bonds
                .get("bonds")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            device.diagnostics = diagnostics;
        })
        .await;
    Ok(())
}

async fn static_asset(State(_context): State<WebContext>, request: Request<Body>) -> Response {
    let requested = request.uri().path().trim_start_matches('/');
    let path = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    let asset = WEB_ASSETS
        .iter()
        .find(|(name, _, _)| *name == path)
        .or_else(|| WEB_ASSETS.iter().find(|(name, _, _)| *name == "index.html"));
    let Some((_, bytes, mime)) = asset else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Web assets were not embedded",
        )
            .into_response();
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, *mime)
        .header(
            header::CACHE_CONTROL,
            if path == "index.html" {
                "no-store"
            } else {
                "public, max-age=31536000, immutable"
            },
        )
        .body(Body::from(*bytes))
        .unwrap()
}

fn mutation_auth(context: &WebContext, headers: &HeaderMap) -> ApiResult<()> {
    validate_origin(headers)?;
    authenticate(context, headers)
}

fn validate_origin(headers: &HeaderMap) -> ApiResult<()> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if origin.is_empty()
        || origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("http://localhost:")
    {
        return Ok(());
    }
    Err(ApiError::forbidden("cross-origin request rejected"))
}

fn authenticate(context: &WebContext, headers: &HeaderMap) -> ApiResult<()> {
    let cookies = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let valid = cookies.split(';').map(str::trim).any(|cookie| {
        cookie.strip_prefix("epd_session=").is_some_and(|value| {
            constant_time_eq(value.as_bytes(), context.auth.session_token.as_bytes())
        })
    });
    if valid {
        Ok(())
    } else {
        Err(ApiError::unauthorized("local session required"))
    }
}

fn load_install_token() -> Result<String> {
    let directory = dirs::config_dir()
        .ok_or_else(|| anyhow!("config directory unavailable"))?
        .join("epd-agent");
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = directory.join("local-token");
    if path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        let token = std::fs::read_to_string(&path).context("read local API token")?;
        let token = token.trim().to_owned();
        if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(token);
        }
    }
    let token = random_token();
    write_private_file(&path, token.as_bytes()).context("write local API token")?;
    Ok(token)
}

fn write_private_file(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

type ApiResult<T> = std::result::Result<T, ApiError>;

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "message": self.message } })),
        )
            .into_response()
    }
}

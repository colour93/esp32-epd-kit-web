use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use hmac::{Hmac, Mac};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{broadcast, mpsc, oneshot, watch},
};

use crate::{
    protocol::{self, FrameAssembler},
    rpc::{self, FrameChannel},
    state::{DeviceCandidate, SharedState, unix_now},
};

const SERVICE_TYPE: &str = "_epdkit._tcp.local.";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(18);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FRAME_BYTES: usize = 2048;
const DEFAULT_PORT: u16 = 38474;
const SECRET_SERVICE: &str = "dev.epd-kit.agent.lan-device";

type HmacSha256 = Hmac<Sha256>;

struct Command {
    op: String,
    args: Value,
    reply: oneshot::Sender<Result<Value>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct LanTarget {
    pub id: String,
    pub name: String,
    pub endpoint: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirectTarget {
    endpoint: SocketAddr,
    secret: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ConnectionIntent {
    Auto,
    Scan,
    Manual(LanTarget),
    Direct(DirectTarget),
    Idle,
}

struct ConnectionAttempt {
    target: LanTarget,
    expected_device_id: Option<String>,
    bootstrap_secret: Option<String>,
}

#[derive(Clone)]
pub struct LanGateway {
    commands: mpsc::Sender<Command>,
    intent: watch::Sender<ConnectionIntent>,
    device_events: broadcast::Sender<String>,
    connected: Arc<AtomicBool>,
    state: Arc<SharedState>,
}

impl LanGateway {
    pub fn spawn(state: Arc<SharedState>, auto_connect: bool) -> Self {
        let (commands, receiver) = mpsc::channel(32);
        let initial_intent = if auto_connect {
            ConnectionIntent::Auto
        } else {
            ConnectionIntent::Idle
        };
        let (intent, intent_receiver) = watch::channel(initial_intent);
        let (device_events, _) = broadcast::channel(32);
        let connected = Arc::new(AtomicBool::new(false));
        let gateway = Self {
            commands,
            intent,
            device_events,
            connected,
            state: state.clone(),
        };
        tokio::spawn(supervisor(
            state,
            receiver,
            gateway.intent.clone(),
            intent_receiver,
            gateway.device_events.clone(),
            gateway.connected.clone(),
        ));
        gateway
    }

    pub async fn request(&self, op: impl Into<String>, args: Value) -> Result<Value> {
        if !self.connected.load(Ordering::Acquire) {
            bail!("LAN device is not connected");
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command {
                op: op.into(),
                args,
                reply,
            })
            .await
            .map_err(|_| anyhow!("LAN supervisor stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("LAN request was cancelled"))?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.device_events.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub async fn scan(&self) {
        self.state
            .update_device(|device| {
                device.phase = "scanning".into();
                device.transport = "lan".into();
                device.connection_mode = "scan".into();
                device.selected_device_id = None;
                device.candidates.clear();
                device.scan_observed = 0;
                device.scan_started_at = Some(unix_now());
                device.last_error = None;
            })
            .await;
        self.intent.send_replace(ConnectionIntent::Scan);
        self.state
            .log("info", "lan", "manual LAN discovery requested")
            .await;
    }

    pub async fn connect_device(&self, id: String, secret: Option<String>) -> Result<()> {
        let id = id.trim().to_owned();
        validate_device_id(&id)?;
        if let Some(secret) = secret {
            validate_secret(&secret)?;
            credential_set(id.clone(), secret).await?;
        }
        if credential_get(id.clone()).await?.is_none() {
            bail!("LAN device key is required");
        }
        let candidate = self
            .state
            .snapshot()
            .await
            .device
            .candidates
            .into_iter()
            .find(|candidate| candidate.transport == "lan" && candidate.id == id)
            .ok_or_else(|| anyhow!("selected LAN device is no longer in the discovery results"))?;
        let endpoint = candidate
            .endpoint
            .as_deref()
            .ok_or_else(|| anyhow!("selected LAN device has no reachable endpoint"))?
            .parse()
            .context("parse LAN device endpoint")?;
        let target = LanTarget {
            id,
            name: candidate.name,
            endpoint,
        };
        self.state
            .update_device(|device| {
                device.phase = "connecting".into();
                device.transport = "lan".into();
                device.connection_mode = "manual".into();
                device.selected_device_id = Some(target.id.clone());
                device.last_error = None;
            })
            .await;
        self.intent
            .send_replace(ConnectionIntent::Manual(target.clone()));
        self.state
            .log(
                "info",
                "lan",
                format!("manual LAN connection requested for {}", target.id),
            )
            .await;
        Ok(())
    }

    pub async fn connect_endpoint(&self, endpoint: String, secret: Option<String>) -> Result<()> {
        let endpoint = parse_endpoint(&endpoint)?;
        let secret = secret
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if let Some(secret) = secret.as_deref() {
            validate_secret(secret)?;
        }
        self.state
            .update_device(|device| {
                device.phase = "connecting".into();
                device.transport = "lan".into();
                device.connection_mode = "manual".into();
                device.selected_device_id = None;
                device.name = Some(endpoint.to_string());
                device.last_error = None;
            })
            .await;
        self.intent
            .send_replace(ConnectionIntent::Direct(DirectTarget { endpoint, secret }));
        self.state
            .log(
                "info",
                "lan",
                format!("direct LAN connection requested for {endpoint}"),
            )
            .await;
        Ok(())
    }

    pub async fn auto_connect(&self) {
        self.state
            .update_device(|device| {
                device.phase = "scanning".into();
                device.transport = "lan".into();
                device.connection_mode = "auto".into();
                device.selected_device_id = None;
                device.scan_started_at = Some(unix_now());
                device.last_error = None;
            })
            .await;
        self.intent.send_replace(ConnectionIntent::Auto);
        self.state
            .log("info", "lan", "automatic LAN connection enabled")
            .await;
    }

    pub async fn disconnect(&self) {
        let disconnecting = self.connected.load(Ordering::Acquire);
        self.state
            .update_device(|device| {
                if device.transport == "lan" {
                    device.phase = if disconnecting {
                        "disconnecting".into()
                    } else {
                        "idle".into()
                    };
                    device.connection_mode = "idle".into();
                    device.selected_device_id = None;
                    device.last_error = None;
                }
            })
            .await;
        self.intent.send_replace(ConnectionIntent::Idle);
        self.state
            .log("info", "lan", "LAN disconnect requested")
            .await;
    }
}

struct LanSession {
    stream: TcpStream,
    device_id: String,
    device_name: String,
    endpoint: SocketAddr,
    assembler: FrameAssembler,
    next_id: AtomicU32,
    frame_bytes: usize,
}

impl FrameChannel for LanSession {
    fn transport_name(&self) -> &'static str {
        "lan"
    }

    fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    fn next_request_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn assembler(&mut self) -> &mut FrameAssembler {
        &mut self.assembler
    }

    async fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() > MAX_FRAME_BYTES || frame.len() > u16::MAX as usize {
            bail!("LAN frame exceeds supported size");
        }
        self.stream
            .write_u16_le(frame.len() as u16)
            .await
            .context("write LAN frame length")?;
        self.stream
            .write_all(frame)
            .await
            .context("write LAN frame")?;
        self.stream.flush().await.context("flush LAN frame")
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>> {
        let length = self
            .stream
            .read_u16_le()
            .await
            .context("read LAN frame length")? as usize;
        if !(8..=MAX_FRAME_BYTES).contains(&length) {
            bail!("LAN frame length {length} is outside supported limits");
        }
        let mut frame = vec![0u8; length];
        self.stream
            .read_exact(&mut frame)
            .await
            .context("read LAN frame")?;
        Ok(frame)
    }
}

async fn supervisor(
    state: Arc<SharedState>,
    mut commands: mpsc::Receiver<Command>,
    intent_sender: watch::Sender<ConnectionIntent>,
    mut intent: watch::Receiver<ConnectionIntent>,
    device_events: broadcast::Sender<String>,
    connected: Arc<AtomicBool>,
) {
    let mut preferred_target = match load_saved_target() {
        Ok(target) => target,
        Err(error) => {
            state
                .log(
                    "warn",
                    "lan",
                    format!("cannot load saved LAN target: {error:#}"),
                )
                .await;
            None
        }
    };
    let mut preferred_device_id = preferred_target.as_ref().map(|target| target.id.clone());
    state.log("info", "lan", "LAN supervisor started").await;
    loop {
        let desired = intent.borrow_and_update().clone();
        if desired == ConnectionIntent::Idle {
            if intent.changed().await.is_err() {
                return;
            }
            continue;
        }
        if state.paused().await {
            if state.snapshot().await.device.transport == "lan" {
                state
                    .update_device(|device| device.phase = "paused".into())
                    .await;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }

        let attempt = match &desired {
            ConnectionIntent::Manual(target) => Some(ConnectionAttempt {
                target: target.clone(),
                expected_device_id: Some(target.id.clone()),
                bootstrap_secret: None,
            }),
            ConnectionIntent::Direct(target) => Some(ConnectionAttempt {
                target: LanTarget {
                    id: String::new(),
                    name: target.endpoint.to_string(),
                    endpoint: target.endpoint,
                },
                expected_device_id: None,
                bootstrap_secret: target.secret.clone(),
            }),
            ConnectionIntent::Auto => preferred_target.clone().map(|target| ConnectionAttempt {
                expected_device_id: Some(target.id.clone()),
                target,
                bootstrap_secret: None,
            }),
            ConnectionIntent::Scan => {
                match discover(&state, &desired, None, &mut intent).await {
                    Ok(_) => {}
                    Err(error) => {
                        state
                            .log("warn", "lan", format!("LAN discovery failed: {error:#}"))
                            .await;
                    }
                }
                if intent.borrow().clone() == desired {
                    intent_sender.send_replace(ConnectionIntent::Idle);
                    intent.borrow_and_update();
                    state
                        .update_device(|device| {
                            if device.transport == "lan" {
                                device.phase = "idle".into();
                                device.connection_mode = "idle".into();
                            }
                        })
                        .await;
                }
                continue;
            }
            ConnectionIntent::Idle => None,
        };
        let attempt = match attempt {
            Some(attempt) => attempt,
            None => match discover(
                &state,
                &desired,
                preferred_device_id.as_deref(),
                &mut intent,
            )
            .await
            {
                Ok(Some(target)) => ConnectionAttempt {
                    expected_device_id: Some(target.id.clone()),
                    target,
                    bootstrap_secret: None,
                },
                Ok(None) => continue,
                Err(error) => {
                    state
                        .update_device(|device| {
                            if device.transport == "lan" {
                                device.phase = "unavailable".into();
                                device.last_error = Some(format!("{error:#}"));
                            }
                        })
                        .await;
                    state.log("warn", "lan", format!("{error:#}")).await;
                    wait_or_control_change(&mut intent, Duration::from_secs(2)).await;
                    continue;
                }
            },
        };

        let connection = tokio::select! {
            result = connect(&state, &attempt) => Some(result),
            changed = intent.changed() => {
                if changed.is_err() { return; }
                None
            }
        };
        let Some(connection) = connection else {
            continue;
        };
        let mut session = match connection {
            Ok(session) => session,
            Err(error) => {
                if matches!(desired, ConnectionIntent::Auto) {
                    // A saved DHCP endpoint may be stale. The next pass resolves
                    // the stable device id through mDNS before retrying.
                    preferred_target = None;
                }
                state
                    .update_device(|device| {
                        if device.transport == "lan" {
                            device.phase = "reconnecting".into();
                            device.last_error = Some(format!("{error:#}"));
                        }
                    })
                    .await;
                state
                    .log("warn", "lan", format!("cannot connect: {error:#}"))
                    .await;
                wait_or_control_change(&mut intent, Duration::from_secs(2)).await;
                continue;
            }
        };

        let prime_result = tokio::select! {
            result = prime_session(&state, &mut session, &device_events) => Some(result),
            changed = intent.changed() => {
                if changed.is_err() { return; }
                None
            }
        };
        let Some(prime_result) = prime_result else {
            continue;
        };
        if let Err(error) = prime_result {
            state
                .update_device(|device| {
                    if device.transport == "lan" {
                        device.phase = "reconnecting".into();
                        device.last_error = Some(format!("{error:#}"));
                    }
                })
                .await;
            state
                .log("warn", "lan", format!("LAN handshake failed: {error:#}"))
                .await;
            wait_or_control_change(&mut intent, Duration::from_secs(2)).await;
            continue;
        }

        let target = LanTarget {
            id: session.device_id.clone(),
            name: session.device_name.clone(),
            endpoint: session.endpoint,
        };
        if let Err(error) = save_target(&target) {
            state
                .log("warn", "lan", format!("cannot save LAN target: {error:#}"))
                .await;
        } else {
            preferred_target = Some(target.clone());
            preferred_device_id = Some(target.id.clone());
            state
                .update_device(|device| {
                    if device.transport == "lan" {
                        device.preferred_device_id = Some(target.id.clone());
                    }
                })
                .await;
        }
        connected.store(true, Ordering::Release);
        let _ = device_events.send("device.connected".into());
        state
            .log(
                "info",
                "lan",
                format!("LAN session ready at {}", session.endpoint),
            )
            .await;
        let mut health = tokio::time::interval_at(
            tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
            HEARTBEAT_INTERVAL,
        );
        health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut control_changed = false;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { return };
                    let result = rpc::transact(
                        &state,
                        &mut session,
                        &device_events,
                        &command.op,
                        command.args,
                        REQUEST_TIMEOUT,
                    ).await;
                    let link_failed = result.as_ref().err().is_some_and(rpc::is_link_error);
                    let _ = command.reply.send(result);
                    if link_failed { break; }
                }
                frame = session.read_frame() => {
                    let Ok(frame) = frame else { break };
                    if let Err(error) = rpc::handle_frame(
                        &state,
                        &mut session.assembler,
                        &device_events,
                        &frame,
                    ).await {
                        state.log("warn", "lan", error.to_string()).await;
                    }
                }
                _ = health.tick() => {
                    if state.paused().await { break; }
                    if let Err(error) = rpc::transact(
                        &state,
                        &mut session,
                        &device_events,
                        "system.status",
                        json!({}),
                        HEARTBEAT_TIMEOUT,
                    ).await {
                        state.log("warn", "lan", format!("LAN heartbeat failed: {error:#}")).await;
                        break;
                    }
                }
                changed = intent.changed() => {
                    control_changed = true;
                    if changed.is_err() {
                        connected.store(false, Ordering::Release);
                        return;
                    }
                    break;
                }
            }
        }
        connected.store(false, Ordering::Release);
        let _ = device_events.send("device.disconnected".into());
        let next = intent.borrow().clone();
        state
            .update_device(|device| {
                if device.transport == "lan" {
                    device.phase = if next == ConnectionIntent::Idle {
                        "idle".into()
                    } else {
                        "reconnecting".into()
                    };
                    device.connection_mode = match next {
                        ConnectionIntent::Idle => "idle",
                        ConnectionIntent::Auto => "auto",
                        ConnectionIntent::Scan => "scan",
                        ConnectionIntent::Manual(_) | ConnectionIntent::Direct(_) => "manual",
                    }
                    .into();
                    device.role = None;
                    device.firmware = None;
                    device.mtu = None;
                }
            })
            .await;
        if !control_changed
            && matches!(
                next,
                ConnectionIntent::Manual(_) | ConnectionIntent::Direct(_)
            )
        {
            intent_sender.send_replace(ConnectionIntent::Auto);
            intent.borrow_and_update();
        }
    }
}

async fn wait_or_control_change(intent: &mut watch::Receiver<ConnectionIntent>, delay: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(delay) => {}
        _ = intent.changed() => {}
    }
}

async fn discover(
    state: &SharedState,
    desired: &ConnectionIntent,
    preferred_device_id: Option<&str>,
    intent: &mut watch::Receiver<ConnectionIntent>,
) -> Result<Option<LanTarget>> {
    state
        .update_device(|device| {
            device.phase = "scanning".into();
            device.transport = "lan".into();
            device.connection_mode = match desired {
                ConnectionIntent::Auto => "auto",
                ConnectionIntent::Scan => "scan",
                ConnectionIntent::Manual(_) | ConnectionIntent::Direct(_) => "manual",
                ConnectionIntent::Idle => "idle",
            }
            .into();
            device.scan_observed = 0;
            device.scan_started_at = Some(unix_now());
            device.last_error = None;
        })
        .await;
    let daemon = ServiceDaemon::new().context("start mDNS daemon")?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .context("browse EPD-KIT mDNS service")?;
    let deadline = tokio::time::Instant::now() + DISCOVERY_TIMEOUT;
    let auto_select_at = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut auto_select_ready = false;
    let mut candidates = HashMap::<String, LanTarget>::new();
    let mut paired_candidates = HashMap::<String, bool>::new();
    loop {
        if auto_select_ready && matches!(desired, ConnectionIntent::Auto) {
            let selected = preferred_device_id
                .and_then(|id| candidates.get(id).cloned())
                .or_else(|| {
                    (candidates.len() == 1)
                        .then(|| candidates.values().next().cloned())
                        .flatten()
                });
            if selected.is_some() {
                let _ = daemon.stop_browse(SERVICE_TYPE);
                let _ = daemon.shutdown();
                return Ok(selected);
            }
        }
        let event = tokio::select! {
            event = receiver.recv_async() => event.ok(),
            _ = tokio::time::sleep_until(deadline) => None,
            changed = intent.changed() => {
                if changed.is_err() { None } else { None }
            }
            _ = tokio::time::sleep_until(auto_select_at), if !auto_select_ready => {
                auto_select_ready = true;
                continue;
            }
        };
        let Some(event) = event else {
            break;
        };
        if let ServiceEvent::ServiceResolved(service) = event
            && let Some(target) = resolved_target(&service)
        {
            let paired = credential_get(target.id.clone()).await?.is_some();
            paired_candidates.insert(target.id.clone(), paired);
            candidates.insert(target.id.clone(), target.clone());
            let mut visible = candidates.values().cloned().collect::<Vec<_>>();
            visible.sort_by(|left, right| left.name.cmp(&right.name));
            state
                .update_device(|device| {
                    device.scan_observed = candidates.len();
                    device.candidates = visible
                        .iter()
                        .map(|candidate| DeviceCandidate {
                            id: candidate.id.clone(),
                            name: candidate.name.clone(),
                            transport: "lan".into(),
                            endpoint: Some(candidate.endpoint.to_string()),
                            paired: paired_candidates.get(&candidate.id).copied(),
                            protocol_major: Some(4),
                            last_seen_at: unix_now(),
                            ..Default::default()
                        })
                        .collect();
                })
                .await;
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    if matches!(desired, ConnectionIntent::Scan) {
        return Ok(None);
    }
    if candidates.len() > 1 {
        bail!("multiple EPD-KIT LAN devices found; select one manually");
    }
    bail!("no EPD-KIT LAN device found through mDNS")
}

fn resolved_target(service: &ResolvedService) -> Option<LanTarget> {
    if service.get_property_val_str("proto") != Some("4") {
        return None;
    }
    let id = service.get_property_val_str("id")?.trim();
    if !is_hex_with_length(id, 12) {
        return None;
    }
    let address = preferred_ipv4(service.get_addresses_v4())?;
    let name = service
        .get_property_val_str("name")
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(id);
    Some(LanTarget {
        id: id.into(),
        name: name.into(),
        endpoint: SocketAddr::from((address, service.get_port())),
    })
}

fn preferred_ipv4(addresses: std::collections::HashSet<Ipv4Addr>) -> Option<Ipv4Addr> {
    let mut addresses = addresses.into_iter().collect::<Vec<_>>();
    addresses.sort_by_key(|address| (address.is_link_local(), address.octets()));
    addresses.into_iter().next()
}

async fn connect(state: &SharedState, attempt: &ConnectionAttempt) -> Result<LanSession> {
    let target = &attempt.target;
    state
        .update_device(|device| {
            if device.transport == "lan" {
                device.phase = "connecting".into();
                device.selected_device_id = Some(target.id.clone());
                device.name = Some(target.name.clone());
            }
        })
        .await;
    state
        .log(
            "info",
            "lan",
            format!("connecting to {} at {}", target.name, target.endpoint),
        )
        .await;
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target.endpoint))
        .await
        .map_err(|_| rpc::link_error("LAN connection timed out"))?
        .context("connect LAN device")?;
    stream.set_nodelay(true).context("enable TCP no-delay")?;
    let mut session = LanSession {
        stream,
        device_id: target.id.clone(),
        device_name: target.name.clone(),
        endpoint: target.endpoint,
        assembler: FrameAssembler::default(),
        next_id: AtomicU32::new(1),
        frame_bytes: 1024,
    };
    authenticate(
        &mut session,
        attempt.expected_device_id.as_deref(),
        attempt.bootstrap_secret.as_deref(),
    )
    .await?;
    Ok(session)
}

async fn authenticate(
    session: &mut LanSession,
    expected_device_id: Option<&str>,
    bootstrap_secret: Option<&str>,
) -> Result<()> {
    let greeting = tokio::time::timeout(CONNECT_TIMEOUT, read_line(&mut session.stream))
        .await
        .map_err(|_| rpc::link_error("LAN authentication greeting timed out"))??;
    let mut parts = greeting.split_whitespace();
    if parts.next() != Some("EPD4") {
        bail!("LAN device sent an invalid authentication greeting");
    }
    let device_id = parts
        .next()
        .ok_or_else(|| anyhow!("LAN authentication greeting has no device id"))?;
    let nonce = parts
        .next()
        .ok_or_else(|| anyhow!("LAN authentication greeting has no nonce"))?;
    validate_device_id(device_id).context("LAN device sent an invalid authentication device id")?;
    if !is_hex_with_length(nonce, 32) {
        bail!("LAN device sent an invalid authentication nonce");
    }
    if parts.next().is_some() {
        bail!("LAN device sent an invalid authentication greeting");
    }
    if expected_device_id.is_some_and(|expected| device_id != expected) {
        bail!("LAN device identity does not match the selected device");
    }
    let secret = match bootstrap_secret {
        Some(secret) => {
            validate_secret(secret)?;
            secret.to_owned()
        }
        None => credential_get(device_id.to_owned())
            .await?
            .ok_or_else(|| anyhow!("LAN device key is not configured"))?,
    };
    let digest = authentication_digest(&secret, device_id, nonce)?;
    session
        .stream
        .write_all(format!("AUTH {digest}\n").as_bytes())
        .await
        .context("send LAN authentication response")?;
    let response = tokio::time::timeout(CONNECT_TIMEOUT, read_line(&mut session.stream))
        .await
        .map_err(|_| rpc::link_error("LAN authentication response timed out"))??;
    let mut parts = response.split_whitespace();
    if parts.next() != Some("OK") {
        bail!("LAN device rejected authentication");
    }
    session.frame_bytes = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1024)
        .clamp(20, MAX_FRAME_BYTES);
    session.device_id = device_id.to_owned();
    if bootstrap_secret.is_some() {
        credential_set(session.device_id.clone(), secret).await?;
    }
    Ok(())
}

async fn read_line(stream: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::with_capacity(96);
    while bytes.len() < 256 {
        let byte = stream.read_u8().await.context("read LAN handshake")?;
        if byte == b'\n' {
            let line = String::from_utf8(bytes).context("decode LAN handshake")?;
            return Ok(line.trim_end_matches('\r').to_owned());
        }
        bytes.push(byte);
    }
    bail!("LAN handshake line is too long")
}

async fn prime_session(
    state: &SharedState,
    session: &mut LanSession,
    device_events: &broadcast::Sender<String>,
) -> Result<()> {
    let hello = rpc::transact(
        state,
        session,
        device_events,
        "system.hello",
        json!({}),
        REQUEST_TIMEOUT,
    )
    .await?;
    if hello.get("protocol_major").and_then(Value::as_u64) != Some(4) {
        bail!("device does not implement protocol v4");
    }
    if let Some(name) = hello.get("device_name").and_then(Value::as_str) {
        session.device_name = name.to_owned();
    }
    let _ = rpc::transact(
        state,
        session,
        device_events,
        "system.time.set",
        json!({
            "unix_seconds": unix_now(),
            "utc_offset_minutes": chrono_offset(),
        }),
        REQUEST_TIMEOUT,
    )
    .await?;
    let config = rpc::transact(
        state,
        session,
        device_events,
        "config.get",
        json!({}),
        REQUEST_TIMEOUT,
    )
    .await?;
    let mut capabilities = rpc::transact(
        state,
        session,
        device_events,
        "capabilities.get",
        json!({}),
        REQUEST_TIMEOUT,
    )
    .await?;
    protocol::hydrate_capabilities(&mut capabilities);
    let resources = rpc::transact(
        state,
        session,
        device_events,
        "resource.list",
        json!({}),
        REQUEST_TIMEOUT,
    )
    .await?;
    let bonds = rpc::transact(
        state,
        session,
        device_events,
        "security.bonds.list",
        json!({}),
        REQUEST_TIMEOUT,
    )
    .await
    .unwrap_or_default();
    let diagnostics = rpc::transact(
        state,
        session,
        device_events,
        "diagnostics.get",
        json!({}),
        REQUEST_TIMEOUT,
    )
    .await
    .ok();
    state
        .update_device(|device| {
            device.phase = "connected".into();
            device.transport = "lan".into();
            device.name = Some(session.device_name.clone());
            device.selected_device_id = Some(session.device_id.clone());
            device.role = hello.get("role").and_then(Value::as_str).map(str::to_owned);
            device.firmware = hello
                .get("firmware")
                .and_then(Value::as_str)
                .map(str::to_owned);
            device.mtu = Some(session.frame_bytes as u64);
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
            device.last_error = None;
        })
        .await;
    Ok(())
}

fn chrono_offset() -> i16 {
    use chrono::Offset;
    (chrono::Local::now().offset().fix().local_minus_utc() / 60) as i16
}

fn parse_endpoint(value: &str) -> Result<SocketAddr> {
    let value = value.trim();
    if value.is_empty() {
        bail!("LAN IP address is required");
    }
    let endpoint = match value.parse::<SocketAddr>() {
        Ok(endpoint) => endpoint,
        Err(_) => SocketAddr::new(
            value
                .parse::<IpAddr>()
                .context("LAN endpoint must be an IP address with an optional port")?,
            DEFAULT_PORT,
        ),
    };
    if endpoint.port() == 0 {
        bail!("LAN endpoint port must be between 1 and 65535");
    }
    let local = match endpoint.ip() {
        IpAddr::V4(address) => {
            address.is_private() || address.is_link_local() || address.is_loopback()
        }
        IpAddr::V6(address) => {
            address.is_unique_local() || address.is_unicast_link_local() || address.is_loopback()
        }
    };
    if !local {
        bail!("LAN endpoint must use a private, link-local, or loopback IP address");
    }
    Ok(endpoint)
}

fn validate_secret(secret: &str) -> Result<()> {
    if !is_hex_with_length(secret, 64) {
        bail!("LAN device key must contain exactly 64 hexadecimal characters");
    }
    Ok(())
}

fn validate_device_id(id: &str) -> Result<()> {
    if !is_hex_with_length(id, 12) {
        bail!("LAN device id must contain exactly 12 hexadecimal characters");
    }
    Ok(())
}

fn is_hex_with_length(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    validate_secret(value)?;
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("hex input is ASCII");
            u8::from_str_radix(text, 16).context("decode LAN device key")
        })
        .collect()
}

fn encode_hex(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn authentication_digest(secret: &str, device_id: &str, nonce: &str) -> Result<String> {
    let key = decode_hex(secret)?;
    let mut mac = HmacSha256::new_from_slice(&key).context("initialize LAN authentication")?;
    mac.update(format!("EPD4:{device_id}:{nonce}").as_bytes());
    Ok(encode_hex(&mac.finalize().into_bytes()))
}

fn credential_entry(id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SECRET_SERVICE, id).context("open LAN device credential")
}

async fn credential_get(id: String) -> Result<Option<String>> {
    tokio::task::spawn_blocking(move || match credential_entry(&id)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("read LAN device credential"),
    })
    .await
    .context("join LAN credential read")?
}

async fn credential_set(id: String, secret: String) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        credential_entry(&id)?
            .set_password(&secret)
            .context("save LAN device credential")
    })
    .await
    .context("join LAN credential write")?
}

fn target_path() -> Result<PathBuf> {
    let directory = dirs::config_dir()
        .ok_or_else(|| anyhow!("config directory unavailable"))?
        .join("epd-agent");
    std::fs::create_dir_all(&directory).context("create agent config directory")?;
    Ok(directory.join("lan-target.json"))
}

fn load_saved_target() -> Result<Option<LanTarget>> {
    let path = target_path()?;
    if !path.exists() {
        return Ok(None);
    }
    serde_json::from_slice(&std::fs::read(&path)?).context("decode saved LAN target")
}

fn save_target(target: &LanTarget) -> Result<()> {
    let path = target_path()?;
    write_private_file(&path, &serde_json::to_vec(target)?).context("write saved LAN target")
}

fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::{
        authentication_digest, decode_hex, parse_endpoint, validate_device_id, validate_secret,
    };

    #[test]
    fn device_key_must_be_32_hex_bytes() {
        assert!(validate_secret(&"ab".repeat(32)).is_ok());
        assert!(validate_secret(&"AB".repeat(32)).is_ok());
        assert!(validate_secret(&"ab".repeat(31)).is_err());
        assert!(validate_secret(&format!("{}zz", "ab".repeat(31))).is_err());
        assert_eq!(decode_hex(&"01".repeat(32)).unwrap(), vec![1; 32]);
        assert!(validate_device_id("A1B2C3D4E5F6").is_ok());
        assert!(validate_device_id("A1B2C3D4E5FG").is_err());
    }

    #[test]
    fn authentication_matches_fixed_hmac_vector() {
        let digest = authentication_digest(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "A1B2C3D4E5F6",
            "00112233445566778899aabbccddeeff",
        )
        .unwrap();
        assert_eq!(
            digest,
            "e3baf020fdcce5a13ed9836d3cc02de150db4cd7c4ad9974c14f17308c960004"
        );
    }

    #[test]
    fn direct_endpoint_defaults_to_protocol_port_and_stays_local() {
        assert_eq!(
            parse_endpoint("192.168.1.42").unwrap().to_string(),
            "192.168.1.42:38474"
        );
        assert_eq!(
            parse_endpoint("10.0.0.8:40000").unwrap().to_string(),
            "10.0.0.8:40000"
        );
        assert!(parse_endpoint("8.8.8.8").is_err());
        assert!(parse_endpoint("epd-kit.local").is_err());
    }
}

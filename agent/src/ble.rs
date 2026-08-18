use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use btleplug::{
    api::{
        Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
        WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot, watch};

use crate::{
    protocol::{self, FrameAssembler, MessageKind},
    state::{BleCandidate, PairingStatus, SharedState, unix_now},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(18);
const PAIRING_PIN_TIMEOUT: Duration = Duration::from_secs(90);
const INTERNAL_COMPANY_ID: u16 = 0xffff;

#[derive(Debug)]
struct BleLinkError(String);

impl fmt::Display for BleLinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BleLinkError {}

fn link_error(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(BleLinkError(message.into()))
}

fn is_link_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<BleLinkError>().is_some()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SavedTarget {
    id: String,
    name: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct AdvertisementState {
    protocol_major: Option<u8>,
    owned: Option<bool>,
    battery: Option<bool>,
    fast_advertising: Option<bool>,
    setup_mode: Option<bool>,
}

struct Command {
    op: String,
    args: Value,
    reply: oneshot::Sender<Result<Value>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ConnectionIntent {
    Auto,
    Scan,
    Manual(String),
    Idle,
}

#[derive(Clone)]
pub struct BleGateway {
    commands: mpsc::Sender<Command>,
    intent: watch::Sender<ConnectionIntent>,
    device_events: broadcast::Sender<String>,
    connected: Arc<AtomicBool>,
    state: Arc<SharedState>,
    pairing: PairingBroker,
}

#[derive(Clone)]
struct PairingBroker {
    state: Arc<SharedState>,
    pending: Arc<Mutex<Option<PendingPairing>>>,
}

struct PendingPairing {
    request_id: String,
    reply: Option<oneshot::Sender<String>>,
}

impl PairingBroker {
    fn new(state: Arc<SharedState>) -> Self {
        Self {
            state,
            pending: Arc::new(Mutex::new(None)),
        }
    }

    async fn request_pin(&self, device_name: &str) -> Result<String> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (reply, response) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.is_some() {
                bail!("another Windows BLE pairing request is already active");
            }
            *pending = Some(PendingPairing {
                request_id: request_id.clone(),
                reply: Some(reply),
            });
        }
        self.state
            .update_device(|device| {
                device.pairing = Some(PairingStatus {
                    request_id: request_id.clone(),
                    device_name: device_name.to_owned(),
                    expires_at: unix_now() + PAIRING_PIN_TIMEOUT.as_secs(),
                });
            })
            .await;
        self.state
            .log(
                "info",
                "ble.pairing",
                format!("waiting for the six-digit passkey shown on {device_name}"),
            )
            .await;

        let result = match tokio::time::timeout(PAIRING_PIN_TIMEOUT, response).await {
            Ok(Ok(pin)) => Ok(pin),
            Ok(Err(_)) => Err(anyhow!("Windows BLE pairing was cancelled")),
            Err(_) => Err(anyhow!("Windows BLE pairing passkey timed out")),
        };
        self.finish(&request_id).await;
        result
    }

    async fn submit_pin(&self, request_id: &str, pin: String) -> Result<()> {
        validate_pairing_pin(&pin)?;
        let sender = {
            let mut pending = self.pending.lock().await;
            let active = pending
                .as_mut()
                .filter(|active| active.request_id == request_id)
                .ok_or_else(|| anyhow!("Windows BLE pairing request is no longer active"))?;
            active
                .reply
                .take()
                .ok_or_else(|| anyhow!("Windows BLE pairing passkey was already submitted"))?
        };
        sender
            .send(pin)
            .map_err(|_| anyhow!("Windows BLE pairing request is no longer active"))?;
        self.state
            .log("info", "ble.pairing", "pairing passkey submitted")
            .await;
        Ok(())
    }

    async fn cancel(&self, request_id: &str) -> Result<()> {
        let cancelled = {
            let mut pending = self.pending.lock().await;
            if pending
                .as_ref()
                .is_some_and(|active| active.request_id == request_id)
            {
                pending.take();
                true
            } else {
                false
            }
        };
        if !cancelled {
            bail!("Windows BLE pairing request is no longer active");
        }
        self.clear_status(request_id).await;
        self.state
            .log("info", "ble.pairing", "pairing request cancelled")
            .await;
        Ok(())
    }

    async fn cancel_active(&self) {
        let request_id = self
            .pending
            .lock()
            .await
            .take()
            .map(|active| active.request_id);
        if let Some(request_id) = request_id {
            self.clear_status(&request_id).await;
        }
    }

    async fn finish(&self, request_id: &str) {
        let mut pending = self.pending.lock().await;
        if pending
            .as_ref()
            .is_some_and(|active| active.request_id == request_id)
        {
            pending.take();
        }
        drop(pending);
        self.clear_status(request_id).await;
    }

    async fn clear_status(&self, request_id: &str) {
        self.state
            .update_device(|device| {
                if device
                    .pairing
                    .as_ref()
                    .is_some_and(|pairing| pairing.request_id == request_id)
                {
                    device.pairing = None;
                }
            })
            .await;
    }
}

fn validate_pairing_pin(pin: &str) -> Result<()> {
    if pin.len() != 6 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("pairing passkey must contain exactly six digits");
    }
    Ok(())
}

impl BleGateway {
    pub fn spawn(state: Arc<SharedState>) -> Self {
        let (commands, receiver) = mpsc::channel(32);
        let (intent, intent_receiver) = watch::channel(ConnectionIntent::Auto);
        let (device_events, _) = broadcast::channel(32);
        let connected = Arc::new(AtomicBool::new(false));
        let pairing = PairingBroker::new(state.clone());
        let gateway = Self {
            commands,
            intent,
            device_events,
            connected,
            state: state.clone(),
            pairing: pairing.clone(),
        };
        tokio::spawn(supervisor(
            state,
            receiver,
            gateway.intent.clone(),
            intent_receiver,
            gateway.device_events.clone(),
            gateway.connected.clone(),
            pairing,
        ));
        gateway
    }

    pub async fn request(&self, op: impl Into<String>, args: Value) -> Result<Value> {
        if !self.connected.load(Ordering::Acquire) {
            bail!("BLE device is not connected");
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command {
                op: op.into(),
                args,
                reply,
            })
            .await
            .map_err(|_| anyhow!("BLE supervisor stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("BLE request was cancelled"))?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.device_events.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub async fn scan(&self) {
        self.pairing.cancel_active().await;
        self.state
            .update_device(|device| {
                device.phase = "scanning".into();
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
            .log("info", "ble", "manual BLE scan requested")
            .await;
    }

    pub async fn connect_device(&self, id: String) -> Result<()> {
        let id = id.trim().to_owned();
        if id.is_empty() {
            bail!("BLE device id is required");
        }
        self.pairing.cancel_active().await;
        let snapshot = self.state.snapshot().await;
        if !snapshot
            .device
            .candidates
            .iter()
            .any(|candidate| candidate.id == id)
        {
            bail!("selected EPD-KIT device is no longer in the scan results");
        }
        self.state
            .update_device(|device| {
                device.phase = "connecting".into();
                device.connection_mode = "manual".into();
                device.selected_device_id = Some(id.clone());
                device.last_error = None;
            })
            .await;
        self.intent
            .send_replace(ConnectionIntent::Manual(id.clone()));
        self.state
            .log(
                "info",
                "ble",
                format!("manual connection requested for {id}"),
            )
            .await;
        Ok(())
    }

    pub async fn auto_connect(&self) {
        self.pairing.cancel_active().await;
        self.state
            .update_device(|device| {
                device.phase = "scanning".into();
                device.connection_mode = "auto".into();
                device.selected_device_id = None;
                device.scan_started_at = Some(unix_now());
                device.last_error = None;
            })
            .await;
        self.intent.send_replace(ConnectionIntent::Auto);
        self.state
            .log("info", "ble", "automatic BLE connection enabled")
            .await;
    }

    pub async fn disconnect(&self) {
        self.pairing.cancel_active().await;
        let disconnecting = self.connected.load(Ordering::Acquire);
        self.state
            .update_device(|device| {
                device.phase = if disconnecting {
                    "disconnecting".into()
                } else {
                    "idle".into()
                };
                device.connection_mode = "idle".into();
                device.selected_device_id = None;
                device.last_error = None;
            })
            .await;
        self.intent.send_replace(ConnectionIntent::Idle);
        self.state
            .log("info", "ble", "BLE disconnect requested")
            .await;
    }

    pub async fn submit_pairing_pin(&self, request_id: &str, pin: String) -> Result<()> {
        self.pairing.submit_pin(request_id, pin).await
    }

    pub async fn cancel_pairing(&self, request_id: &str) -> Result<()> {
        self.pairing.cancel(request_id).await
    }
}

struct Session {
    peripheral: Peripheral,
    device_id: String,
    device_name: String,
    rx: Characteristic,
    notifications: std::pin::Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    assembler: FrameAssembler,
    next_id: AtomicU32,
    frame_bytes: usize,
    pairing_repair_allowed: bool,
}

struct ScanCleanup {
    adapter: Option<Adapter>,
}

impl ScanCleanup {
    fn new(adapter: Adapter) -> Self {
        Self {
            adapter: Some(adapter),
        }
    }

    async fn stop(&mut self) -> Result<()> {
        let Some(adapter) = self.adapter.take() else {
            return Ok(());
        };
        adapter.stop_scan().await.context("stop BLE scan")
    }
}

async fn supervisor(
    state: Arc<SharedState>,
    mut commands: mpsc::Receiver<Command>,
    intent_sender: watch::Sender<ConnectionIntent>,
    mut intent: watch::Receiver<ConnectionIntent>,
    device_events: broadcast::Sender<String>,
    connected: Arc<AtomicBool>,
    pairing: PairingBroker,
) {
    let mut backoff = 1u64;
    let mut repaired_pairing_for: Option<String> = None;
    let mut preferred_target = match load_saved_target() {
        Ok(target) => target,
        Err(error) => {
            state
                .log(
                    "warn",
                    "ble",
                    format!("cannot load saved BLE target: {error:#}"),
                )
                .await;
            None
        }
    };
    state
        .update_device(|device| {
            device.preferred_device_id = preferred_target.as_ref().map(|target| target.id.clone());
        })
        .await;
    state.log("info", "ble", "BLE supervisor started").await;
    loop {
        if state.paused().await {
            state
                .update_device(|device| device.phase = "paused".into())
                .await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        let desired = intent.borrow_and_update().clone();
        if desired == ConnectionIntent::Idle {
            state
                .update_device(|device| {
                    device.phase = "idle".into();
                    device.connection_mode = "idle".into();
                })
                .await;
            if intent.changed().await.is_err() {
                return;
            }
            continue;
        }

        let connect_result = connect(
            &state,
            &desired,
            preferred_target.as_ref(),
            &mut intent,
            &pairing,
        )
        .await;
        match connect_result {
            Ok(Some(mut session)) => {
                backoff = 1;
                let prime_result = tokio::select! {
                    result = prime_session(&state, &mut session, &device_events) => Some(result),
                    changed = intent.changed() => {
                        if changed.is_err() { return; }
                        None
                    }
                };
                let Some(prime_result) = prime_result else {
                    connected.store(false, Ordering::Release);
                    disconnect_peripheral(&session.peripheral).await;
                    continue;
                };
                if let Err(error) = prime_result {
                    connected.store(false, Ordering::Release);
                    state.log("error", "ble", format!("{error:#}")).await;
                    disconnect_peripheral(&session.peripheral).await;
                    let should_repair = session.pairing_repair_allowed
                        && is_link_error(&error)
                        && repaired_pairing_for.as_deref() != Some(&session.device_id);
                    if should_repair {
                        repaired_pairing_for = Some(session.device_id.clone());
                        match platform_repair_pairing(
                            &session.peripheral,
                            &session.device_name,
                            &pairing,
                        )
                        .await
                        {
                            Ok(true) => {
                                state
                                    .log(
                                        "info",
                                        "ble",
                                        "removed stale Windows pairing; system re-pair completed",
                                    )
                                    .await;
                            }
                            Ok(false) => {}
                            Err(repair_error) => {
                                state
                                    .log(
                                        "warn",
                                        "ble",
                                        format!(
                                            "cannot repair Windows BLE pairing: {repair_error:#}"
                                        ),
                                    )
                                    .await;
                            }
                        }
                    }
                    state
                        .update_device(|device| {
                            device.phase = "reconnecting".into();
                            device.role = None;
                            device.firmware = None;
                            device.mtu = None;
                            device.last_error = Some(format!("{error:#}"));
                        })
                        .await;
                    continue;
                }
                repaired_pairing_for = None;
                let target = SavedTarget {
                    id: session.device_id.clone(),
                    name: session.device_name.clone(),
                };
                if let Err(error) = save_target(&target) {
                    state
                        .log("warn", "ble", format!("cannot save BLE target: {error:#}"))
                        .await;
                } else {
                    preferred_target = Some(target.clone());
                    state
                        .update_device(|device| {
                            device.preferred_device_id = Some(target.id.clone());
                        })
                        .await;
                }
                connected.store(true, Ordering::Release);
                let _ = device_events.send("ble.connected".into());
                state
                    .log(
                        "info",
                        "ble",
                        format!(
                            "BLE v4 session ready; frame payload {} bytes",
                            session.frame_bytes
                        ),
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
                            let result = transact(&state, &mut session, &device_events, &command.op, command.args).await;
                            let link_failed = result.as_ref().err().is_some_and(is_link_error);
                            let _ = command.reply.send(result);
                            if link_failed {
                                state.log("warn", "ble", "BLE RPC detected a stale session").await;
                                break;
                            }
                        }
                        notification = session.notifications.next() => {
                            let Some(notification) = notification else { break };
                            if let Err(error) = handle_notification(&state, &mut session.assembler, &device_events, &notification.value).await {
                                state.log("warn", "ble", error.to_string()).await;
                            }
                        }
                        _ = health.tick() => {
                            let transport_connected = tokio::time::timeout(
                                HEARTBEAT_TIMEOUT,
                                session.peripheral.is_connected(),
                            ).await.ok().and_then(|result| result.ok()).unwrap_or(false);
                            if !transport_connected { break; }
                            if state.paused().await { break; }
                            if let Err(error) = transact_with_timeout(
                                &state,
                                &mut session,
                                &device_events,
                                "system.status",
                                json!({}),
                                HEARTBEAT_TIMEOUT,
                            ).await {
                                state.log("warn", "ble", format!("BLE heartbeat failed: {error:#}")).await;
                                break;
                            }
                        }
                        changed = intent.changed() => {
                            control_changed = true;
                            if changed.is_err() {
                                disconnect_peripheral(&session.peripheral).await;
                                connected.store(false, Ordering::Release);
                                return;
                            }
                            break;
                        }
                    }
                }
                connected.store(false, Ordering::Release);
                let mut next = intent.borrow().clone();
                if !control_changed && matches!(next, ConnectionIntent::Manual(_)) {
                    intent_sender.send_replace(ConnectionIntent::Auto);
                    next = ConnectionIntent::Auto;
                    intent.borrow_and_update();
                }
                state
                    .update_device(|device| {
                        device.phase = match next {
                            ConnectionIntent::Idle => "idle",
                            ConnectionIntent::Scan
                            | ConnectionIntent::Auto
                            | ConnectionIntent::Manual(_) => "reconnecting",
                        }
                        .into();
                        device.connection_mode = match next {
                            ConnectionIntent::Idle => "idle",
                            ConnectionIntent::Scan => "scan",
                            ConnectionIntent::Auto => "auto",
                            ConnectionIntent::Manual(_) => "manual",
                        }
                        .into();
                        device.role = None;
                        device.firmware = None;
                        device.mtu = None;
                    })
                    .await;
                let _ = device_events.send("ble.disconnected".into());
                disconnect_peripheral(&session.peripheral).await;
                if control_changed {
                    state
                        .log("info", "ble", "BLE session changed by user control")
                        .await;
                } else {
                    state
                        .log("warn", "ble", "BLE session disconnected; reconnecting")
                        .await;
                }
            }
            Ok(None) => {
                if intent.borrow().clone() != desired {
                    intent.borrow_and_update();
                    backoff = 1;
                    continue;
                }
                state
                    .update_device(|device| {
                        device.phase = "idle".into();
                        device.connection_mode = "idle".into();
                    })
                    .await;
                state.log("info", "ble", "manual BLE scan completed").await;
                if intent.changed().await.is_err() {
                    return;
                }
            }
            Err(error) => {
                connected.store(false, Ordering::Release);
                state
                    .update_device(|device| {
                        device.phase = "unavailable".into();
                        device.last_error = Some(format!("{error:#}"));
                    })
                    .await;
                state
                    .log("warn", "ble", format!("{error:#}; retrying in {backoff}s"))
                    .await;
                let changed = tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(backoff)) => false,
                    changed = intent.changed() => {
                        if changed.is_err() { return; }
                        true
                    }
                };
                if changed {
                    backoff = 1;
                    continue;
                }
                backoff = (backoff * 2).min(30);
            }
        }
    }
}

async fn disconnect_peripheral(peripheral: &Peripheral) {
    let connected = tokio::time::timeout(Duration::from_secs(2), peripheral.is_connected())
        .await
        .ok()
        .and_then(|result| result.ok())
        .unwrap_or(true);
    if !connected {
        return;
    }
    let _ = tokio::time::timeout(Duration::from_secs(3), peripheral.disconnect()).await;
}

async fn connect(
    state: &SharedState,
    desired: &ConnectionIntent,
    preferred_target: Option<&SavedTarget>,
    intent: &mut watch::Receiver<ConnectionIntent>,
    pairing: &PairingBroker,
) -> Result<Option<Session>> {
    let manager = Manager::new()
        .await
        .context("initialize Bluetooth manager")?;
    let adapters = manager
        .adapters()
        .await
        .context(if cfg!(target_os = "macos") {
            "Bluetooth access unavailable; allow EPD Agent in System Settings > Privacy & Security > Bluetooth"
        } else {
            "list Bluetooth adapters"
        })?;
    state
        .log(
            "info",
            "ble",
            format!("found {} Bluetooth adapter(s)", adapters.len()),
        )
        .await;
    let adapter = adapters
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapter found"))?;
    let adapter_name = adapter
        .adapter_info()
        .await
        .unwrap_or_else(|_| "unknown adapter".into());
    state
        .log("info", "ble", format!("using adapter {adapter_name}"))
        .await;
    let Some((peripheral, advertisement)) =
        discover(state, &adapter, desired, preferred_target, intent).await?
    else {
        return Ok(None);
    };
    let session_result = tokio::select! {
      result = async {
        let properties = peripheral.properties().await?.unwrap_or_default();
        let name = properties
            .local_name
            .clone()
            .unwrap_or_else(|| peripheral.id().to_string());
        state
            .update_device(|device| {
                device.phase = "connecting".into();
                device.name = Some(name.clone());
                device.last_error = None;
            })
            .await;
        state
            .log("info", "ble", format!("connecting to {name}"))
            .await;
        let reset_stale_pairing = advertisement.owned == Some(false);
        if platform_pairing_hint(&peripheral, &name, reset_stale_pairing, pairing).await? {
            state
                .log(
                    "info",
                    "ble",
                    "device is unowned; replaced stale Windows pairing before connecting",
                )
                .await;
        }
        if !peripheral.is_connected().await? {
            let connect_result = tokio::time::timeout(CONNECT_TIMEOUT, peripheral.connect())
                .await
                .map_err(|_| {
                    anyhow!(
                        "BLE connection timed out after {}s",
                        CONNECT_TIMEOUT.as_secs()
                    )
                })?;
            if let Err(error) = connect_result {
                let connected_after_error = peripheral.is_connected().await.unwrap_or(false);
                state
                    .log(
                        "warn",
                        "ble",
                        format!(
                            "BLE connect failed; connected_after_error={connected_after_error}; cause={error}"
                        ),
                    )
                    .await;
                if !connected_after_error {
                    return Err(error).context("connect BLE device");
                }
            }
        }
        state
            .update_device(|device| device.phase = "handshaking".into())
            .await;
        state
            .log(
                "info",
                "ble",
                "BLE transport connected; discovering services",
            )
            .await;
        peripheral
            .discover_services()
            .await
            .context("discover BLE services")?;
        let characteristics = peripheral.characteristics();
        state
            .log(
                "info",
                "ble",
                format!(
                    "discovered {} GATT characteristic(s)",
                    characteristics.len()
                ),
            )
            .await;
        let rx = characteristics
            .iter()
            .find(|item| item.uuid == protocol::RX_UUID)
            .cloned()
            .ok_or_else(|| anyhow!("BLE v4 RX characteristic not found"))?;
        let tx = characteristics
            .iter()
            .find(|item| item.uuid == protocol::TX_UUID)
            .cloned()
            .ok_or_else(|| anyhow!("BLE v4 TX characteristic not found"))?;
        peripheral
            .subscribe(&tx)
            .await
            .context("subscribe BLE v4 indications")?;
        state
            .log("info", "ble", "subscribed to BLE v4 indications")
            .await;
        let notifications = peripheral
            .notifications()
            .await
            .context("open BLE notification stream")?;
        Ok(Session {
            device_id: peripheral.id().to_string(),
            device_name: name,
            peripheral: peripheral.clone(),
            rx,
            notifications,
            assembler: FrameAssembler::default(),
            next_id: AtomicU32::new(1),
            frame_bytes: 20,
            pairing_repair_allowed: advertisement.owned == Some(false)
                || advertisement.setup_mode.unwrap_or(false),
        })
      } => Some(result),
      changed = intent.changed() => {
          if changed.is_err() {
              disconnect_peripheral(&peripheral).await;
              return Ok(None);
          }
          None
      }
    };
    let Some(session_result) = session_result else {
        disconnect_peripheral(&peripheral).await;
        return Ok(None);
    };
    if session_result.is_err() {
        disconnect_peripheral(&peripheral).await;
    }
    session_result.map(Some)
}

async fn discover(
    state: &SharedState,
    adapter: &Adapter,
    desired: &ConnectionIntent,
    preferred_target: Option<&SavedTarget>,
    intent: &mut watch::Receiver<ConnectionIntent>,
) -> Result<Option<(Peripheral, AdvertisementState)>> {
    let connection_mode = match desired {
        ConnectionIntent::Auto => "auto",
        ConnectionIntent::Scan => "scan",
        ConnectionIntent::Manual(_) => "manual",
        ConnectionIntent::Idle => "idle",
    };
    state
        .update_device(|device| {
            device.phase = "scanning".into();
            device.connection_mode = connection_mode.into();
            if matches!(desired, ConnectionIntent::Auto) {
                device.selected_device_id = preferred_target.map(|target| target.id.clone());
            } else if !matches!(desired, ConnectionIntent::Manual(_)) {
                device.selected_device_id = None;
            }
            device.scan_observed = 0;
            device.scan_started_at = Some(unix_now());
        })
        .await;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("start BLE scan")?;
    let mut scan_cleanup = ScanCleanup::new(adapter.clone());
    state
        .log(
            "info",
            "ble",
            format!("unfiltered BLE scan active for up to 15s; mode={connection_mode}"),
        )
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let auto_select_at = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut seen = HashSet::new();
    let mut observed = HashSet::new();
    let mut candidates = HashMap::<String, BleCandidate>::new();
    let mut candidate_peripherals = HashMap::<String, Peripheral>::new();
    let mut candidate_advertisements = HashMap::<String, AdvertisementState>::new();
    loop {
        let peripherals = match adapter.peripherals().await {
            Ok(peripherals) => peripherals,
            Err(error) => {
                let _ = scan_cleanup.stop().await;
                return Err(error).context("list BLE scan results");
            }
        };
        for peripheral in peripherals {
            let properties = match peripheral.properties().await {
                Ok(properties) => properties,
                Err(error) => {
                    let _ = scan_cleanup.stop().await;
                    return Err(error).context("read BLE advertisement");
                }
            };
            let Some(properties) = properties else {
                continue;
            };
            let matches_service = properties.services.contains(&protocol::SERVICE_UUID);
            let advertisement = advertisement_state(&properties.manufacturer_data);
            let matches_name = properties
                .local_name
                .as_deref()
                .is_some_and(|name| name.starts_with("EPD-KIT-"))
                && advertisement.protocol_major.is_none_or(|major| major == 4);
            let identity = peripheral.id().to_string();
            observed.insert(identity.clone());
            if matches_service || matches_name {
                let name = properties.local_name.as_deref().unwrap_or("unnamed");
                if seen.insert(identity.clone()) {
                    let services = if properties.services.is_empty() {
                        "none".into()
                    } else {
                        properties
                            .services
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    let matched_by = match (matches_name, matches_service) {
                        (true, true) => "name+service",
                        (true, false) => "name",
                        (false, true) => "service",
                        (false, false) => unreachable!(),
                    };
                    state.log("info", "ble.scan", format!(
                        "candidate name={name} id={identity} rssi={} services={services} match={matched_by}",
                        properties.rssi.map_or_else(|| "unknown".into(), |value| format!("{value}dBm")),
                    )).await;
                }
                candidates.insert(
                    identity.clone(),
                    BleCandidate {
                        id: identity.clone(),
                        name: name.into(),
                        rssi: properties.rssi,
                        advertises_service: matches_service,
                        protocol_major: advertisement.protocol_major,
                        owned: advertisement.owned,
                        battery: advertisement.battery,
                        fast_advertising: advertisement.fast_advertising,
                        last_seen_at: unix_now(),
                    },
                );
                candidate_advertisements.insert(identity.clone(), advertisement);
                candidate_peripherals.insert(identity, peripheral);
            }
        }

        let mut visible_candidates = candidates.values().cloned().collect::<Vec<_>>();
        visible_candidates.sort_by(|left, right| {
            right
                .rssi
                .unwrap_or(i16::MIN)
                .cmp(&left.rssi.unwrap_or(i16::MIN))
                .then_with(|| left.name.cmp(&right.name))
        });
        state
            .update_device(|device| {
                device.candidates = visible_candidates;
                device.scan_observed = observed.len();
            })
            .await;

        let selected_id = match desired {
            ConnectionIntent::Manual(target) => candidate_peripherals
                .contains_key(target)
                .then(|| target.clone())
                .or_else(|| {
                    let saved = preferred_target.filter(|saved| saved.id == *target)?;
                    let mut matches = candidates.values().filter(|candidate| {
                        candidate.name == saved.name
                            && (candidate.advertises_service || candidate.protocol_major == Some(4))
                    });
                    let candidate = matches.next();
                    candidate
                        .filter(|_| matches.next().is_none())
                        .map(|candidate| candidate.id.clone())
                }),
            ConnectionIntent::Auto if tokio::time::Instant::now() >= auto_select_at => {
                if let Some(target) = preferred_target {
                    candidates
                        .values()
                        .find(|candidate| candidate.id == target.id)
                        .or_else(|| {
                            let mut matches = candidates.values().filter(|candidate| {
                                candidate.name == target.name
                                    && (candidate.advertises_service
                                        || candidate.protocol_major == Some(4))
                            });
                            let candidate = matches.next();
                            candidate.filter(|_| matches.next().is_none())
                        })
                        .map(|candidate| candidate.id.clone())
                } else if candidates.len() == 1 {
                    candidates.keys().next().cloned()
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(selected_id) = selected_id {
            scan_cleanup.stop().await?;
            let selected_name = candidates
                .get(&selected_id)
                .map(|candidate| candidate.name.clone());
            state
                .update_device(|device| {
                    device.phase = "connecting".into();
                    device.selected_device_id = Some(selected_id.clone());
                    if selected_name.is_some() {
                        device.name = selected_name;
                    }
                })
                .await;
            state
                .log(
                    "info",
                    "ble",
                    format!("EPD-KIT candidate selected id={selected_id}"),
                )
                .await;
            return Ok(candidate_peripherals
                .get(&selected_id)
                .cloned()
                .map(|peripheral| {
                    let advertisement = candidate_advertisements
                        .get(&selected_id)
                        .copied()
                        .unwrap_or_default();
                    (peripheral, advertisement)
                }));
        }

        if tokio::time::Instant::now() >= deadline {
            scan_cleanup.stop().await?;
            if *desired == ConnectionIntent::Scan {
                return Ok(None);
            }
            if let ConnectionIntent::Manual(target) = desired {
                bail!(
                    "selected EPD-KIT device {target} was not found after 15s ({} peripherals observed)",
                    observed.len()
                );
            }
            if let Some(target) = preferred_target {
                bail!(
                    "saved EPD-KIT device {} was not found after 15s ({} peripherals observed)",
                    target.name,
                    observed.len()
                );
            }
            if candidates.len() > 1 {
                bail!(
                    "multiple EPD-KIT devices found; select one manually before enabling automatic connection"
                );
            }
            bail!(
                "no EPD-KIT BLE v4 device found after 15s ({} peripherals observed)",
                observed.len()
            );
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(400)) => {}
            changed = intent.changed() => {
                scan_cleanup.stop().await?;
                if changed.is_err() {
                    return Ok(None);
                }
                return Ok(None);
            }
        }
    }
}

async fn prime_session(
    state: &SharedState,
    session: &mut Session,
    device_events: &broadcast::Sender<String>,
) -> Result<()> {
    state
        .log("info", "ble", "initializing BLE v4 session")
        .await;
    let hello = transact(state, session, device_events, "system.hello", json!({})).await?;
    if hello.get("protocol_major").and_then(Value::as_u64) != Some(4) {
        bail!("device does not implement BLE protocol v4");
    }
    if let Some(name) = hello.get("device_name").and_then(Value::as_str) {
        session.device_name = name.to_owned();
    }
    session.frame_bytes = hello
        .get("mtu")
        .and_then(Value::as_u64)
        .map(|mtu| mtu.saturating_sub(3).clamp(20, 244) as usize)
        .unwrap_or(20);
    state
        .log(
            "info",
            "ble",
            format!(
                "session hello complete; role={} firmware={} mtu={} frame_payload={}",
                hello
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                hello
                    .get("firmware")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                hello.get("mtu").and_then(Value::as_u64).unwrap_or(23),
                session.frame_bytes,
            ),
        )
        .await;
    let now = chrono_offset();
    let _ = transact(
        state,
        session,
        device_events,
        "system.time.set",
        json!({
            "unix_seconds": unix_now(),
            "utc_offset_minutes": now,
        }),
    )
    .await?;
    let config = transact(state, session, device_events, "config.get", json!({})).await?;
    let capabilities =
        transact(state, session, device_events, "capabilities.get", json!({})).await?;
    let resources = transact(state, session, device_events, "resource.list", json!({})).await?;
    let bonds = transact(
        state,
        session,
        device_events,
        "security.bonds.list",
        json!({}),
    )
    .await
    .unwrap_or_default();
    let diagnostics = transact(state, session, device_events, "diagnostics.get", json!({}))
        .await
        .ok();
    state
        .update_device(|device| {
            device.phase = "connected".into();
            device.name = Some(session.device_name.clone());
            device.role = hello.get("role").and_then(Value::as_str).map(str::to_owned);
            device.firmware = hello
                .get("firmware")
                .and_then(Value::as_str)
                .map(str::to_owned);
            device.mtu = hello.get("mtu").and_then(Value::as_u64);
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
    state.log("info", "ble", "device state primed").await;
    Ok(())
}

fn advertisement_state(manufacturer_data: &HashMap<u16, Vec<u8>>) -> AdvertisementState {
    let Some(data) = manufacturer_data.get(&INTERNAL_COMPANY_ID) else {
        return AdvertisementState::default();
    };
    let Some((&protocol_major, flags)) = data.split_first() else {
        return AdvertisementState::default();
    };
    let Some(&flags) = flags.first() else {
        return AdvertisementState {
            protocol_major: Some(protocol_major),
            ..AdvertisementState::default()
        };
    };
    AdvertisementState {
        protocol_major: Some(protocol_major),
        owned: Some(flags & 0x01 != 0),
        battery: Some(flags & 0x02 != 0),
        fast_advertising: Some(flags & 0x08 != 0),
        setup_mode: Some(flags & 0x10 != 0),
    }
}

fn target_path() -> Result<PathBuf> {
    let directory = dirs::config_dir()
        .ok_or_else(|| anyhow!("config directory unavailable"))?
        .join("epd-agent");
    std::fs::create_dir_all(&directory).context("create agent config directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory.join("ble-target.json"))
}

fn load_saved_target() -> Result<Option<SavedTarget>> {
    let path = target_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let target =
        serde_json::from_slice(&std::fs::read(&path)?).context("decode saved BLE target")?;
    Ok(Some(target))
}

fn save_target(target: &SavedTarget) -> Result<()> {
    let path = target_path()?;
    write_private_file(&path, &serde_json::to_vec(target)?).context("write saved BLE target")
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

async fn transact(
    state: &SharedState,
    session: &mut Session,
    device_events: &broadcast::Sender<String>,
    op: &str,
    args: Value,
) -> Result<Value> {
    transact_with_timeout(state, session, device_events, op, args, REQUEST_TIMEOUT).await
}

async fn transact_with_timeout(
    state: &SharedState,
    session: &mut Session,
    device_events: &broadcast::Sender<String>,
    op: &str,
    args: Value,
    timeout: Duration,
) -> Result<Value> {
    let id = session.next_id.fetch_add(1, Ordering::Relaxed);
    let frames = protocol::encode_request(id, op, args, session.frame_bytes)?;
    let frame_count = frames.len();
    let encoded_bytes = frames.iter().map(Vec::len).sum::<usize>();
    let started = Instant::now();
    state
        .log(
            "debug",
            "ble.rpc",
            format!("request id={id} op={op} frames={frame_count} bytes={encoded_bytes}"),
        )
        .await;
    for frame in frames {
        tokio::time::timeout(
            timeout,
            session
                .peripheral
                .write(&session.rx, &frame, WriteType::WithResponse),
        )
        .await
        .map_err(|_| link_error(format!("BLE write timed out for {op}")))?
        .map_err(|error| link_error(format!("write BLE request {op}: {error}")))?;
    }
    let response = tokio::time::timeout(timeout, async {
        loop {
            let notification = session
                .notifications
                .next()
                .await
                .ok_or_else(|| anyhow!("BLE notification stream closed"))?;
            let Some(message) = session.assembler.feed(&notification.value)? else {
                continue;
            };
            match message.kind {
                MessageKind::Response if message.id == id => {
                    return Ok::<protocol::Response, anyhow::Error>(protocol::decode_response(
                        &message.payload,
                    )?);
                }
                MessageKind::Event => {
                    dispatch_event(state, device_events, &message.payload).await?
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| link_error(format!("BLE response timed out for {op}")))?
    .map_err(|error| link_error(format!("BLE response failed for {op}: {error:#}")))?;
    if response.ok {
        state
            .log(
                "debug",
                "ble.rpc",
                format!(
                    "response id={id} op={op} elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
            )
            .await;
        return Ok(response.result);
    }
    let error = response
        .error
        .ok_or_else(|| anyhow!("BLE request failed without error payload"))?;
    state
        .log(
            "warn",
            "ble.rpc",
            format!(
                "response id={id} op={op} code={} elapsed_ms={}",
                error.code,
                started.elapsed().as_millis()
            ),
        )
        .await;
    Err(anyhow!("{}: {}", error.code, error.message))
}

async fn handle_notification(
    state: &SharedState,
    assembler: &mut FrameAssembler,
    device_events: &broadcast::Sender<String>,
    value: &[u8],
) -> Result<()> {
    let Some(message) = assembler.feed(value)? else {
        return Ok(());
    };
    if message.kind == MessageKind::Event {
        dispatch_event(state, device_events, &message.payload).await?;
    }
    Ok(())
}

async fn dispatch_event(
    state: &SharedState,
    device_events: &broadcast::Sender<String>,
    payload: &[u8],
) -> Result<()> {
    let event = protocol::decode_event(payload)?;
    state
        .log("info", "device", format!("{} {}", event.name, event.data))
        .await;
    let _ = device_events.send(event.name);
    Ok(())
}

fn chrono_offset() -> i16 {
    use chrono::Offset;
    (chrono::Local::now().offset().fix().local_minus_utc() / 60) as i16
}

#[cfg(target_os = "windows")]
fn windows_bluetooth_address(peripheral: &Peripheral) -> Option<u64> {
    let id = peripheral.id().to_string();
    let mut address = 0u64;
    let mut count = 0usize;
    for part in id.split(':') {
        if part.len() != 2 {
            return None;
        }
        let octet = u8::from_str_radix(part, 16).ok()?;
        address = (address << 8) | u64::from(octet);
        count += 1;
    }
    (count == 6).then_some(address)
}

#[cfg(target_os = "windows")]
async fn windows_device_information(
    peripheral: &Peripheral,
    name: &str,
) -> Result<Option<windows::Devices::Enumeration::DeviceInformation>> {
    use windows::Devices::{Bluetooth::BluetoothLEDevice, Enumeration::DeviceInformation};

    if let Some(address) = windows_bluetooth_address(peripheral)
        && let Ok(operation) = BluetoothLEDevice::FromBluetoothAddressAsync(address)
        && let Ok(device) = operation.await
        && let Ok(information) = device.DeviceInformation()
    {
        return Ok(Some(information));
    }

    let selector = BluetoothLEDevice::GetDeviceSelector()?;
    let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.await?;
    for device in devices {
        if device.Name()?.to_string() == name {
            return Ok(Some(device));
        }
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
async fn windows_pair(
    device: &windows::Devices::Enumeration::DeviceInformation,
    name: &str,
    context: &str,
    pairing_broker: &PairingBroker,
) -> Result<()> {
    use windows::{
        Devices::Enumeration::{
            DeviceInformationCustomPairing, DevicePairingKinds, DevicePairingProtectionLevel,
            DevicePairingRequestedEventArgs, DevicePairingResultStatus,
        },
        Foundation::TypedEventHandler,
        core::{HSTRING, Ref},
    };

    let pairing = device.Pairing()?;
    let custom = pairing.Custom()?;
    let runtime = tokio::runtime::Handle::current();
    let request_broker = pairing_broker.clone();
    let request_name = name.to_owned();
    let token = {
        let handler: TypedEventHandler<
            DeviceInformationCustomPairing,
            DevicePairingRequestedEventArgs,
        > = TypedEventHandler::new(move |_sender, args: Ref<DevicePairingRequestedEventArgs>| {
            let args = args.ok()?;
            match args.PairingKind()? {
                DevicePairingKinds::ProvidePin => {
                    let args = args.clone();
                    let deferral = args.GetDeferral()?;
                    let broker = request_broker.clone();
                    let name = request_name.clone();
                    runtime.spawn(async move {
                        match broker.request_pin(&name).await {
                            Ok(pin) => {
                                if let Err(error) = args.AcceptWithPin(&HSTRING::from(pin)) {
                                    broker
                                        .state
                                        .log(
                                            "error",
                                            "ble.pairing",
                                            format!("cannot submit passkey to Windows: {error}"),
                                        )
                                        .await;
                                }
                            }
                            Err(error) => {
                                broker
                                    .state
                                    .log("warn", "ble.pairing", format!("{error:#}"))
                                    .await;
                            }
                        }
                        if let Err(error) = deferral.Complete() {
                            broker
                                .state
                                .log(
                                    "warn",
                                    "ble.pairing",
                                    format!("cannot complete Windows pairing request: {error}"),
                                )
                                .await;
                        }
                    });
                    Ok(())
                }
                kind => {
                    tracing::warn!(
                        scope = "ble.pairing",
                        "Windows requested unsupported pairing ceremony {kind:?}"
                    );
                    Ok(())
                }
            }
        });
        custom.PairingRequested(&handler)?
    };
    let result = match custom.PairWithProtectionLevelAsync(
        DevicePairingKinds::ProvidePin,
        DevicePairingProtectionLevel::EncryptionAndAuthentication,
    ) {
        Ok(operation) => operation.await,
        Err(error) => Err(error),
    };
    let remove_result = custom.RemovePairingRequested(token);
    pairing_broker.cancel_active().await;
    let result = result?;
    remove_result?;
    match result.Status()? {
        DevicePairingResultStatus::Paired | DevicePairingResultStatus::AlreadyPaired => Ok(()),
        status => bail!("Windows BLE {context} failed: {status:?}"),
    }
}

#[cfg(target_os = "windows")]
async fn platform_pairing_hint(
    peripheral: &Peripheral,
    name: &str,
    reset_stale_pairing: bool,
    pairing_broker: &PairingBroker,
) -> Result<bool> {
    use windows::Devices::Enumeration::DeviceUnpairingResultStatus;

    let Some(device) = windows_device_information(peripheral, name).await? else {
        return Ok(false);
    };
    let pairing = device.Pairing()?;
    if pairing.IsPaired()? && !reset_stale_pairing {
        return Ok(false);
    }
    if pairing.IsPaired()? {
        let unpair = pairing.UnpairAsync()?.await?;
        match unpair.Status()? {
            DeviceUnpairingResultStatus::Unpaired
            | DeviceUnpairingResultStatus::AlreadyUnpaired => {}
            status => bail!("Windows BLE stale-pair removal failed: {status:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let device = windows_device_information(peripheral, name)
            .await?
            .ok_or_else(|| anyhow!("Windows BLE device disappeared after stale-pair removal"))?;
        windows_pair(&device, name, "re-pair", pairing_broker).await?;
        return Ok(true);
    }
    windows_pair(&device, name, "pairing", pairing_broker).await?;
    Ok(false)
}

#[cfg(not(target_os = "windows"))]
async fn platform_pairing_hint(
    _peripheral: &Peripheral,
    _name: &str,
    _reset_stale_pairing: bool,
    _pairing_broker: &PairingBroker,
) -> Result<bool> {
    Ok(false)
}

#[cfg(target_os = "windows")]
async fn platform_repair_pairing(
    _peripheral: &Peripheral,
    name: &str,
    pairing_broker: &PairingBroker,
) -> Result<bool> {
    use windows::Devices::Enumeration::DeviceUnpairingResultStatus;

    let Some(device) = windows_device_information(_peripheral, name).await? else {
        return Ok(false);
    };
    let pairing = device.Pairing()?;
    if pairing.IsPaired()? {
        let unpair = pairing.UnpairAsync()?.await?;
        match unpair.Status()? {
            DeviceUnpairingResultStatus::Unpaired
            | DeviceUnpairingResultStatus::AlreadyUnpaired => {}
            status => bail!("Windows BLE unpair failed: {status:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let device = windows_device_information(_peripheral, name)
        .await?
        .ok_or_else(|| anyhow!("Windows BLE device disappeared before re-pair"))?;
    windows_pair(&device, name, "re-pair", pairing_broker).await?;
    Ok(true)
}

#[cfg(not(target_os = "windows"))]
async fn platform_repair_pairing(
    _peripheral: &Peripheral,
    _name: &str,
    _pairing_broker: &PairingBroker,
) -> Result<bool> {
    Ok(false)
}

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::{ble::BleGateway, lan::LanGateway, state::SharedState};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    #[default]
    Ble,
    Lan,
}

impl TransportKind {
    const fn code(self) -> u8 {
        match self {
            Self::Ble => 0,
            Self::Lan => 1,
        }
    }

    const fn from_code(value: u8) -> Self {
        match value {
            1 => Self::Lan,
            _ => Self::Ble,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ble => "ble",
            Self::Lan => "lan",
        }
    }
}

/// Transport-neutral device control surface used by the Agent business layer.
///
/// BLE and LAN adapters stay behind this boundary so publishers, producers,
/// and the local web API do not depend on connection-specific details.
#[derive(Clone)]
pub struct DeviceGateway {
    ble: BleGateway,
    lan: LanGateway,
    active: Arc<AtomicU8>,
    device_events: broadcast::Sender<String>,
    state: Arc<SharedState>,
}

impl DeviceGateway {
    pub fn spawn(state: Arc<SharedState>) -> Self {
        let ble = BleGateway::spawn(state.clone());
        let lan = LanGateway::spawn(state.clone());
        let (device_events, _) = broadcast::channel(64);
        relay_events(ble.subscribe(), device_events.clone());
        relay_events(lan.subscribe(), device_events.clone());
        Self {
            ble,
            lan,
            active: Arc::new(AtomicU8::new(TransportKind::Ble.code())),
            device_events,
            state,
        }
    }

    pub async fn request(&self, op: impl Into<String>, args: Value) -> Result<Value> {
        let op = op.into();
        match self.active_transport() {
            TransportKind::Ble => self.ble.request(op, args).await,
            TransportKind::Lan => self.lan.request(op, args).await,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.device_events.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        match self.active_transport() {
            TransportKind::Ble => self.ble.is_connected(),
            TransportKind::Lan => self.lan.is_connected(),
        }
    }

    pub fn active_transport(&self) -> TransportKind {
        TransportKind::from_code(self.active.load(Ordering::Acquire))
    }

    pub async fn scan(&self, transport: TransportKind) {
        self.activate(transport).await;
        match transport {
            TransportKind::Ble => self.ble.scan().await,
            TransportKind::Lan => self.lan.scan().await,
        }
    }

    pub async fn connect_device(
        &self,
        transport: TransportKind,
        id: String,
        secret: Option<String>,
    ) -> Result<()> {
        self.activate(transport).await;
        match transport {
            TransportKind::Ble => {
                if secret.is_some() {
                    bail!("BLE connection does not accept a LAN device key");
                }
                self.ble.connect_device(id).await
            }
            TransportKind::Lan => self.lan.connect_device(id, secret).await,
        }
    }

    pub async fn auto_connect(&self, transport: TransportKind) {
        self.activate(transport).await;
        match transport {
            TransportKind::Ble => self.ble.auto_connect().await,
            TransportKind::Lan => self.lan.auto_connect().await,
        }
    }

    pub async fn disconnect(&self) {
        match self.active_transport() {
            TransportKind::Ble => self.ble.disconnect().await,
            TransportKind::Lan => self.lan.disconnect().await,
        }
    }

    pub async fn submit_pairing_pin(&self, request_id: &str, pin: String) -> Result<()> {
        if self.active_transport() != TransportKind::Ble {
            bail!("pairing passkeys are only used by BLE connections");
        }
        self.ble.submit_pairing_pin(request_id, pin).await
    }

    pub async fn cancel_pairing(&self, request_id: &str) -> Result<()> {
        self.ble.cancel_pairing(request_id).await
    }

    async fn activate(&self, transport: TransportKind) {
        let previous = self.active_transport();
        if previous == transport {
            return;
        }
        match previous {
            TransportKind::Ble => self.ble.disconnect().await,
            TransportKind::Lan => self.lan.disconnect().await,
        }
        self.active.store(transport.code(), Ordering::Release);
        self.state
            .update_device(|device| {
                device.transport = transport.as_str().into();
                device.phase = "idle".into();
                device.connection_mode = "idle".into();
                device.preferred_device_id = None;
                device.selected_device_id = None;
                device.candidates.clear();
                device.scan_observed = 0;
                device.scan_started_at = None;
                device.pairing = None;
                device.last_error = None;
            })
            .await;
        self.state
            .log(
                "info",
                "gateway",
                format!("active transport changed to {}", transport.as_str()),
            )
            .await;
    }
}

fn relay_events(mut source: broadcast::Receiver<String>, destination: broadcast::Sender<String>) {
    tokio::spawn(async move {
        loop {
            match source.recv().await {
                Ok(event) => {
                    let _ = destination.send(event);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

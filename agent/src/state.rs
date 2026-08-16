use std::{
    collections::VecDeque,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{RwLock, broadcast};

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub at: u64,
    pub level: &'static str,
    pub scope: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentStatus {
    pub version: &'static str,
    pub paused: bool,
    pub platform: &'static str,
    pub autostart_enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BleCandidate {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
    pub advertises_service: bool,
    pub protocol_major: Option<u8>,
    pub owned: Option<bool>,
    pub battery: Option<bool>,
    pub fast_advertising: Option<bool>,
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceStatus {
    pub phase: String,
    pub connection_mode: String,
    pub preferred_device_id: Option<String>,
    pub selected_device_id: Option<String>,
    pub candidates: Vec<BleCandidate>,
    pub scan_observed: usize,
    pub scan_started_at: Option<u64>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub firmware: Option<String>,
    pub mtu: Option<u64>,
    pub config: Option<Value>,
    pub capabilities: Option<Value>,
    pub resources: Vec<Value>,
    pub bonds: Vec<Value>,
    pub diagnostics: Option<Value>,
    pub pairing: Option<PairingStatus>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingStatus {
    pub request_id: String,
    pub device_name: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceTypeStatus {
    pub id: String,
    pub title: String,
    pub description: String,
    pub configurable: bool,
    pub multi_instance: bool,
    pub auto_sync: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceStatus {
    pub id: String,
    pub type_id: String,
    pub title: String,
    pub enabled: bool,
    pub phase: String,
    pub resource_keys: Vec<String>,
    pub last_sync_at: Option<u64>,
    pub next_sync_at: Option<u64>,
    pub last_error: Option<String>,
    pub details: Value,
}

impl Default for SourceStatus {
    fn default() -> Self {
        Self {
            id: String::new(),
            type_id: String::new(),
            title: String::new(),
            enabled: false,
            phase: String::new(),
            resource_keys: Vec::new(),
            last_sync_at: None,
            next_sync_at: None,
            last_error: None,
            details: Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub agent: AgentStatus,
    pub device: DeviceStatus,
    pub source_types: Vec<SourceTypeStatus>,
    pub sources: Vec<SourceStatus>,
    pub logs: Vec<LogEntry>,
}

pub struct SharedState {
    snapshot: RwLock<Snapshot>,
    events: broadcast::Sender<String>,
}

impl SharedState {
    pub fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            snapshot: RwLock::new(Snapshot {
                agent: AgentStatus {
                    version: env!("CARGO_PKG_VERSION"),
                    paused: false,
                    platform: std::env::consts::OS,
                    autostart_enabled: crate::autostart::is_enabled(),
                },
                device: DeviceStatus {
                    phase: "scanning".into(),
                    connection_mode: "auto".into(),
                    ..Default::default()
                },
                source_types: Vec::new(),
                sources: Vec::new(),
                logs: Vec::new(),
            }),
            events,
        })
    }

    pub async fn snapshot(&self) -> Snapshot {
        self.snapshot.read().await.clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    pub async fn update_device(&self, update: impl FnOnce(&mut DeviceStatus)) {
        let mut snapshot = self.snapshot.write().await;
        update(&mut snapshot.device);
        self.publish_locked(&snapshot);
    }

    pub async fn register_source_type(&self, source_type: SourceTypeStatus) {
        let mut snapshot = self.snapshot.write().await;
        if snapshot
            .source_types
            .iter()
            .all(|item| item.id != source_type.id)
        {
            snapshot.source_types.push(source_type);
        }
        self.publish_locked(&snapshot);
    }

    pub async fn register_source(&self, source: SourceStatus) {
        let mut snapshot = self.snapshot.write().await;
        if let Some(current) = snapshot
            .sources
            .iter_mut()
            .find(|item| item.id == source.id)
        {
            *current = source;
        } else {
            snapshot.sources.push(source);
        }
        self.publish_locked(&snapshot);
    }

    pub async fn register_source_if_absent(&self, source: SourceStatus) -> bool {
        let mut snapshot = self.snapshot.write().await;
        if snapshot.sources.iter().any(|item| item.id == source.id) {
            return false;
        }
        snapshot.sources.push(source);
        self.publish_locked(&snapshot);
        true
    }

    pub async fn update_source(&self, id: &str, update: impl FnOnce(&mut SourceStatus)) {
        let mut snapshot = self.snapshot.write().await;
        if let Some(source) = snapshot.sources.iter_mut().find(|item| item.id == id) {
            update(source);
        }
        self.publish_locked(&snapshot);
    }

    pub async fn remove_source(&self, id: &str) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.sources.retain(|item| item.id != id);
        self.publish_locked(&snapshot);
    }

    pub async fn set_paused(&self, paused: bool) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.agent.paused = paused;
        self.publish_locked(&snapshot);
    }

    pub async fn set_autostart(&self, enabled: bool) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.agent.autostart_enabled = enabled;
        self.publish_locked(&snapshot);
    }

    pub async fn paused(&self) -> bool {
        self.snapshot.read().await.agent.paused
    }

    pub async fn log(&self, level: &'static str, scope: &'static str, message: impl Into<String>) {
        let message = message.into();
        match level {
            "error" => tracing::error!(scope, "{message}"),
            "warn" => tracing::warn!(scope, "{message}"),
            _ => tracing::info!(scope, "{message}"),
        }
        let mut snapshot = self.snapshot.write().await;
        let mut logs = VecDeque::from(std::mem::take(&mut snapshot.logs));
        logs.push_front(LogEntry {
            at: unix_now(),
            level,
            scope,
            message,
        });
        logs.truncate(120);
        snapshot.logs = logs.into();
        self.publish_locked(&snapshot);
    }

    fn publish_locked(&self, snapshot: &Snapshot) {
        if let Ok(json) = serde_json::to_string(snapshot) {
            let _ = self.events.send(json);
        }
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{SharedState, SourceStatus};

    #[test]
    fn source_details_default_to_an_object() {
        assert!(SourceStatus::default().details.is_object());
    }

    #[tokio::test]
    async fn source_id_reservation_is_atomic() {
        let state = SharedState::new();
        let source = SourceStatus {
            id: "shared-id".into(),
            ..Default::default()
        };
        let (first, second) = tokio::join!(
            state.register_source_if_absent(source.clone()),
            state.register_source_if_absent(source),
        );
        assert_ne!(first, second);
        assert_eq!(state.snapshot().await.sources.len(), 1);
    }
}

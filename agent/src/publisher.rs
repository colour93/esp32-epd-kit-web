use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::{
    ble::BleGateway,
    state::{SharedState, unix_now},
};

const HEARTBEAT_SEC: u64 = 300;

#[derive(Clone, Debug)]
pub struct SemanticResource {
    pub producer_id: &'static str,
    pub key: &'static str,
    pub schema_id: &'static str,
    pub schema_version: u16,
    pub ttl_sec: u64,
    pub persistence: &'static str,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct CycleCompletion {
    pub cycle_id: u64,
    pub producer_id: &'static str,
    pub success: bool,
}

enum Command {
    Publish(SemanticResource, oneshot::Sender<Result<bool>>),
    CycleComplete(CycleCompletion),
    Reconcile,
    Flush(oneshot::Sender<()>),
}

struct CachedResource {
    resource: SemanticResource,
    payload_hash: u32,
    sent_hash: Option<u32>,
    last_write_at: Option<u64>,
}

#[derive(Clone)]
pub struct ResourcePublisher {
    commands: mpsc::Sender<Command>,
}

impl ResourcePublisher {
    pub fn spawn(
        state: Arc<SharedState>,
        ble: BleGateway,
        completions: mpsc::Sender<CycleCompletion>,
    ) -> Self {
        let (commands, receiver) = mpsc::channel(32);
        let publisher = Self { commands };
        tokio::spawn(run(state, ble.clone(), completions, receiver));
        let mut events = ble.subscribe();
        let reconcile = publisher.clone();
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if event == "ble.connected" {
                    let _ = reconcile.commands.send(Command::Reconcile).await;
                }
            }
        });
        publisher
    }

    pub async fn publish(&self, resource: SemanticResource) -> Result<bool> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Publish(resource, reply))
            .await
            .map_err(|_| anyhow!("resource publisher stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("resource publish was cancelled"))?
    }

    pub async fn complete_cycle(
        &self,
        cycle_id: u64,
        producer_id: &'static str,
        success: bool,
    ) -> Result<()> {
        self.commands
            .send(Command::CycleComplete(CycleCompletion {
                cycle_id,
                producer_id,
                success,
            }))
            .await
            .map_err(|_| anyhow!("resource publisher stopped"))
    }

    pub async fn flush(&self) -> Result<()> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Flush(reply))
            .await
            .map_err(|_| anyhow!("resource publisher stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("publisher flush was cancelled"))
    }
}

async fn run(
    state: Arc<SharedState>,
    ble: BleGateway,
    completions: mpsc::Sender<CycleCompletion>,
    mut commands: mpsc::Receiver<Command>,
) {
    let mut cache = HashMap::<String, CachedResource>::new();
    while let Some(command) = commands.recv().await {
        match command {
            Command::Publish(resource, reply) => {
                let payload_hash = match serde_json::to_vec(&resource.payload) {
                    Ok(encoded) => crc32fast::hash(&encoded),
                    Err(error) => {
                        let _ = reply.send(Err(error.into()));
                        continue;
                    }
                };
                let key = resource.key.to_owned();
                let entry = cache.entry(key).or_insert_with(|| CachedResource {
                    resource: resource.clone(),
                    payload_hash,
                    sent_hash: None,
                    last_write_at: None,
                });
                entry.resource = resource;
                entry.payload_hash = payload_hash;
                let result = publish_one(&state, &ble, entry, false).await;
                let _ = reply.send(result);
            }
            Command::Reconcile => {
                for entry in cache.values_mut() {
                    entry.sent_hash = None;
                    if let Err(error) = publish_one(&state, &ble, entry, true).await {
                        state
                            .log("warn", "publisher", format!("reconcile failed: {error:#}"))
                            .await;
                    }
                }
            }
            Command::CycleComplete(completion) => {
                let _ = completions.send(completion).await;
            }
            Command::Flush(reply) => {
                let _ = reply.send(());
            }
        }
    }
}

async fn publish_one(
    state: &SharedState,
    ble: &BleGateway,
    entry: &mut CachedResource,
    force: bool,
) -> Result<bool> {
    let now = unix_now();
    let heartbeat_due = entry
        .last_write_at
        .is_none_or(|last| now.saturating_sub(last) >= HEARTBEAT_SEC);
    if !force && entry.sent_hash == Some(entry.payload_hash) && !heartbeat_due {
        return Ok(false);
    }
    if !ble.is_connected() {
        state
            .log(
                "info",
                "publisher",
                format!("{} cached until BLE reconnects", entry.resource.key),
            )
            .await;
        return Ok(false);
    }
    let snapshot = state.snapshot().await;
    let current_revision = snapshot
        .device
        .resources
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some(entry.resource.key))
        .and_then(|item| item.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let revision = now.max(current_revision.saturating_add(1));
    let resource = json!({
        "key": entry.resource.key,
        "schema_id": entry.resource.schema_id,
        "schema_version": entry.resource.schema_version,
        "revision": revision,
        "updated_at": now,
        "ttl_sec": entry.resource.ttl_sec,
        "persistence": entry.resource.persistence,
        "payload": entry.resource.payload,
    });
    ble.request("resource.put", json!({ "resource": resource }))
        .await
        .with_context(|| format!("publish resource {}", entry.resource.key))?;
    entry.sent_hash = Some(entry.payload_hash);
    entry.last_write_at = Some(now);
    if let Ok(resources) = ble.request("resource.list", json!({})).await {
        state
            .update_device(|device| {
                device.resources = resources
                    .get("resources")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
            })
            .await;
    }
    state
        .log(
            "info",
            "publisher",
            format!(
                "{} synchronized by {}; revision={revision} hash={:08x}",
                entry.resource.key, entry.resource.producer_id, entry.payload_hash
            ),
        )
        .await;
    Ok(true)
}

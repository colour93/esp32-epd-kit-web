use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::{
    gateway::DeviceGateway,
    state::{SharedState, unix_now},
};

const HEARTBEAT_SEC: u64 = 300;

#[derive(Clone, Debug)]
pub struct SemanticResource {
    pub source_id: String,
    pub key: String,
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
    Get(String, oneshot::Sender<Option<SemanticResource>>),
    Delete(String, oneshot::Sender<Result<()>>),
    CycleComplete(CycleCompletion),
    Reconcile,
    Prepare(Vec<String>, oneshot::Sender<Result<()>>),
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
        gateway: DeviceGateway,
        completions: mpsc::Sender<CycleCompletion>,
    ) -> Self {
        let (commands, receiver) = mpsc::channel(32);
        let publisher = Self { commands };
        tokio::spawn(run(state, gateway.clone(), completions, receiver));
        let mut events = gateway.subscribe();
        let reconcile = publisher.clone();
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if event == "device.connected" {
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

    pub async fn delete(&self, key: String) -> Result<()> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Delete(key, reply))
            .await
            .map_err(|_| anyhow!("resource publisher stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("resource delete was cancelled"))?
    }

    pub async fn get(&self, key: String) -> Result<Option<SemanticResource>> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Get(key, reply))
            .await
            .map_err(|_| anyhow!("resource publisher stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("resource lookup was cancelled"))
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

    pub async fn reconcile(&self) -> Result<()> {
        self.commands
            .send(Command::Reconcile)
            .await
            .map_err(|_| anyhow!("resource publisher stopped"))
    }

    pub async fn prepare_page(&self, page: &Value) -> Result<()> {
        let keys = page_resource_keys(page)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Prepare(keys, reply))
            .await
            .map_err(|_| anyhow!("resource publisher stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("page resource preparation was cancelled"))?
    }
}

async fn run(
    state: Arc<SharedState>,
    gateway: DeviceGateway,
    completions: mpsc::Sender<CycleCompletion>,
    mut commands: mpsc::Receiver<Command>,
) {
    let mut cache = HashMap::<String, CachedResource>::new();
    let mut pending_deletes = HashSet::<String>::new();
    while let Some(command) = commands.recv().await {
        match command {
            Command::Publish(resource, reply) => {
                let snapshot = state.snapshot().await;
                let enabled = snapshot
                    .sources
                    .iter()
                    .find(|source| source.id == resource.source_id)
                    .is_none_or(|source| source.enabled);
                if !enabled {
                    let _ = reply.send(Ok(false));
                    continue;
                }
                let payload_hash = match serde_json::to_vec(&resource.payload) {
                    Ok(encoded) => crc32fast::hash(&encoded),
                    Err(error) => {
                        let _ = reply.send(Err(error.into()));
                        continue;
                    }
                };
                let key = resource.key.clone();
                pending_deletes.remove(&key);
                let entry = cache.entry(key).or_insert_with(|| CachedResource {
                    resource: resource.clone(),
                    payload_hash,
                    sent_hash: None,
                    last_write_at: None,
                });
                entry.resource = resource;
                entry.payload_hash = payload_hash;
                state.upsert_catalog_resource(catalog_summary(entry)).await;
                let result = publish_one(&state, &gateway, entry, false).await;
                let _ = reply.send(result);
            }
            Command::Get(key, reply) => {
                let resource = cache.get(&key).map(|entry| entry.resource.clone());
                let _ = reply.send(resource);
            }
            Command::Delete(key, reply) => {
                cache.remove(&key);
                state.remove_catalog_resource(&key).await;
                pending_deletes.insert(key.clone());
                match delete_one(&state, &gateway, &key).await {
                    Ok(true) => {
                        pending_deletes.remove(&key);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        state
                            .log(
                                "warn",
                                "publisher",
                                format!("delete queued for {key}: {error:#}"),
                            )
                            .await;
                    }
                }
                let _ = reply.send(Ok(()));
            }
            Command::Reconcile => {
                let snapshot = state.snapshot().await;
                let desired = desired_resource_keys(&snapshot.device.config);
                for key in snapshot
                    .device
                    .resources
                    .iter()
                    .filter_map(|item| item.get("key").and_then(Value::as_str))
                {
                    if !desired.contains(key) {
                        pending_deletes.insert(key.to_owned());
                    }
                }
                for key in pending_deletes.clone() {
                    match delete_one(&state, &gateway, &key).await {
                        Ok(true) => {
                            pending_deletes.remove(&key);
                        }
                        Ok(false) => {}
                        Err(error) => {
                            state
                                .log(
                                    "warn",
                                    "publisher",
                                    format!("delete reconcile failed for {key}: {error:#}"),
                                )
                                .await;
                        }
                    }
                }
                for entry in cache.values_mut() {
                    entry.sent_hash = None;
                    if !desired.contains(entry.resource.key.as_str()) {
                        continue;
                    }
                    if let Err(error) = publish_one(&state, &gateway, entry, true).await {
                        state
                            .log("warn", "publisher", format!("reconcile failed: {error:#}"))
                            .await;
                    }
                }
            }
            Command::Prepare(keys, reply) => {
                let mut result = Ok(());
                for key in keys {
                    if let Some(entry) = cache.get_mut(&key) {
                        if let Err(error) = publish_one(&state, &gateway, entry, true).await {
                            result = Err(error);
                            break;
                        }
                    }
                }
                let _ = reply.send(result);
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

async fn delete_one(state: &SharedState, gateway: &DeviceGateway, key: &str) -> Result<bool> {
    if !gateway.is_connected() {
        state
            .log(
                "info",
                "publisher",
                format!("{key} deletion queued until BLE reconnects"),
            )
            .await;
        return Ok(false);
    }
    gateway
        .request("resource.delete", json!({ "key": key }))
        .await
        .with_context(|| format!("delete resource {key}"))?;
    state
        .update_device(|device| {
            device
                .resources
                .retain(|item| item.get("key").and_then(Value::as_str) != Some(key));
        })
        .await;
    state
        .log("info", "publisher", format!("{key} removed"))
        .await;
    Ok(true)
}

async fn publish_one(
    state: &SharedState,
    gateway: &DeviceGateway,
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
    if !gateway.is_connected() {
        state
            .log(
                "debug",
                "publisher",
                format!("{} cached until BLE reconnects", entry.resource.key),
            )
            .await;
        return Ok(false);
    }
    let snapshot = state.snapshot().await;
    if !force
        && !desired_resource_keys(&snapshot.device.config).contains(entry.resource.key.as_str())
    {
        return Ok(false);
    }
    if let Some(source) = snapshot
        .sources
        .iter()
        .find(|source| source.id == entry.resource.source_id)
    {
        if !source.enabled {
            return Ok(false);
        }
        if !source.realtime
            && !force
            && source.interval_sec.is_some_and(|interval| {
                entry
                    .last_write_at
                    .is_some_and(|last| now.saturating_sub(last) < interval)
            })
        {
            return Ok(false);
        }
    }
    let existing =
        snapshot.device.resources.iter().any(|item| {
            item.get("key").and_then(Value::as_str) == Some(entry.resource.key.as_str())
        });
    let max_resources = snapshot
        .device
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.get("max_resources"))
        .and_then(Value::as_u64);
    if !existing
        && max_resources.is_some_and(|limit| snapshot.device.resources.len() as u64 >= limit)
    {
        let limit = max_resources.unwrap();
        bail!(
            "device resource store is full ({}/{limit}); update firmware or remove an unused resource",
            snapshot.device.resources.len()
        );
    }
    let current_revision = snapshot
        .device
        .resources
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some(entry.resource.key.as_str()))
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
    gateway
        .request("resource.put", json!({ "resource": resource }))
        .await
        .with_context(|| format!("publish resource {}", entry.resource.key))?;
    entry.sent_hash = Some(entry.payload_hash);
    entry.last_write_at = Some(now);
    if let Ok(resources) = gateway.request("resource.list", json!({})).await {
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
            "debug",
            "publisher",
            format!(
                "{} synchronized by {}; revision={revision} hash={:08x}",
                entry.resource.key, entry.resource.source_id, entry.payload_hash
            ),
        )
        .await;
    Ok(true)
}

fn desired_resource_keys(config: &Option<Value>) -> HashSet<&str> {
    config
        .as_ref()
        .and_then(|config| config.get("page"))
        .map(page_resource_keys)
        .unwrap_or_default()
}

fn page_resource_keys(page: &Value) -> HashSet<&str> {
    page.get("bindings")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|bindings| bindings.values())
        .filter_map(|binding| {
            binding
                .as_str()
                .or_else(|| binding.get("resource_key").and_then(Value::as_str))
        })
        .filter(|key| !key.is_empty())
        .collect()
}

fn catalog_summary(entry: &CachedResource) -> Value {
    json!({
        "key": entry.resource.key,
        "schema_id": entry.resource.schema_id,
        "schema_version": entry.resource.schema_version,
        "revision": unix_now(),
        "updated_at": unix_now(),
        "ttl_sec": entry.resource.ttl_sec,
        "persistence": entry.resource.persistence,
        "content_crc": entry.payload_hash,
    })
}

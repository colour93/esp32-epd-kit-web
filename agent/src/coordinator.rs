use std::{collections::HashSet, sync::Arc};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{
    ble::BleGateway,
    producer::ProducerRegistry,
    publisher::{CycleCompletion, ResourcePublisher},
    state::SharedState,
};

pub struct SyncCoordinator;

impl SyncCoordinator {
    pub fn spawn(
        state: Arc<SharedState>,
        ble: BleGateway,
        producers: ProducerRegistry,
        publisher: ResourcePublisher,
        completions: mpsc::Receiver<CycleCompletion>,
    ) {
        tokio::spawn(run(state, ble, producers, publisher, completions));
    }
}

async fn run(
    state: Arc<SharedState>,
    ble: BleGateway,
    producers: ProducerRegistry,
    publisher: ResourcePublisher,
    mut completions: mpsc::Receiver<CycleCompletion>,
) {
    let mut events = ble.subscribe();
    let mut active: Option<(u64, HashSet<&'static str>, bool)> = None;
    let mut next_cycle_id = 1u64;
    loop {
        tokio::select! {
            event = events.recv() => {
                let Ok(event) = event else { continue };
                if event != "ble.connected" { continue; }
                let snapshot = state.snapshot().await;
                let automatic_battery = snapshot.device.connection_mode == "auto"
                    && snapshot.device.config.as_ref()
                        .and_then(|config| config.get("power"))
                        .and_then(|power| power.get("profile"))
                        .and_then(Value::as_str) == Some("battery");
                if !automatic_battery { continue; }
                let cycle_id = next_cycle_id;
                next_cycle_id = next_cycle_id.wrapping_add(1).max(1);
                let expected = producers.auto_sync_ids().into_iter().collect::<HashSet<_>>();
                if expected.is_empty() {
                    complete_device_sync(&state, &ble, &publisher).await;
                    continue;
                }
                active = Some((cycle_id, expected, true));
                if let Err(error) = producers.refresh_cycle(cycle_id).await {
                    state.log("warn", "coordinator", error.to_string()).await;
                } else {
                    state.log("info", "coordinator", format!("sync cycle {cycle_id} started")).await;
                }
            }
            completion = completions.recv() => {
                let Some(completion) = completion else { return };
                let Some((cycle_id, expected, success)) = active.as_mut() else { continue };
                if *cycle_id != completion.cycle_id { continue; }
                expected.remove(completion.producer_id);
                *success &= completion.success;
                if expected.is_empty() {
                    let finished_id = *cycle_id;
                    let finished_success = *success;
                    active = None;
                    state.log("info", "coordinator", format!("sync cycle {finished_id} producers complete; success={finished_success}")).await;
                    complete_device_sync(&state, &ble, &publisher).await;
                }
            }
        }
    }
}

async fn complete_device_sync(
    state: &SharedState,
    ble: &BleGateway,
    publisher: &ResourcePublisher,
) {
    if let Err(error) = publisher.flush().await {
        state.log("warn", "coordinator", error.to_string()).await;
        return;
    }
    match ble.request("system.sync.complete", json!({})).await {
        Ok(result) => {
            state
                .log(
                    "info",
                    "coordinator",
                    format!(
                        "device sync complete; sleep_scheduled={}",
                        result
                            .get("sleep_scheduled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    ),
                )
                .await;
        }
        Err(error) => state.log("warn", "coordinator", error.to_string()).await,
    }
}

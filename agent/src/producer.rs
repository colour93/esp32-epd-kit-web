use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use tokio::sync::mpsc;

use crate::{
    publisher::ResourcePublisher,
    state::{SharedState, SourceStatus, SourceTypeStatus},
};

#[derive(Clone)]
pub struct ProducerContext {
    pub state: Arc<SharedState>,
    pub publisher: ResourcePublisher,
}

#[derive(Debug)]
pub enum ProducerTrigger {
    Manual,
    SyncCycle(u64),
}

pub struct ProducerManifest {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub configurable: bool,
    pub multi_instance: bool,
    pub auto_sync: bool,
    pub built_in_source: Option<BuiltInSourceManifest>,
}

#[derive(Clone, Copy)]
pub struct BuiltInSourceManifest {
    pub id: &'static str,
    pub title: &'static str,
    pub resource_keys: &'static [&'static str],
}

#[derive(Clone)]
pub struct ProducerControl {
    pub manifest: &'static ProducerManifest,
    trigger: mpsc::Sender<ProducerTrigger>,
}

impl ProducerControl {
    pub fn new(
        manifest: &'static ProducerManifest,
        trigger: mpsc::Sender<ProducerTrigger>,
    ) -> Self {
        Self { manifest, trigger }
    }

    pub async fn refresh(&self) -> Result<()> {
        self.trigger
            .send(ProducerTrigger::Manual)
            .await
            .map_err(|_| anyhow!("producer {} stopped", self.manifest.id))
    }

    pub async fn refresh_cycle(&self, cycle_id: u64) -> Result<()> {
        self.trigger
            .send(ProducerTrigger::SyncCycle(cycle_id))
            .await
            .map_err(|_| anyhow!("producer {} stopped", self.manifest.id))
    }
}

#[derive(Clone)]
pub struct ProducerRegistry {
    items: Arc<Vec<ProducerControl>>,
}

impl ProducerRegistry {
    pub async fn new(state: &SharedState, items: Vec<ProducerControl>) -> Result<Self> {
        for (index, item) in items.iter().enumerate() {
            if items[..index]
                .iter()
                .any(|registered| registered.manifest.id == item.manifest.id)
            {
                bail!("duplicate producer id: {}", item.manifest.id);
            }
            state
                .register_source_type(SourceTypeStatus {
                    id: item.manifest.id.into(),
                    title: item.manifest.title.into(),
                    description: item.manifest.description.into(),
                    configurable: item.manifest.configurable,
                    multi_instance: item.manifest.multi_instance,
                    auto_sync: item.manifest.auto_sync,
                })
                .await;
            if let Some(source) = item.manifest.built_in_source {
                state
                    .register_source(SourceStatus {
                        id: source.id.into(),
                        type_id: item.manifest.id.into(),
                        title: source.title.into(),
                        enabled: true,
                        phase: "starting".into(),
                        resource_keys: source
                            .resource_keys
                            .iter()
                            .map(|key| (*key).into())
                            .collect(),
                        ..Default::default()
                    })
                    .await;
            }
        }
        Ok(Self {
            items: Arc::new(items),
        })
    }

    pub async fn refresh(&self, id: &str) -> Result<()> {
        let producer = self
            .items
            .iter()
            .find(|item| item.manifest.id == id)
            .ok_or_else(|| anyhow!("unknown producer id: {id}"))?;
        producer.refresh().await
    }

    pub fn auto_sync_ids(&self) -> Vec<&'static str> {
        self.items
            .iter()
            .filter(|item| item.manifest.auto_sync)
            .map(|item| item.manifest.id)
            .collect()
    }

    pub async fn refresh_cycle(&self, cycle_id: u64) -> Result<()> {
        for producer in self.items.iter().filter(|item| item.manifest.auto_sync) {
            producer.refresh_cycle(cycle_id).await?;
        }
        Ok(())
    }
}

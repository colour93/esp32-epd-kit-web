use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PagePreset {
    pub id: String,
    pub title: String,
    pub page: Value,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SourcePolicy {
    pub enabled: Option<bool>,
    pub interval_sec: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SettingsFile {
    #[serde(default)]
    page_presets: Vec<PagePreset>,
    #[serde(default)]
    source_policies: HashMap<String, SourcePolicy>,
}

#[derive(Clone)]
pub struct SettingsStore {
    path: PathBuf,
    data: Arc<RwLock<SettingsFile>>,
}

impl SettingsStore {
    pub fn load() -> Result<Self> {
        let directory = dirs::config_dir()
            .ok_or_else(|| anyhow!("config directory unavailable"))?
            .join("epd-agent");
        std::fs::create_dir_all(&directory).context("create agent config directory")?;
        let path = directory.join("settings.json");
        let data = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path).context("read agent settings")?)
                .context("parse agent settings")?
        } else {
            SettingsFile::default()
        };
        Ok(Self {
            path,
            data: Arc::new(RwLock::new(data)),
        })
    }

    pub async fn presets(&self) -> Vec<PagePreset> {
        self.data.read().await.page_presets.clone()
    }

    pub async fn policies(&self) -> HashMap<String, SourcePolicy> {
        self.data.read().await.source_policies.clone()
    }

    pub async fn put_preset(&self, preset: PagePreset) -> Result<()> {
        let mut data = self.data.write().await;
        if let Some(current) = data
            .page_presets
            .iter_mut()
            .find(|item| item.id == preset.id)
        {
            *current = preset;
        } else {
            data.page_presets.push(preset);
        }
        save(&self.path, &data)
    }

    pub async fn delete_preset(&self, id: &str) -> Result<bool> {
        let mut data = self.data.write().await;
        let before = data.page_presets.len();
        data.page_presets.retain(|item| item.id != id);
        let removed = before != data.page_presets.len();
        if removed {
            save(&self.path, &data)?;
        }
        Ok(removed)
    }

    pub async fn set_source_policy(&self, id: String, policy: SourcePolicy) -> Result<()> {
        let mut data = self.data.write().await;
        data.source_policies.insert(id, policy);
        save(&self.path, &data)
    }
}

fn save(path: &PathBuf, data: &SettingsFile) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(data)?).context("write agent settings")?;
    std::fs::rename(&temporary, path).context("replace agent settings")
}

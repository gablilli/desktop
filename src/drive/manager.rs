use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveConfig {
    pub id: String,
    pub name: String,
    pub drive_type: String,
    pub sync_path: PathBuf,
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveState {
    pub drives: HashMap<String, DriveConfig>,
}

impl Default for DriveState {
    fn default() -> Self {
        Self {
            drives: HashMap::new(),
        }
    }
}

pub struct DriveManager {
    state: Arc<RwLock<DriveState>>,
    config_dir: PathBuf,
}

impl DriveManager {
    /// Create a new DriveManager instance
    pub fn new() -> Result<Self> {
        let config_dir = Self::get_config_dir()?;

        // Ensure config directory exists
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir)
                .context("Failed to create .cloudreve config directory")?;
        }

        Ok(Self {
            state: Arc::new(RwLock::new(DriveState::default())),
            config_dir,
        })
    }

    /// Get the .cloudreve config directory path
    fn get_config_dir() -> Result<PathBuf> {
        let home_dir = dirs::home_dir().context("Failed to get user home directory")?;
        Ok(home_dir.join(".cloudreve"))
    }

    /// Get the config file path
    fn get_config_file(&self) -> PathBuf {
        self.config_dir.join("drives.json")
    }

    /// Load drive configurations from disk
    pub async fn load(&self) -> Result<()> {
        let config_file = self.get_config_file();

        if !config_file.exists() {
            tracing::info!(target: "drive", "No existing drive config found, starting fresh");
            return Ok(());
        }

        tracing::debug!(target: "drive", path = %config_file.display(), "Loading drive configurations");

        let content =
            fs::read_to_string(&config_file).context("Failed to read drive config file")?;

        let state: DriveState =
            serde_json::from_str(&content).context("Failed to parse drive config")?;

        let mut write_guard = self.state.write().await;
        *write_guard = state;

        tracing::info!(target: "drive", count = write_guard.drives.len(), "Loaded drive(s) from config");

        Ok(())
    }

    /// Persist drive configurations to disk
    pub async fn persist(&self) -> Result<()> {
        let config_file = self.get_config_file();
        let read_guard = self.state.read().await;

        tracing::debug!(target: "drive", path = %config_file.display(), count = read_guard.drives.len(), "Persisting drive configurations");

        let content = serde_json::to_string_pretty(&*read_guard)
            .context("Failed to serialize drive state")?;

        fs::write(&config_file, content).context("Failed to write drive config file")?;

        tracing::info!(target: "drive", count = read_guard.drives.len(), "Persisted drive(s) to config");

        Ok(())
    }

    /// Add a new drive
    pub async fn add_drive(&self, config: DriveConfig) -> Result<String> {
        let mut write_guard = self.state.write().await;
        let id = config.id.clone();
        write_guard.drives.insert(id.clone(), config);
        Ok(id)
    }

    /// Remove a drive by ID
    pub async fn remove_drive(&self, id: &str) -> Result<Option<DriveConfig>> {
        let mut write_guard = self.state.write().await;
        Ok(write_guard.drives.remove(id))
    }

    /// Get a drive by ID
    pub async fn get_drive(&self, id: &str) -> Option<DriveConfig> {
        let read_guard = self.state.read().await;
        read_guard.drives.get(id).cloned()
    }

    /// List all drives
    pub async fn list_drives(&self) -> Vec<DriveConfig> {
        let read_guard = self.state.read().await;
        read_guard.drives.values().cloned().collect()
    }

    /// Update drive configuration
    pub async fn update_drive(&self, id: &str, config: DriveConfig) -> Result<()> {
        let mut write_guard = self.state.write().await;
        if write_guard.drives.contains_key(id) {
            write_guard.drives.insert(id.to_string(), config);
            Ok(())
        } else {
            anyhow::bail!("Drive not found: {}", id)
        }
    }

    /// Enable/disable a drive
    pub async fn set_drive_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut write_guard = self.state.write().await;
        if let Some(drive) = write_guard.drives.get_mut(id) {
            drive.enabled = enabled;
            Ok(())
        } else {
            anyhow::bail!("Drive not found: {}", id)
        }
    }

    /// Placeholder: Start syncing a drive
    pub async fn start_sync(&self, id: &str) -> Result<()> {
        // TODO: Implement actual sync logic
        tracing::info!(target: "drive::sync", drive_id = %id, "Starting sync for drive");
        Ok(())
    }

    /// Placeholder: Stop syncing a drive
    pub async fn stop_sync(&self, id: &str) -> Result<()> {
        // TODO: Implement actual sync logic
        tracing::info!(target: "drive::sync", drive_id = %id, "Stopping sync for drive");
        Ok(())
    }

    /// Placeholder: Get sync status for a drive
    pub async fn get_sync_status(&self, id: &str) -> Result<serde_json::Value> {
        // TODO: Implement actual status retrieval
        tracing::debug!(target: "drive::sync", drive_id = %id, "Getting sync status");
        Ok(serde_json::json!({
            "drive_id": id,
            "status": "idle",
            "last_sync": null,
            "files_synced": 0,
        }))
    }
}

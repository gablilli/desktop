use super::commands::ManagerCommand;
use super::mounts::{DriveConfig, Mount};
use crate::EventBroadcaster;
use crate::drive::commands::MountCommand;
use crate::drive::utils::{local_path_to_cr_uri, view_online_url};
use crate::inventory::{InventoryDb, RecentTasks, TaskRecord, TaskStatus};
use crate::tasks::TaskProgress;
use crate::utils::toast::send_conflict_toast;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{fs, thread};
use tokio::spawn;
use tokio::sync::{Mutex, RwLock, mpsc};

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

/// Summary of the current status including drives and recent tasks
#[derive(Debug, Clone, Serialize)]
pub struct StatusSummary {
    /// All configured drives (unfiltered)
    pub drives: Vec<DriveConfig>,
    /// Active tasks (pending/running) with optional live progress info
    pub active_tasks: Vec<TaskWithProgress>,
    /// Recently finished tasks (completed/failed/cancelled)
    pub finished_tasks: Vec<TaskRecord>,
}

/// A task record with optional live progress information
#[derive(Debug, Clone, Serialize)]
pub struct TaskWithProgress {
    /// The task record from the database
    #[serde(flatten)]
    pub task: TaskRecord,
    /// Live progress information for running tasks (None if task is not currently running)
    pub live_progress: Option<TaskProgress>,
}

/// Capacity summary for UI display
#[derive(Debug, Clone, Serialize)]
pub struct CapacitySummary {
    /// Total capacity in bytes
    pub total: i64,
    /// Used capacity in bytes
    pub used: i64,
    /// Formatted label for display (e.g., "152.1 MB / 1.0 GB (14.9%)")
    pub label: String,
}

/// Sync status for UI display
#[derive(Debug, Clone, Serialize)]
pub enum SyncStatus {
    /// All files are in sync
    InSync,
    /// Currently syncing files
    Syncing,
    /// Sync is paused
    Paused,
    /// There was an error during sync
    Error,
}

/// Drive status information for the Windows Shell UI
#[derive(Debug, Clone, Serialize)]
pub struct DriveStatusUI {
    /// Drive display name
    pub name: String,
    /// Path to the raw (non-ICO) icon image
    pub raw_icon_path: Option<String>,
    /// Capacity summary (None if not available)
    pub capacity: Option<CapacitySummary>,
    /// URL to user profile page
    pub profile_url: String,
    /// URL to settings page
    pub settings_url: String,
    pub storage_url: String,
    /// Current sync status
    pub sync_status: SyncStatus,
    /// Number of active (pending/running) tasks
    pub active_task_count: usize,
}

pub struct DriveManager {
    drives: Arc<RwLock<HashMap<String, Arc<Mount>>>>,
    config_dir: PathBuf,
    inventory: Arc<InventoryDb>,
    command_tx: mpsc::UnboundedSender<ManagerCommand>,
    command_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<ManagerCommand>>>>,
    processor_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    event_broadcaster: Arc<EventBroadcaster>,
}

impl DriveManager {
    /// Create a new DriveManager instance
    pub fn new(event_broadcaster: Arc<EventBroadcaster>) -> Result<Self> {
        let config_dir = Self::get_config_dir()?;

        // Ensure config directory exists
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir)
                .context("Failed to create .cloudreve config directory")?;
        }

        let (command_tx, command_rx) = mpsc::unbounded_channel();

        Ok(Self {
            config_dir,
            drives: Arc::new(RwLock::new(HashMap::new())),
            inventory: Arc::new(InventoryDb::new().context("Failed to create inventory database")?),
            command_tx,
            command_rx: Arc::new(Mutex::new(Some(command_rx))),
            processor_handle: Arc::new(Mutex::new(None)),
            event_broadcaster: event_broadcaster,
        })
    }

    pub fn get_inventory(&self) -> Arc<InventoryDb> {
        self.inventory.clone()
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
            self.event_broadcaster.no_drive();
            return Ok(());
        }

        tracing::debug!(target: "drive", path = %config_file.display(), "Loading drive configurations");

        let content =
            fs::read_to_string(&config_file).context("Failed to read drive config file")?;

        let state: DriveState =
            serde_json::from_str(&content).context("Failed to parse drive config")?;

        // Add drives to manager
        let mut count = 0;
        for (id, config) in state.drives.iter() {
            self.add_drive(config.clone())
                .await
                .context(format!("Failed to add drive: {}", id))?;
            count += 1;
        }

        if count == 0 {
            self.event_broadcaster.no_drive();
        }

        tracing::info!(target: "drive", count = count, "Loaded drive(s) from config");

        Ok(())
    }

    /// Persist drive configurations to disk
    pub async fn persist(&self) -> Result<()> {
        let config_file = self.get_config_file();
        let write_guard = self.drives.write().await;

        tracing::debug!(target: "drive", path = %config_file.display(), count = write_guard.len(), "Persisting drive configurations");

        let mut new_state = DriveState::default();

        // Update drive states from underlying mounts
        for (id, mount) in write_guard.iter() {
            let config = mount.get_config().await;
            new_state.drives.insert(id.clone(), config);
        }

        let content =
            serde_json::to_string_pretty(&new_state).context("Failed to serialize drive state")?;
        fs::write(&config_file, content).context("Failed to write drive config file")?;

        tracing::info!(target: "drive", count = new_state.drives.len(), "Persisted drive(s) to config");

        Ok(())
    }

    /// Register a callback to be invoked when status UI changes
    /// This is a dummy implementation that calls the callback every 30 seconds
    pub fn register_on_status_ui_changed<F>(&self, fnc: F) -> Result<()>
    where
        F: Fn() + Send + 'static,
    {
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(30));
                tracing::trace!(target: "drive::manager", "Register_on_status_ui_changed: Invoking status UI changed callback");
                fnc();
            }
        });
        Ok(())
    }

    /// Add a new drive
    pub async fn add_drive(&self, mut config: DriveConfig) -> Result<String> {
        // Fetch favicon if icon_path is not set or doesn't exist
        if config.icon_path.is_none()
            || !config
                .icon_path
                .as_ref()
                .map(|p| std::path::Path::new(p).exists())
                .unwrap_or(false)
        {
            match super::favicon::fetch_and_save_favicon(&config.instance_url).await {
                Ok(result) => {
                    tracing::info!(target: "drive", ico_path = %result.ico_path, raw_path = %result.raw_path, "Favicon fetched successfully");
                    config.icon_path = Some(result.ico_path);
                    config.raw_icon_path = Some(result.raw_path);
                }
                Err(e) => {
                    tracing::warn!(target: "drive", error = %e, "Failed to fetch favicon, continuing without icon");
                }
            }
        }

        let mut write_guard = self.drives.write().await;
        let mut mount = Mount::new(
            config.clone(),
            self.inventory.clone(),
            self.command_tx.clone(),
        )
        .await;
        if let Err(e) = mount.start().await {
            tracing::error!(target: "drive", error = %e, "Failed to start drive");
            return Err(e).context("Failed to start drive");
        }

        let mount_arc = Arc::new(mount);
        mount_arc.spawn_command_processor(mount_arc.clone()).await;
        mount_arc
            .spawn_remote_event_processor(mount_arc.clone())
            .await;
        mount_arc.spawn_props_refresh_task().await;
        let id = mount_arc.id.clone();
        write_guard.insert(id.clone(), mount_arc);
        Ok(id)
    }

    // Search drive by child file path.
    // Child path can be up to the sync root path.
    pub async fn search_drive_by_child_path(&self, path: &str) -> Option<Arc<Mount>> {
        let read_guard = self.drives.read().await;

        // Convert the input path to an absolute PathBuf for comparison
        let target_path = PathBuf::from(path);
        let target_path = match target_path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // If canonicalize fails (e.g., path doesn't exist), try to work with the original path
                target_path
            }
        };

        // Iterate through all drives and check if the target path is under their sync root
        for (_, mount) in read_guard.iter() {
            let sync_path = mount.get_sync_path().await;

            // Normalize the sync path
            let sync_path = match sync_path.canonicalize() {
                Ok(p) => p,
                Err(_) => sync_path,
            };

            // Check if target_path starts with sync_path (is a child of sync_path)
            if target_path.starts_with(&sync_path) {
                return Some(mount.clone());
            }
        }

        None
    }

    /// Remove a drive by ID
    pub async fn remove_drive(&self, _id: &str) -> Result<Option<DriveConfig>> {
        // let mut write_guard = self.drives.write().await;
        // Ok(write_guard.remove(id).map(async|mount| mount.get_config().await))
        Err(anyhow::anyhow!("Not implemented"))
    }

    /// Get a drive by ID
    pub async fn get_drive(&self, id: &str) -> Option<Arc<Mount>> {
        let read_guard = self.drives.read().await;
        read_guard.get(id).cloned()
    }

    /// List all drives
    pub async fn list_drives(&self) -> Vec<DriveConfig> {
        // let read_guard = self.drives.read().await;
        // read_guard
        //     .values()
        //     .map(|mount| mount.get_config())
        //     .collect()
        Vec::new()
    }

    /// Update drive configuration
    pub async fn update_drive(&self, _id: &str, _config: DriveConfig) -> Result<()> {
        // let mut write_guard = self.drives.write().await;
        // if write_guard.contains_key(id) {
        //     // write_guard.insert(id.to_string(), Mount::new(config.clone()));
        //     Ok(())
        // } else {
        //     anyhow::bail!("Drive not found: {}", id)
        // }
        Err(anyhow::anyhow!("Not implemented"))
    }

    /// Enable/disable a drive
    pub async fn set_drive_enabled(&self, _id: &str, _enabled: bool) -> Result<()> {
        Err(anyhow::anyhow!("Not implemented"))
    }

    /// Placeholder: Start syncing a drive
    pub async fn start_sync(&self, _id: &str) -> Result<()> {
        Err(anyhow::anyhow!("Not implemented"))
    }

    /// Placeholder: Stop syncing a drive
    pub async fn stop_sync(&self, _id: &str) -> Result<()> {
        Err(anyhow::anyhow!("Not implemented"))
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

    /// Get a summary of the current status including all drives and recent tasks.
    ///
    /// # Arguments
    /// * `drive_id` - Optional drive ID to filter tasks. If None, returns tasks from all drives.
    ///                Note: drives list always returns all drives regardless of this filter.
    pub async fn get_status_summary(&self, drive_id: Option<&str>) -> Result<StatusSummary> {
        // Get all drive configs (unfiltered)
        let read_guard = self.drives.read().await;
        let mut drives = Vec::with_capacity(read_guard.len());
        for mount in read_guard.values() {
            drives.push(mount.get_config().await);
        }

        // Query recent tasks from inventory (filtered by drive_id if provided)
        let recent_tasks = self
            .inventory
            .query_recent_tasks(drive_id)
            .context("Failed to query recent tasks")?;

        // Collect running task progress from all task queues
        // Build a map of task_id -> TaskProgress for quick lookup
        let mut progress_map: HashMap<String, TaskProgress> = HashMap::new();

        if let Some(drive_filter) = drive_id {
            // If filtering by drive, only get progress from that drive's task queue
            if let Some(mount) = read_guard.get(drive_filter) {
                for progress in mount.task_queue.ongoing_progress().await {
                    progress_map.insert(progress.task_id.clone(), progress);
                }
            }
        } else {
            // Get progress from all drives
            for mount in read_guard.values() {
                for progress in mount.task_queue.ongoing_progress().await {
                    progress_map.insert(progress.task_id.clone(), progress);
                }
            }
        }

        // Merge progress info into active tasks
        let active_tasks: Vec<TaskWithProgress> = recent_tasks
            .active
            .into_iter()
            .map(|task| {
                let progress = progress_map.remove(&task.id);
                TaskWithProgress { task, live_progress: progress }
            })
            .collect();

        Ok(StatusSummary {
            drives,
            active_tasks,
            finished_tasks: recent_tasks.finished,
        })
    }

    /// Get drive status by sync root ID (CFAPI ID) for the Windows Shell Status UI.
    ///
    /// # Arguments
    /// * `syncroot_id` - The sync root ID string (e.g., "cloudreve<hash>!S-1-5-21-xxx!user_id")
    ///
    /// # Returns
    /// * `Ok(Some(DriveStatusUI))` - Drive status if found
    /// * `Ok(None)` - No drive found with the given sync root ID
    /// * `Err` - An error occurred
    pub async fn get_drive_status_by_syncroot_id(
        &self,
        syncroot_id: &str,
    ) -> Result<Option<DriveStatusUI>> {
        let read_guard = self.drives.read().await;

        // Find the drive with matching sync root ID
        let mut found_mount: Option<&Arc<Mount>> = None;
        for mount in read_guard.values() {
            let config = mount.config.read().await;
            if let Some(ref sync_root) = config.sync_root_id {
                let sync_root_str = sync_root.to_os_string().to_string_lossy().to_string();
                if sync_root_str == syncroot_id {
                    drop(config);
                    found_mount = Some(mount);
                    break;
                }
            }
        }

        let mount = match found_mount {
            Some(m) => m,
            None => {
                tracing::debug!(target: "drive::manager", syncroot_id = %syncroot_id, "No drive found for sync root ID");
                return Ok(None);
            }
        };

        let config = mount.get_config().await;
        let drive_id = &config.id;

        // Get capacity from drive props
        let capacity = match mount.get_drive_props() {
            Ok(Some(props)) => props.capacity.map(|cap| {
                let percentage = if cap.total > 0 {
                    (cap.used as f64 / cap.total as f64) * 100.0
                } else {
                    0.0
                };
                CapacitySummary {
                    total: cap.total,
                    used: cap.used,
                    label: format!(
                        "{} / {} ({:.1}%)",
                        format_bytes(cap.used),
                        format_bytes(cap.total),
                        percentage
                    ),
                }
            }),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(target: "drive::manager", drive_id = %drive_id, error = %e, "Failed to get drive props");
                None
            }
        };

        // Build profile URL: siteURL/profile/<user_id>?user_hint=<user_id>
        let profile_url = format!(
            "{}/profile/{}?user_hint={}",
            config.instance_url.trim_end_matches('/'),
            config.user_id,
            config.user_id
        );

        // Build settings URL: siteURL/settings?user_hint=<user_id>
        let settings_url = format!(
            "{}/settings?user_hint={}",
            config.instance_url.trim_end_matches('/'),
            config.user_id
        );

        let storage_url = format!(
            "{}/settings?tab=storage&user_hint={}",
            config.instance_url.trim_end_matches('/'),
            config.user_id
        );

        // Determine sync status based on active tasks
        let active_task_count = match self.inventory.query_recent_tasks(Some(drive_id)) {
            Ok(tasks) => tasks.active.len(),
            Err(e) => {
                tracing::warn!(target: "drive::manager", drive_id = %drive_id, error = %e, "Failed to query recent tasks");
                0
            }
        };

        let sync_status = if active_task_count > 0 {
            SyncStatus::Syncing
        } else {
            SyncStatus::InSync
        };

        Ok(Some(DriveStatusUI {
            name: config.name.clone(),
            raw_icon_path: config.raw_icon_path.clone(),
            capacity,
            profile_url,
            settings_url,
            storage_url,
            sync_status,
            active_task_count,
        }))
    }

    /// Get a command sender for external code to send commands to the manager
    pub fn get_command_sender(&self) -> mpsc::UnboundedSender<ManagerCommand> {
        self.command_tx.clone()
    }

    /// Spawn the command processor task
    pub async fn spawn_command_processor(self: &Arc<Self>) {
        let mut command_rx_guard = self.command_rx.lock().await;
        if let Some(command_rx) = command_rx_guard.take() {
            let manager = self.clone();
            let handle = tokio::spawn(async move {
                Self::process_commands(manager, command_rx).await;
            });
            *self.processor_handle.lock().await = Some(handle);
        }
    }

    /// Process commands from external sources asynchronously
    async fn process_commands(
        manager: Arc<Self>,
        mut command_rx: mpsc::UnboundedReceiver<ManagerCommand>,
    ) {
        tracing::info!(target: "drive::manager", "Command processor started");

        while let Some(command) = command_rx.recv().await {
            tracing::trace!(target: "drive::manager", command = ?command, "Processing command");
            let manager = manager.clone();
            match command {
                ManagerCommand::ViewOnline { path } => {
                    let path = path.clone();
                    spawn(async move {
                        let path = path.clone();
                        let result = manager.handle_view_online(path.clone()).await;
                        // TODO: handle result in UI
                        tracing::debug!(target: "drive::manager", path = %path.display(), result = ?result, "ViewOnline command result");
                    });
                }
                ManagerCommand::PersistConfig => {
                    let result = manager.persist().await;
                    if let Err(e) = result {
                        tracing::error!(target: "drive::manager", error = %e, "Failed to persist config");
                    }
                }
                ManagerCommand::SyncNow { paths, mode } => {
                    let paths = paths.clone();
                    if paths.len() < 1 {
                        tracing::error!(target: "drive::manager", "No paths provided for sync command");
                        return;
                    }
                    spawn(async move {
                        let drive = manager
                            .search_drive_by_child_path(
                                paths.get(0).unwrap().to_str().unwrap_or(""),
                            )
                            .await;
                        if let Some(drive) = drive {
                            let _ = drive.command_tx.send(MountCommand::Sync {
                                local_paths: paths,
                                mode: mode,
                            });
                        } else {
                            tracing::error!(target: "drive::manager", "No drive found for path: {:?}", paths.get(0).unwrap());
                        }
                    });
                }
                ManagerCommand::GenerateThumbnail { path, response } => {
                    let path = path.clone();
                    spawn(async move {
                        let drive = manager
                            .search_drive_by_child_path(path.to_str().unwrap_or(""))
                            .await;
                        if let Some(drive) = drive {
                            let result = drive.generate_thumbnail(path.clone()).await;
                            if let Err(e) = result {
                                tracing::error!(target: "drive::manager", error = %e, "Failed to generate thumbnail");
                                let _ = response.send(Err(e));
                                return;
                            }

                            let _ = response.send(result);
                            return;
                        }

                        let _ = response
                            .send(Err(anyhow::anyhow!("No drive found for path: {:?}", path)));
                    });
                }
                ManagerCommand::ResolveConflict {
                    drive_id,
                    file_id,
                    path,
                    action,
                } => {
                    spawn(async move {
                        let drive = manager.get_drive(&drive_id).await;
                        if let Some(drive) = drive {
                            let result = drive.resolve_conflict(action, file_id,path).await;
                            if let Err(e) = result {
                                tracing::error!(target: "drive::manager", error = %e, "Failed to resolve conflict");
                            }
                        } else {
                            tracing::error!(target: "drive::manager", "No drive found for drive_id: {:?}", drive_id);
                        }
                    });
                }
                ManagerCommand::ShowConflictToast { path } => {
                    let path = path.clone();
                    spawn(async move {
                        let result = manager.handle_show_conflict_toast(path.clone()).await;
                        if let Err(e) = result {
                            tracing::error!(target: "drive::manager", path = %path.display(), error = %e, "Failed to show conflict toast");
                        }
                    });
                }
                ManagerCommand::GetDriveStatusUI { syncroot_id, response } => {
                    spawn(async move {
                        let result = manager.get_drive_status_by_syncroot_id(&syncroot_id).await;
                        let _ = response.send(result);
                    });
                }
                ManagerCommand::OpenProfileUrl { syncroot_id } => {
                    spawn(async move {
                        let result = manager.handle_open_profile_url(&syncroot_id).await;
                        if let Err(e) = result {
                            tracing::error!(target: "drive::manager", syncroot_id = %syncroot_id, error = %e, "Failed to open profile URL");
                        }
                    });
                }
                ManagerCommand::OpenStorageDetailsUrl { syncroot_id } => {
                    spawn(async move {
                        let result = manager.handle_open_storage_details_url(&syncroot_id).await;
                        if let Err(e) = result {
                            tracing::error!(target: "drive::manager", syncroot_id = %syncroot_id, error = %e, "Failed to open storage details URL");
                        }
                    });
                }
                ManagerCommand::OpenSyncStatusWindow => {
                    manager.event_broadcaster.open_sync_status_window();
                }
                ManagerCommand::OpenSettingsWindow => {
                    manager.event_broadcaster.open_settings_window();
                }
            }
        }

        tracing::info!(target: "drive::manager", "Command processor stopped");
    }

    /// Handle ViewOnline command
    async fn handle_view_online(&self, path: PathBuf) -> Result<()> {
        tracing::debug!(target: "drive::manager", path = %path.display(), "ViewOnline command");

        // Find the drive that contains this path
        let mount = self
            .search_drive_by_child_path(path.to_str().unwrap_or(""))
            .await
            .ok_or_else(|| anyhow::anyhow!("No drive found for path: {:?}", path))?;

        let file_meta = self
            .inventory
            .query_by_path(path.to_str().unwrap_or(""))
            .context("Failed to query file metadata")?;

        let config = mount.get_config().await;
        let (sync_path, remote_path) =
            { (config.sync_path.clone(), config.remote_path.to_string()) };
        let uri = local_path_to_cr_uri(path.clone(), sync_path, remote_path)
            .context("failed to convert local path to cloudreve uri")?
            .to_string();

        // Determine which URL to open
        let url = match file_meta {
            // If no metadata, assume it's the sync root, open folder
            None => view_online_url(&config.remote_path, None, &config)?,
            Some(ref meta) if meta.is_folder => view_online_url(&uri, None, &config)?,
            Some(ref meta) => {
                use cloudreve_api::models::uri::CrUri;
                let parent_path = CrUri::new(&uri)?.parent()?.to_string();
                view_online_url(&parent_path, Some(&uri), &config)?
            }
        };

        open::that(url)?;
        Ok(())
    }

    /// Handle ShowConflictToast command
    async fn handle_show_conflict_toast(&self, path: PathBuf) -> Result<()> {
        tracing::debug!(target: "drive::manager", path = %path.display(), "ShowConflictToast command");

        // Find the drive that contains this path
        let mount = self
            .search_drive_by_child_path(path.to_str().unwrap_or(""))
            .await
            .ok_or_else(|| anyhow::anyhow!("No drive found for path: {:?}", path))?;

        // Query inventory for file metadata
        let file_meta = self
            .inventory
            .query_by_path(path.to_str().unwrap_or(""))
            .context("Failed to query file metadata")?
            .ok_or_else(|| anyhow::anyhow!("File not found in inventory: {:?}", path))?;

        let config = mount.get_config().await;

        // Send the conflict toast
        send_conflict_toast(&config.id, &path, file_meta.id);

        Ok(())
    }

    /// Handle OpenProfileUrl command - opens user profile page in browser
    async fn handle_open_profile_url(&self, syncroot_id: &str) -> Result<()> {
        tracing::debug!(target: "drive::manager", syncroot_id = %syncroot_id, "OpenProfileUrl command");

        let status = self
            .get_drive_status_by_syncroot_id(syncroot_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("No drive found for syncroot_id: {}", syncroot_id))?;

        open::that(&status.profile_url)?;
        Ok(())
    }

    /// Handle OpenStorageDetailsUrl command - opens storage/capacity page in browser
    async fn handle_open_storage_details_url(&self, syncroot_id: &str) -> Result<()> {
        tracing::debug!(target: "drive::manager", syncroot_id = %syncroot_id, "OpenStorageDetailsUrl command");

        let status = self
            .get_drive_status_by_syncroot_id(syncroot_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("No drive found for syncroot_id: {}", syncroot_id))?;

        // Open the profile URL which shows storage details
        open::that(&status.storage_url)?;
        Ok(())
    }

    pub async fn shutdown(&self) {
        tracing::info!(target: "drive::manager", "Shutting down DriveManager");

        // Close the command channel to signal the processor task to stop
        drop(self.command_tx.clone());

        // Wait for the processor task to finish
        if let Some(handle) = self.processor_handle.lock().await.take() {
            tracing::debug!(target: "drive::manager", "Waiting for command processor to finish");
            handle.abort();
        }

        let write_guard = self.drives.write().await;
        for (_, mount) in write_guard.iter() {
            mount.shutdown().await;
        }
        tracing::info!(target: "drive", "All drives shutdown");
    }
}

/// Format bytes into a human-readable string (e.g., "1.5 GB")
fn format_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let bytes_f = bytes as f64;

    if bytes_f >= TB {
        format!("{:.1} TB", bytes_f / TB)
    } else if bytes_f >= GB {
        format!("{:.1} GB", bytes_f / GB)
    } else if bytes_f >= MB {
        format!("{:.1} MB", bytes_f / MB)
    } else if bytes_f >= KB {
        format!("{:.1} KB", bytes_f / KB)
    } else {
        format!("{} B", bytes)
    }
}

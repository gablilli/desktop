use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::{
    cfapi::{
        metadata::{self, Metadata},
        placeholder::{ConvertOptions, LocalFileInfo, OpenOptions, Placeholder, UpdateOptions},
    },
    drive::{
        sync::cloud_file_to_metadata_entry,
        utils::{local_path_to_cr_uri, notify_shell_change},
    },
    inventory::{FileMetadata, InventoryDb, MetadataEntry, TaskStatus, TaskUpdate},
    tasks::{TaskQueue, queue::QueuedTask},
    uploader::{ProgressCallback, ProgressUpdate, UploadParams, Uploader, UploaderConfig},
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cloudreve_api::{
    Client,
    api::ExplorerApi,
    models::explorer::{CreateFileService, FileResponse, file_type},
};
use nt_time::FileTime;
use tokio_util::sync::CancellationToken;
use tracing::{self, debug, error, info, warn};
use uuid::Uuid;
use windows::Win32::UI::Shell::{SHCNE_CREATE, SHCNE_MKDIR};

/// Progress reporter that updates task state
struct TaskProgressReporter {
    task_id: String,
    inventory: Arc<InventoryDb>,
}

impl TaskProgressReporter {
    fn new(task_id: String, inventory: Arc<InventoryDb>) -> Self {
        Self { task_id, inventory }
    }
}

impl ProgressCallback for TaskProgressReporter {
    fn on_progress(&self, update: ProgressUpdate) {
        // Update task progress in inventory
        let task_update = TaskUpdate {
            progress: Some(update.progress),
            processed_bytes: Some(update.uploaded as i64),
            total_bytes: Some(update.total_size as i64),
            ..Default::default()
        };

        if let Err(e) = self.inventory.update_task(&self.task_id, task_update) {
            warn!(
                target: "tasks::upload",
                task_id = %self.task_id,
                error = %e,
                "Failed to persist upload progress"
            );
        }
    }
}

pub struct UploadTask<'a> {
    inventory: Arc<InventoryDb>,
    cr_client: Arc<Client>,
    drive_id: &'a str,
    sync_path: PathBuf,
    remote_base: String,
    task: &'a QueuedTask,
    local_file: Option<LocalFileInfo>,
    inventory_meta: Option<FileMetadata>,
    cancel_token: CancellationToken,
}

impl<'a> UploadTask<'a> {
    pub fn new(
        inventory: Arc<InventoryDb>,
        cr_client: Arc<Client>,
        drive_id: &'a str,
        task: &'a QueuedTask,
        sync_path: PathBuf,
        remote_base: String,
    ) -> Self {
        Self {
            inventory,
            cr_client,
            drive_id,
            local_file: None,
            inventory_meta: None,
            task,
            sync_path,
            remote_base,
            cancel_token: CancellationToken::new(),
        }
    }

    /// Set the cancellation token
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }

    // Upload a local file/folder to cloud
    pub async fn execute(&mut self) -> Result<()> {
        // Get local file info
        let local_file = LocalFileInfo::from_path(&self.task.payload.local_path)
            .context("failed to get local file info")?;
        if !local_file.exists {
            info!(
                target: "tasks::upload",
                task_id = %self.task.task_id,
                local_path = %self.task.payload.local_path_display(),
                "Local file does not exist, skipping upload"
            );
            return Ok(());
        }

        if local_file.in_sync() && !local_file.is_directory() {
            info!(
                target: "tasks::upload",
                task_id = %self.task.task_id,
                local_path = %self.task.payload.local_path_display(),
                "Local file is in sync, skipping upload"
            );
            return Ok(());
        }

        let is_directory = local_file.is_directory;
        let file_size = local_file.file_size.unwrap_or(0);
        self.local_file = Some(local_file);

        // Get inventory meta
        let path_str = self
            .task
            .payload
            .local_path
            .to_str()
            .context("failed to get local path as str")?;
        self.inventory_meta = self
            .inventory
            .query_by_path(path_str)
            .context("failed to get inventory meta")?;

        // Handle empty files and directories separately
        if is_directory || file_size == 0 {
            self.create_empty_file_or_folder().await?;
        } else {
            // Use the new uploader for regular files
            self.upload_file_with_uploader().await?;
        }

        Ok(())
    }

    /// Upload a file using the new uploader module
    async fn upload_file_with_uploader(&mut self) -> Result<()> {
        let local_file = self.local_file.as_ref().unwrap();
        let file_size = local_file.file_size.unwrap_or(0);

        info!(
            target: "tasks::upload",
            task_id = %self.task.task_id,
            local_path = %self.task.payload.local_path_display(),
            file_size = file_size,
            "Starting file upload"
        );

        // Get remote URI
        let uri = local_path_to_cr_uri(
            self.task.payload.local_path.clone(),
            self.sync_path.clone(),
            self.remote_base.clone(),
        )
        .context("failed to convert local path to cloudreve uri")?
        .to_string();

        // Get storage policy ID from the credential in existing session or use default
        // For now, we'll need to get it from the file info or use a default
        let policy_id = self.get_storage_policy_id().await?;

        // Create upload params
        let params = UploadParams {
            local_path: self.task.payload.local_path.clone(),
            remote_uri: uri,
            policy_id,
            file_size,
            mime_type: None, // Could be detected from file extension
            last_modified: local_file
                .last_modified
                .map(|t| t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64),
            overwrite: false, // TODO: Get from task config
            task_id: self.task.task_id.clone(),
            drive_id: self.drive_id.to_string(),
        };

        // Create uploader configuration
        let config = UploaderConfig::default();

        // Create uploader
        let uploader = Uploader::new(self.cr_client.clone(), self.inventory.clone(), config)
            .with_cancel_token(self.cancel_token.clone());

        // Create progress reporter
        let progress = TaskProgressReporter::new(self.task.task_id.clone(), self.inventory.clone());

        // Execute upload
        uploader
            .upload(params, progress)
            .await
            .map_err(|e| anyhow::anyhow!("Upload failed: {}", e))?;

        // Update local file placeholder status after successful upload
        self.finalize_upload().await?;

        Ok(())
    }

    /// Get storage policy ID for the upload
    async fn get_storage_policy_id(&self) -> Result<String> {
        // Try to get policy from inventory metadata
        if let Some(ref meta) = self.inventory_meta {
            if let Some(ref props) = meta.props {
                if let Some(policy_id) = props.get("storage_policy_id").and_then(|v| v.as_str()) {
                    return Ok(policy_id.to_string());
                }
            }
        }

        // Try to get from parent folder or use default
        // For now, get the first available policy
        let policies = self
            .cr_client
            .get_storage_policy_options()
            .await
            .context("failed to get storage policies")?;

        policies
            .first()
            .map(|p| p.id.clone())
            .ok_or_else(|| anyhow::anyhow!("No storage policies available"))
    }

    /// Finalize upload by updating local file placeholder
    async fn finalize_upload(&mut self) -> Result<()> {
        // Get file info from server to confirm upload
        let uri = local_path_to_cr_uri(
            self.task.payload.local_path.clone(),
            self.sync_path.clone(),
            self.remote_base.clone(),
        )
        .context("failed to convert local path to cloudreve uri")?
        .to_string();

        let file_info = self
            .cr_client
            .get_file_info(&cloudreve_api::models::explorer::GetFileInfoService {
                uri: Some(uri),
                id: None,
                extended: None,
                folder_summary: None,
            })
            .await
            .context("failed to get file info after upload")?;

        //self.file_uploaded(&file_info)
        Ok(())
    }

    async fn create_empty_file_or_folder(&mut self) -> Result<()> {
        info!(
            target: "tasks::upload",
            task_id = %self.task.task_id,
            local_path = %self.task.payload.local_path_display(),
            "Creating empty file/folder"
        );
        let local_file = self.local_file.as_ref().unwrap();
        let uri = local_path_to_cr_uri(
            self.task.payload.local_path.clone(),
            self.sync_path.clone(),
            self.remote_base.clone(),
        )
        .context("failed to convert local path to cloudreve uri")?
        .to_string();

        // Create file in remote
        let res = self
            .cr_client
            .create_file(&CreateFileService {
                uri,
                file_type: if local_file.is_directory {
                    "folder".to_string()
                } else {
                    "file".to_string()
                },
                err_on_conflict: Some(!local_file.is_directory),
                metadata: None,
            })
            .await;
        match res {
            Ok(folder) => self.file_uploaded(&folder),
            Err(e) => Err(e.into()),
        }
    }

    fn file_uploaded(&self, file: &FileResponse) -> Result<()> {
        info!(
            target: "tasks::upload",
            task_id = %self.task.task_id,
            local_path = %self.task.payload.local_path_display(),
            "File uploaded"
        );

        // Upsert inventory
        self.inventory
            .upsert(
                &cloud_file_to_metadata_entry(
                    file,
                    &Uuid::parse_str(self.drive_id).context("failed to parse drive id")?,
                    &self.task.payload.local_path,
                )
                .context("failed to convert cloud file to metadata entry")?,
            )
            .context("failed to upsert inventory")?;

        let mut local_handle = OpenOptions::new()
            .write_access()
            .exclusive()
            .open(&self.task.payload.local_path)
            .context("failed to open local file")?;

        // Convert to placeholder if it's not
        if !self.local_file.as_ref().unwrap().is_placeholder() {
            info!(
                target: "tasks::upload",
                task_id = %self.task.task_id,
                local_path = %self.task.payload.local_path_display(),
                "Converting to placeholder"
            );
            local_handle
                .convert_to_placeholder(ConvertOptions::default().mark_in_sync(), None)
                .context("failed to convert to placeholder")?;

            drop(local_handle);
            local_handle = OpenOptions::new()
                .write_access()
                .exclusive()
                .open(&self.task.payload.local_path)
                .context("failed to open local file")?;
        }

        // Sync placeholder info with cloud
        let created_at =
            FileTime::from_unix_time(file.created_at.parse::<DateTime<Utc>>()?.timestamp())?;
        let last_modified =
            FileTime::from_unix_time(file.updated_at.parse::<DateTime<Utc>>()?.timestamp())?;
        let mut metadata = Metadata::default();
        if file.file_type == file_type::FILE {
            metadata = metadata.size(file.size as u64);
        }
        local_handle
            .update(
                UpdateOptions::default()
                    .mark_in_sync()
                    .has_children()
                    .metadata(
                        metadata
                            .created(created_at)
                            .accessed(last_modified)
                            .written(last_modified)
                            .changed(last_modified),
                    ),
                None,
            )
            .context("failed to sync placeholder info with cloud")?;

        // Notify shell change
        notify_shell_change(
            &self.task.payload.local_path,
            if self.local_file.as_ref().unwrap().is_directory {
                SHCNE_MKDIR
            } else {
                SHCNE_CREATE
            },
        )
        .context("failed to notify shell change")?;
        Ok(())
    }
}

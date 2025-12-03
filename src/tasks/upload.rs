use std::{
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::{
    cfapi::{
        metadata::{self, Metadata},
        placeholder::{ConvertOptions, LocalFileInfo, OpenOptions, Placeholder, UpdateOptions},
    },
    drive::{
        placeholder::CrPlaceholder,
        sync::cloud_file_to_metadata_entry,
        utils::{local_path_to_cr_uri, notify_shell_change},
    },
    inventory::{FileMetadata, InventoryDb, MetadataEntry, TaskStatus, TaskUpdate},
    tasks::{TaskQueue, queue::QueuedTask},
    uploader::{ProgressCallback, ProgressUpdate, UploadParams, Uploader, UploaderConfig},
};
use anyhow::{Context, Error, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use cloudreve_api::{
    ApiError, Client,
    api::ExplorerApi,
    error::ErrorCode,
    models::explorer::{CreateFileService, FileResponse, FileUpdateService, file_type},
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
    local_file: Option<CrPlaceholder>,
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
        let placeholder_file = CrPlaceholder::new(
            &self.task.payload.local_path,
            self.sync_path.clone(),
            Uuid::from_str(self.drive_id)?,
        );
        if !placeholder_file.local_file_info.exists {
            info!(
                target: "tasks::upload",
                task_id = %self.task.task_id,
                local_path = %self.task.payload.local_path_display(),
                "Local file does not exist, skipping upload"
            );
            return Ok(());
        }

        if placeholder_file.local_file_info.in_sync()
            && !placeholder_file.local_file_info.is_directory()
        {
            info!(
                target: "tasks::upload",
                task_id = %self.task.task_id,
                local_path = %self.task.payload.local_path_display(),
                "Local file is in sync, skipping upload"
            );
            return Ok(());
        }

        let is_directory = placeholder_file.local_file_info.is_directory;
        let file_size = placeholder_file.local_file_info.file_size.unwrap_or(0);
        self.local_file = Some(placeholder_file);

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

        // clear file error state
        // Mark file as error state
        if let Err(e) = self
            .local_file
            .as_mut()
            .unwrap()
            .update_sync_error_state(false)
        {
            warn!(target: "tasks::upload", task_id = %self.task.task_id, local_path = %self.task.payload.local_path_display(), error = ?e, "Failed to clear sync error state");
        }

        // Handle empty files and directories separately
        let upload_res = match (is_directory, file_size == 0, self.inventory_meta.is_none()) {
            (true, _, _) => self.create_empty_file_or_folder().await,
            (false, true, true) => self.create_empty_file_or_folder().await,
            (false, true, false) => self.clear_file_content().await,
            (false, false, _) => self.upload_file_with_uploader().await,
        };

        self.handle_error(upload_res).await
    }

    async fn handle_error(&mut self, r: Result<()>) -> Result<()> {
        match r {
            Ok(()) => Ok(()),
            Err(e) => {
                // Check if the error is an ApiError with StaleVersion (40076)
                // The error might be wrapped with anyhow context, so check the chain
                let is_stale_version = e.chain().any(|cause| {
                    if let Some(api_err) = cause.downcast_ref::<ApiError>() {
                        matches!(
                            api_err,
                            ApiError::ApiError { code, .. }
                                if *code == ErrorCode::StaleVersion as i32
                        )
                    } else {
                        false
                    }
                });

                if is_stale_version {
                    warn!(
                        target: "tasks::upload",
                        task_id = %self.task.task_id,
                        local_path = %self.task.payload.local_path_display(),
                        "Stale version detected, server has newer version, skipping upload"
                    );
                }

                // Mark file as error state
                if let Err(e) = self
                    .local_file
                    .as_mut()
                    .unwrap()
                    .update_sync_error_state(true)
                {
                    warn!(target: "tasks::upload", task_id = %self.task.task_id, local_path = %self.task.payload.local_path_display(), error = ?e, "Failed to update sync error state");
                }
                Err(e)
            }
        }
    }

    async fn clear_file_content(&mut self) -> Result<()> {
        info!(
            target: "tasks::upload",
            task_id = %self.task.task_id,
            local_path = %self.task.payload.local_path_display(),
            "Clearing file content with update request"
        );

        let uri = local_path_to_cr_uri(
            self.task.payload.local_path.clone(),
            self.sync_path.clone(),
            self.remote_base.clone(),
        )
        .context("failed to convert local path to cloudreve uri")?
        .to_string();
        let etag = self.inventory_meta.as_ref().unwrap().etag.clone();
        let res = self
            .cr_client
            .update_file(
                &FileUpdateService {
                    uri,
                    previous: Some(etag),
                },
                Bytes::new(),
            )
            .await;

        match res {
            Ok(file) => self.file_uploaded(&file),
            Err(e) => Err(e.into()),
        }
    }

    /// Upload a file using the new uploader module
    async fn upload_file_with_uploader(&mut self) -> Result<()> {
        let local_file = self.local_file.as_ref().unwrap();
        let file_size = local_file.local_file_info.file_size.unwrap_or(0);
        let is_new_file = self.inventory_meta.is_none();

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
        // Create upload params
        let params = UploadParams {
            local_path: self.task.payload.local_path.clone(),
            remote_uri: uri,
            file_size,
            mime_type: None, // Could be detected from file extension
            last_modified: local_file
                .local_file_info
                .last_modified
                .map(|t| t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64),
            overwrite: !is_new_file,
            previous_version: self
                .inventory_meta
                .as_ref()
                .map(|m| m.etag.clone())
                .unwrap_or_default(),
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
            .context("failed to upload file")?;
        
        // Update local file placeholder status after successful upload
        self.finalize_upload().await?;

        Ok(())
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
        let local_file = &self.local_file.as_ref().unwrap().local_file_info;
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

    fn file_uploaded(&mut self, file: &FileResponse) -> Result<()> {
        info!(
            target: "tasks::upload",
            task_id = %self.task.task_id,
            local_path = %self.task.payload.local_path_display(),
            "File uploaded"
        );

        self.local_file = Some(
            self.local_file
                .take()
                .unwrap()
                .with_mark_no_children(true)
                .with_remote_file(file),
        );

        self.local_file
            .as_mut()
            .unwrap()
            .commit(self.inventory.clone())?;
        Ok(())
    }
}

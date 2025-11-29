use std::sync::Arc;

use crate::{
    cfapi::{
        metadata::{self, Metadata},
        placeholder::{ConvertOptions, LocalFileInfo, OpenOptions, Placeholder, UpdateOptions},
    },
    drive::sync::cloud_file_to_metadata_entry,
    inventory::{FileMetadata, InventoryDb, MetadataEntry},
    tasks::{TaskQueue, queue::QueuedTask},
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cloudreve_api::{
    Client,
    api::ExplorerApi,
    models::explorer::{CreateFileService, FileResponse, file_type},
};
use nt_time::FileTime;
use tracing;
use uuid::Uuid;

pub struct UploadTask<'a> {
    inventory: Arc<InventoryDb>,
    cr_client: Arc<Client>,
    drive_id: &'a str,

    task: &'a QueuedTask,
    local_file: Option<LocalFileInfo>,
    inventory_meta: Option<FileMetadata>,
}

impl<'a> UploadTask<'a> {
    pub fn new(
        inventory: Arc<InventoryDb>,
        cr_client: Arc<Client>,
        drive_id: &'a str,
        task: &'a QueuedTask,
    ) -> Self {
        Self {
            inventory,
            cr_client,
            drive_id,
            local_file: None,
            inventory_meta: None,
            task,
        }
    }
    // Upload a local file/folder to cloud
    pub async fn execute(&mut self) -> Result<()> {
        // Get local file info
        let local_file = LocalFileInfo::from_path(&self.task.payload.local_path)
            .context("failed to get local file info")?;
        if !local_file.exists {
            tracing::info!(
                target: "tasks::upload",
                task_id = %self.task.task_id,
                local_path = %self.task.payload.local_path_display(),
                "Local file does not exist, skipping upload"
            );
            return Ok(());
        }

        if local_file.in_sync() {
            tracing::info!(
                target: "tasks::upload",
                task_id = %self.task.task_id,
                local_path = %self.task.payload.local_path_display(),
                "Local file is in sync, skipping upload"
            );
            return Ok(());
        }

        let is_directory = local_file.is_directory;
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

        if is_directory || self.local_file.as_ref().unwrap().file_size.unwrap_or(0) == 0_u64 {
            self.create_empty_file_or_folder().await?;
        }

        Ok(())
    }

    async fn create_empty_file_or_folder(&mut self) -> Result<()> {
        tracing::info!(
            target: "tasks::upload",
            task_id = %self.task.task_id,
            local_path = %self.task.payload.local_path_display(),
            "Creating empty file/folder"
        );
        let local_file = self.local_file.as_ref().unwrap();
        // Create file in remote
        let res = self
            .cr_client
            .create_file(&CreateFileService {
                uri: self
                    .task
                    .payload
                    .remote_uri
                    .clone()
                    .context("remote uri not found")?,
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
        tracing::info!(
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
            .open(&self.task.payload.local_path)
            .context("failed to open local file")?;

        // Convert to placeholder if it's not
        if !self.local_file.as_ref().unwrap().is_placeholder() {
            local_handle.convert_to_placeholder(ConvertOptions::default().mark_in_sync(), None)?;
        }

        // Sync placeholder info with cloud
        let created_at =
            FileTime::from_unix_time(file.created_at.parse::<DateTime<Utc>>()?.timestamp())?;
        let last_modified =
            FileTime::from_unix_time(file.updated_at.parse::<DateTime<Utc>>()?.timestamp())?;
        let metadata = if file.file_type == file_type::FOLDER {
            Metadata::directory()
        } else {
            Metadata::file()
        };
        local_handle
            .update(
                UpdateOptions::default().mark_in_sync().metadata(
                    metadata
                        .changed(last_modified)
                        .created(created_at)
                        .written(last_modified)
                        .size(file.size as u64),
                ),
                None,
            )
            .context("failed to sync placeholder info with cloud")?;

        Ok(())
    }
}

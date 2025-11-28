use crate::{
    cfapi::{metadata::Metadata, placeholder_file::PlaceholderFile},
    drive::utils::notify_shell_change,
    inventory::{FileMetadata, InventoryDb, MetadataEntry},
};
use anyhow::{Context, Result};
use chrono::DateTime;
use cloudreve_api::models::explorer::{FileResponse, file_type};
use nt_time::FileTime;
use std::{ffi::OsString, path::PathBuf, sync::Arc};
use uuid::Uuid;
use widestring::U16CString;
use windows::Win32::UI::Shell::{
    SHCNE_CREATE, SHCNE_DELETE, SHCNE_MKDIR, SHCNF_PATHW, SHChangeNotify,
};

pub struct CrPlaceholder {
    local_path: PathBuf,
    sync_root: PathBuf,
    drive_id: Uuid,
    file_meta: Option<FileMetadata>,
    options: u32,
}

enum CrPlaceholderOptions {
    InvalidateAllRange = 1 << 0,
}

impl CrPlaceholder {
    pub fn new(local_path: PathBuf, sync_root: PathBuf, drive_id: Uuid) -> Self {
        Self {
            local_path,
            sync_root,
            drive_id,
            file_meta: None,
            options: 0,
        }
    }

    pub fn invalidate_all_range(mut self) -> Self {
        self.options |= CrPlaceholderOptions::InvalidateAllRange as u32;
        self
    }

    // Commit changes to file system and inventory
    pub fn commit(&mut self, inventory: Arc<InventoryDb>) -> Result<()> {
        if self.file_meta.is_none() {
            return Err(anyhow::anyhow!("File metadata is not set"));
        }

        // TODO: handle placeholder exist sceneario
        if self.local_path.exists() {
            // TODO: convert to placeholder if it's not one;
            return Ok(());
        }

        // Create placeholder file/directory
        let file_meta = self.file_meta.as_ref().unwrap();
        let relative_path = self
            .local_path
            .strip_prefix(&self.sync_root)
            .context("failed to get relative path")?;
        let primary_entity = OsString::from(file_meta.etag.clone());
        let placeholder = PlaceholderFile::new(relative_path)
            .metadata(
                match file_meta.is_folder {
                    true => Metadata::directory(),
                    false => Metadata::file(),
                }
                .size(file_meta.size as u64)
                .changed(FileTime::from_unix_time(file_meta.updated_at)?)
                .written(FileTime::from_unix_time(file_meta.updated_at)?)
                .created(FileTime::from_unix_time(file_meta.created_at)?),
            )
            .mark_in_sync()
            .overwrite()
            .blob(primary_entity.into_encoded_bytes());
        let parent_path: &std::path::Path = self
            .local_path
            .parent()
            .ok_or(anyhow::anyhow!("failed to get parent path"))?;
        placeholder
            .create::<&std::path::Path>(parent_path)
            .context("failed to create placeholder")?;

        // Upser inventory
        inventory
            .upsert(&MetadataEntry::from(file_meta))
            .context("failed to upsert inventory")?;

        // Notify shell change
        // notify_shell_change(
        //     &self.local_path,
        //     if file_meta.is_folder {
        //         SHCNE_CREATE
        //     } else {
        //         SHCNE_MKDIR
        //     },
        // )
        // .context("failed to notify shell change")?;

        Ok(())
    }

    pub fn with_remote_file(mut self, file_info: &FileResponse) -> Self {
        // Parse RFC3339 time strings from Golang
        let created_at = DateTime::parse_from_rfc3339(&file_info.created_at)
            .ok()
            .map(|dt| dt.timestamp())
            .unwrap_or_default();

        let updated_at = DateTime::parse_from_rfc3339(&file_info.updated_at)
            .ok()
            .map(|dt| dt.timestamp())
            .unwrap_or_default();

        self.file_meta = Some(FileMetadata {
            drive_id: self.drive_id,
            local_path: self.local_path.to_string_lossy().to_string(),
            remote_uri: file_info.path.clone(),
            is_folder: file_info.file_type == file_type::FOLDER,
            created_at,
            updated_at,
            size: file_info.size,
            etag: file_info.primary_entity.clone().unwrap_or_default(),
            id: 0,
            metadata: file_info.metadata.clone().unwrap_or_default(),
            props: None,
            permissions: file_info.permission.clone().unwrap_or_default(),
            shared: file_info.shared.unwrap_or(false),
        });
        self
    }
}

use crate::{
    cfapi::{
        metadata::Metadata,
        placeholder::{ConvertOptions, LocalFileInfo, OpenOptions, UpdateOptions},
        placeholder_file::PlaceholderFile,
    },
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

    pub fn with_invalidate_all_range(mut self, enable: bool) -> Self {
        if enable {
            self.options |= CrPlaceholderOptions::InvalidateAllRange as u32;
        } else {
            self.options &= !(CrPlaceholderOptions::InvalidateAllRange as u32);
        }
        self
    }

    pub fn delete_placeholder(&self, inventory: Arc<InventoryDb>) -> Result<()> {
        // Delete local file/folder if it exists
        if self.local_path.exists() {
            if self.local_path.is_dir() {
                std::fs::remove_dir_all(&self.local_path)
                    .context("failed to delete local directory")?;
            } else {
                std::fs::remove_file(&self.local_path).context("failed to delete local file")?;
            }
        }

        // Remove from inventory
        let path_str = self
            .local_path
            .to_str()
            .context("failed to convert path to string")?;
        inventory
            .batch_delete_by_path(vec![path_str])
            .context("failed to delete from inventory")?;

        // Notify shell change
        notify_shell_change(&self.local_path, SHCNE_DELETE)
            .context("failed to notify shell change")?;

        Ok(())
    }

    // Commit changes to file system and inventory
    pub fn commit(&mut self, inventory: Arc<InventoryDb>) -> Result<()> {
        if self.file_meta.is_none() {
            return Err(anyhow::anyhow!("File metadata is not set"));
        }

        let file_meta = self.file_meta.as_ref().unwrap();

        if self.local_path.exists() {
            let local_file_info = LocalFileInfo::from_path(&self.local_path)?;
            if !local_file_info.is_placeholder() {
                // Upgrade to placeholder
                let mut local_handle = OpenOptions::new()
                    .write_access()
                    .exclusive()
                    .open(&self.local_path)
                    .context("failed to open local file")?;
                tracing::info!(
                    target: "drive::placeholder",
                    local_path = %self.local_path.display(),
                    "Converting to placeholder"
                );
                local_handle
                    .convert_to_placeholder(ConvertOptions::default().mark_in_sync(), None)
                    .context("failed to convert to placeholder")?;
            }

            // TODO: Update metadata
            
            if self.options & CrPlaceholderOptions::InvalidateAllRange as u32 != 0 {
                tracing::debug!(target: "drive::placeholder", local_path = %self.local_path.display(), "Invalidating all range");
                let mut local_handle = OpenOptions::new()
                    .write_access()
                    .exclusive()
                    .open(&self.local_path)
                    .context("failed to open local file")?;
                local_handle
                    .update(UpdateOptions::default().dehydrate(), None)
                    .context("failed to invalidate all range")?;
            }
        } else {
            // Create placeholder file/directory
            let relative_path = self
                .local_path
                .strip_prefix(&self.sync_root)
                .context("failed to get relative path")?;
            tracing::trace!(target: "drive::placeholder", relative_path = %relative_path.to_string_lossy(), "Relative path");
            let primary_entity = OsString::from(file_meta.etag.clone());
            let placeholder = PlaceholderFile::new(self.local_path.file_name().context("failed to get file name")?)
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
        }

        // Upser inventory
        inventory
            .upsert(&MetadataEntry::from(file_meta))
            .context("failed to upsert inventory")?;

        // Notify shell change
        notify_shell_change(
            &self.local_path,
            if file_meta.is_folder {
                SHCNE_CREATE
            } else {
                SHCNE_MKDIR
            },
        )
        .context("failed to notify shell change")?;

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

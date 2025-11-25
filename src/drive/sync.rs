use crate::{
    cfapi::{metadata::Metadata, placeholder_file::PlaceholderFile},
    drive::{mounts::Mount, utils::remote_path_to_local_relative_path},
    inventory::MetadataEntry,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use cloudreve_api::models::{
    explorer::{FileResponse, file_type, metadata},
    uri::CrUri,
};
use notify_debouncer_full::notify::event::{
    AccessKind, CreateKind, EventKind, ModifyKind, RemoveKind,
};
use notify_debouncer_full::{DebouncedEvent, notify::Event};
use nt_time::FileTime;
use std::{collections::HashMap, ffi::OsString, path::PathBuf};
use uuid::Uuid;

pub fn cloud_file_to_placeholder(
    file: &FileResponse,
    local_path: &PathBuf,
    remote_path: &CrUri,
) -> Result<PlaceholderFile> {
    let file_uri = CrUri::new(&file.path)?;
    let relative_path = remote_path_to_local_relative_path(&file_uri, &remote_path)?;
    tracing::trace!(target: "drive::sync", file_uri = %file_uri.to_string(), remote_path = %remote_path.to_string(), relative_path = %relative_path.to_string_lossy(), "Relative path");
    let primary_entity = OsString::from(file.primary_entity.as_ref().unwrap_or(&String::new()));
    // Remove leading slash if presented

    // Parse RFC time string to unix timestamp
    let created_at =
        FileTime::from_unix_time(file.created_at.parse::<DateTime<Utc>>()?.timestamp())?;
    let last_modified =
        FileTime::from_unix_time(file.updated_at.parse::<DateTime<Utc>>()?.timestamp())?;

    Ok(PlaceholderFile::new(relative_path)
        .metadata(
            match file.file_type == file_type::FOLDER {
                true => Metadata::directory(),
                false => Metadata::file(),
            }
            .size(file.size as u64)
            .changed(last_modified)
            .written(last_modified)
            .created(created_at),
        )
        .mark_in_sync()
        .overwrite()
        .blob(primary_entity.into_encoded_bytes()))
}

pub fn cloud_file_to_metadata_entry(
    file: &FileResponse,
    drive_id: &Uuid,
    local_path: &PathBuf,
) -> Result<MetadataEntry> {
    let mut local_path = local_path.clone();
    local_path.push(file.name.clone());
    let local_path_str = local_path.to_str();
    if local_path_str.is_none() {
        tracing::error!(
            target: "drive::mounts",
            local_path = %local_path.display(),
            error = "Failed to convert local path to string"
        );
        return Err(anyhow::anyhow!("Failed to convert local path to string"));
    }

    // Parse RFC time string to unix timestamp
    let created_at = file.created_at.parse::<DateTime<Utc>>()?.timestamp();
    let last_modified = file.updated_at.parse::<DateTime<Utc>>()?.timestamp();

    Ok(MetadataEntry::new(
        drive_id.clone(),
        local_path_str.unwrap(),
        file.path.clone(),
        file.file_type == file_type::FOLDER,
    )
    .with_created_at(created_at)
    .with_updated_at(last_modified)
    .with_permissions(file.permission.as_ref().unwrap_or(&String::new()).clone())
    .with_shared(file.shared.unwrap_or(false))
    .with_etag(
        file.primary_entity
            .as_ref()
            .unwrap_or(&String::new())
            .clone(),
    )
    .with_metadata(file.metadata.as_ref().unwrap_or(&HashMap::new()).clone()))
}

pub fn is_symbolic_link(file: &FileResponse) -> bool {
    return file.metadata.is_some()
        && file
            .metadata
            .as_ref()
            .unwrap()
            .get(metadata::SHARE_REDIRECT)
            .is_some();
}

pub type GroupedFsEvents = HashMap<EventKind, Vec<Event>>;

/// Groups filesystem events by their first-level EventKind.
///
/// This function groups events into a HashMap where the key is the first-level EventKind
/// (normalized to use ::Any for nested variants) and the value is a vector of events.
///
/// # Arguments
/// * `events` - A vector of DebouncedEvent to be grouped
///
/// # Returns
/// A HashMap mapping EventKind to Vec<DebouncedEvent>
pub fn group_fs_events(events: Vec<DebouncedEvent>) -> GroupedFsEvents {
    let mut grouped: GroupedFsEvents = HashMap::new();

    for event in events {
        let normalized_kind = normalize_event_kind(&event.kind);
        grouped
            .entry(normalized_kind)
            .or_insert_with(Vec::new)
            .push(event.event);
    }

    grouped
}

/// Normalizes an EventKind to its first-level representation.
///
/// This helper function converts all nested EventKind variants to use their ::Any variant,
/// effectively grouping by the first level only. This can be extended to support deeper
/// level matching by adding parameters for match depth or specific variant matching.
///
/// # Arguments
/// * `kind` - The EventKind to normalize
///
/// # Returns
/// A normalized EventKind representing the first level only
fn normalize_event_kind(kind: &EventKind) -> EventKind {
    match kind {
        EventKind::Any => EventKind::Any,
        EventKind::Access(_) => EventKind::Access(AccessKind::Any),
        EventKind::Create(_) => EventKind::Create(CreateKind::Any),
        EventKind::Modify(_) => EventKind::Modify(ModifyKind::Any),
        EventKind::Remove(_) => EventKind::Remove(RemoveKind::Any),
        EventKind::Other => EventKind::Other,
    }
}

/// Determines how deep a sync operation should traverse for a given path list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Sync only the provided path entries.
    PathOnly,
    /// Sync the provided path entries and their first-level children.
    PathAndFirstLayer,
    /// Sync the provided path entries and every descendant.
    FullHierarchy,
}

impl Mount {
    /// Syncs a list of local paths by grouping them under their parent directories.
    pub async fn sync_paths(&self, local_paths: Vec<PathBuf>, mode: SyncMode) -> Result<()> {
        if local_paths.is_empty() {
            tracing::debug!(target: "drive::sync", id = %self.id, "No paths provided for sync");
            return Ok(());
        }

        let mut grouped: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

        for path in local_paths {
            let parent = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.clone());
            grouped.entry(parent).or_default().push(path);
        }

        for (parent, paths) in grouped.iter() {
            self.sync_group(parent, paths, mode).await?;
        }

        Ok(())
    }

    async fn sync_group(
        &self,
        parent: &PathBuf,
        paths: &[PathBuf],
        mode: SyncMode,
    ) -> Result<()> {
        tracing::info!(
            target: "drive::sync",
            id = %self.id,
            parent = %parent.display(),
            paths = paths.len(),
            mode = ?mode,
            "Queued grouped sync"
        );

        match mode {
            SyncMode::PathOnly => {
                for path in paths {
                    tracing::debug!(
                        target: "drive::sync",
                        id = %self.id,
                        path = %path.display(),
                        "TODO: sync path only"
                    );
                }
            }
            SyncMode::PathAndFirstLayer => {
                for path in paths {
                    tracing::debug!(
                        target: "drive::sync",
                        id = %self.id,
                        path = %path.display(),
                        "TODO: sync path and first layer of children"
                    );
                }
            }
            SyncMode::FullHierarchy => {
                for path in paths {
                    tracing::debug!(
                        target: "drive::sync",
                        id = %self.id,
                        path = %path.display(),
                        "TODO: sync path and descendants"
                    );
                }
            }
        }

        // TODO: plug in actual sync tasks once implemented.
        Ok(())
    }
}
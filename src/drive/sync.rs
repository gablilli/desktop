use crate::{
    cfapi::{
        metadata::Metadata,
        placeholder::{PinState, Placeholder, PlaceholderInfo},
        placeholder_file::PlaceholderFile,
    },
    drive::{
        mounts::Mount,
        utils::{local_path_to_cr_uri, remote_path_to_local_relative_path},
    },
    inventory::MetadataEntry,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cloudreve_api::{
    ApiError,
    api::{ExplorerApi, explorer::ExplorerApiExt},
    error::ErrorCode,
    models::{
        explorer::{FileResponse, GetFileInfoService, file_type, metadata},
        uri::CrUri,
    },
};
use notify_debouncer_full::notify::event::{
    AccessKind, CreateKind, EventKind, ModifyKind, RemoveKind,
};
use notify_debouncer_full::{DebouncedEvent, notify::Event};
use nt_time::FileTime;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};
use tokio::task;
use uuid::Uuid;
use windows::{
    Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
    core::Error as WindowsError,
};

pub fn cloud_file_to_placeholder(
    file: &FileResponse,
    _local_path: &PathBuf,
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

const REMOTE_PAGE_SIZE: i32 = 1000;

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

#[derive(Debug, Clone)]
struct LocalPlaceholderState {
    in_sync: bool,
    pin_state: PinState,
    on_disk_data_size: i64,
    validated_data_size: i64,
    modified_data_size: i64,
}

impl From<PlaceholderInfo> for LocalPlaceholderState {
    fn from(info: PlaceholderInfo) -> Self {
        Self {
            in_sync: info.is_in_sync(),
            pin_state: info.pin_state(),
            on_disk_data_size: info.on_disk_data_size(),
            validated_data_size: info.validated_data_size(),
            modified_data_size: info.modified_data_size(),
        }
    }
}

#[derive(Debug, Clone)]
struct LocalFileInfo {
    exists: bool,
    is_directory: bool,
    file_size: Option<u64>,
    last_modified: Option<SystemTime>,
    placeholder: Option<LocalPlaceholderState>,
}

impl LocalFileInfo {
    fn missing() -> Self {
        Self {
            exists: false,
            is_directory: false,
            file_size: None,
            last_modified: None,
            placeholder: None,
        }
    }

    fn from_path(path: &Path) -> Result<Self> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::missing());
            }
            Err(err) => {
                return Err(err).context(format!(
                    "failed to read local metadata for {}",
                    path.display()
                ));
            }
        };

        let is_directory = metadata.is_dir();
        let file_size = (!is_directory).then_some(metadata.len());
        let last_modified = metadata.modified().ok();

        let placeholder = match Placeholder::options().open(path) {
            Ok(handle) => match handle.info() {
                Ok(Some(info)) => Some(LocalPlaceholderState::from(info)),
                Ok(None) => None,
                Err(err) => {
                    return Err(err).context(format!(
                        "failed to query placeholder info for {}",
                        path.display()
                    ));
                }
            },
            Err(err) => {
                if should_suppress_placeholder_error(&err) {
                    tracing::trace!(
                        target: "drive::sync",
                        path = %path.display(),
                        error = %err,
                        "Placeholder handle unavailable"
                    );
                } else {
                    tracing::warn!(
                        target: "drive::sync",
                        path = %path.display(),
                        error = %err,
                        "Failed to open placeholder handle"
                    );
                }
                None
            }
        };

        Ok(Self {
            exists: true,
            is_directory,
            file_size,
            last_modified,
            placeholder,
        })
    }
}

fn should_suppress_placeholder_error(err: &WindowsError) -> bool {
    let code = err.code();
    code == ERROR_FILE_NOT_FOUND.to_hresult() || code == ERROR_PATH_NOT_FOUND.to_hresult()
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

    async fn sync_group(&self, parent: &PathBuf, paths: &[PathBuf], mode: SyncMode) -> Result<()> {
        tracing::info!(
            target: "drive::sync",
            id = %self.id,
            parent = %parent.display(),
            paths = paths.len(),
            mode = ?mode,
            "Queued grouped sync"
        );

        let remote_files = self.fetch_remote_file_infos(parent, paths).await?;
        tracing::debug!(
            target: "drive::sync",
            id = %self.id,
            parent = %parent.display(),
            requested = paths.len(),
            fetched = remote_files.len(),
            "Fetched remote metadata for sync group"
        );
        tracing::trace!("{:?}", remote_files);

        let local_files = self.fetch_local_file_infos(paths).await?;
        tracing::debug!(
            target: "drive::sync",
            id = %self.id,
            parent = %parent.display(),
            locals = local_files.len(),
            "Fetched local metadata for sync group"
        );
        tracing::trace!("{:?}", local_files);

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

    async fn fetch_local_file_infos(
        &self,
        paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, LocalFileInfo>> {
        if paths.is_empty() {
            return Ok(HashMap::new());
        }

        let targets: Vec<PathBuf> = paths.to_vec();
        let mut entries = HashMap::with_capacity(targets.len());
        for path in targets {
            let info = LocalFileInfo::from_path(&path)?;
            entries.insert(path, info);
        }

        Ok(entries)
    }

    async fn fetch_remote_file_infos(
        &self,
        parent: &PathBuf,
        paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, FileResponse>> {
        if paths.is_empty() {
            return Ok(HashMap::new());
        }

        let (remote_base, sync_root) = {
            let config = self.config.read().await;
            (config.remote_path.clone(), config.sync_path.clone())
        };

        let mut target_remote_paths: HashMap<String, PathBuf> = HashMap::with_capacity(paths.len());
        for path in paths {
            let remote_uri =
                local_path_to_cr_uri(path.clone(), sync_root.clone(), remote_base.clone())
                    .with_context(|| format!("failed to map {} to remote uri", path.display()))?;
            target_remote_paths.insert(remote_uri.to_string(), path.clone());
        }

        if target_remote_paths.len() == 1 {
            let (remote_uri, local_path) = target_remote_paths.iter().next().unwrap();
            let request = GetFileInfoService {
                uri: Some(remote_uri.clone()),
                ..Default::default()
            };

            match self.cr_client.get_file_info(&request).await {
                Ok(file_info) => {
                    let mut entries = HashMap::new();
                    entries.insert(local_path.clone(), file_info);
                    return Ok(entries);
                }
                Err(ApiError::ApiError { code, .. }) if code == ErrorCode::NotFound as i32 => {
                    tracing::warn!(
                        target: "drive::sync",
                        id = %self.id,
                        path = %local_path.display(),
                        remote_path = %remote_uri,
                        "Remote entry missing during sync"
                    );
                    return Ok(HashMap::new());
                }
                Err(err) => {
                    return Err(err.into());
                }
            }
        }

        let parent_remote_uri =
            local_path_to_cr_uri(parent.clone(), sync_root.clone(), remote_base.clone())
                .with_context(|| {
                    format!("failed to map parent {} to remote uri", parent.display())
                })?;
        let parent_uri_str = parent_remote_uri.to_string();

        let mut remote_entries: HashMap<PathBuf, FileResponse> =
            HashMap::with_capacity(paths.len());
        let mut remaining: HashSet<String> = target_remote_paths.keys().cloned().collect();
        let mut previous_response = None;

        while !remaining.is_empty() {
            let response = self
                .cr_client
                .list_files_all(
                    previous_response.as_ref(),
                    parent_uri_str.as_str(),
                    REMOTE_PAGE_SIZE,
                )
                .await?;

            for file in &response.res.files {
                if let Some(local_path) = target_remote_paths.get(&file.path) {
                    if remote_entries.contains_key(local_path) {
                        continue;
                    }
                    remote_entries.insert(local_path.clone(), file.clone());
                    remaining.remove(&file.path);
                }
            }

            let has_more = response.more;
            previous_response = Some(response);

            if !has_more {
                break;
            }
        }

        if !remaining.is_empty() {
            for missing in remaining {
                if let Some(local_path) = target_remote_paths.get(&missing) {
                    tracing::warn!(
                        target: "drive::sync",
                        id = %self.id,
                        path = %local_path.display(),
                        remote_path = %missing,
                        "Remote entry missing during sync"
                    );
                }
            }
        }

        Ok(remote_entries)
    }
}

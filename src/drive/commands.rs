use crate::{
    cfapi::{
        filter::ticket,
        placeholder::{LocalFileInfo, OpenOptions, PinState},
        utility::WriteAt,
    },
    drive::{
        mounts::Mount,
        sync::{GroupedFsEvents, SyncMode},
        utils::local_path_to_cr_uri,
    },
    tasks::TaskPayload,
};
use anyhow::{Context, Result};
use bytes::Bytes;
use cloudreve_api::{
    ApiError,
    api::{ExplorerApi, explorer::ExplorerApiExt},
    models::{
        explorer::{
            DeleteFileService, FileResponse, FileURLService, MoveFileService, RenameFileService,
            metadata,
        },
        uri::CrUri,
        user::Token,
    },
};
use notify_debouncer_full::notify::{
    Event, EventKind,
    event::{CreateKind, ModifyKind, RemoveKind, RenameMode},
};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
};
use tokio::sync::oneshot::Sender;
use widestring::U16CString;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW};
use windows_core::PCWSTR;
const PAGE_SIZE: i32 = 1000;

#[derive(Debug, Clone)]
pub struct GetPlacehodlerResult {
    pub files: Vec<FileResponse>,
    pub local_path: PathBuf,
    pub remote_path: CrUri,
}

/// Messages sent from OS threads (SyncFilter callbacks) to the async processing task
///
/// # Safety    
/// This is safe because Windows CFAPI callbacks are designed to be invoked from arbitrary threads
/// and the data contained in Request, ticket, and info types are meant to be passed between threads
/// during the callback's lifetime.
#[derive(Debug)]
pub enum MountCommand {
    FetchPlaceholders {
        path: PathBuf,
        response: Sender<Result<GetPlacehodlerResult>>,
    },
    RefreshCredentials {
        credentials: Token,
    },
    FetchData {
        path: PathBuf,
        ticket: ticket::FetchData,
        range: Range<u64>,
        response: Sender<Result<()>>,
    },
    ProcessFsEvents {
        events: GroupedFsEvents,
    },
    Sync {
        local_paths: Vec<PathBuf>,
        mode: SyncMode,
    },
    Rename {
        source: PathBuf,
        target: PathBuf,
        response: Sender<Result<()>>,
    },
    Renamed {
        source: PathBuf,
        destination: PathBuf,
    },
}

// SAFETY: Windows CFAPI is designed to allow callbacks from arbitrary threads.
// The Request, ticket, and info types contain data that is valid for the duration
// of the callback and can be safely transferred between threads.
unsafe impl Send for MountCommand {}

/// Commands for the DriveManager
/// These can be sent from external sources like context menus or other UI components
#[derive(Debug)]
pub enum ManagerCommand {
    /// View a file or folder online in the web interface
    ViewOnline {
        path: PathBuf,
    },
    PersistConfig,
    GenerateThumbnail {
        path: PathBuf,
        response: Sender<Result<Bytes>>,
    },
    SyncNow {
        paths: Vec<PathBuf>,
        mode: SyncMode,
    },
}

impl Mount {
    pub async fn fetch_data(
        &self,
        path: PathBuf,
        ticket: ticket::FetchData,
        range: Range<u64>,
    ) -> Result<()> {
        let config = self.config.read().await;
        let remote_base = config.remote_path.clone();
        let sync_path = config.sync_path.clone();
        drop(config);

        let uri = local_path_to_cr_uri(path.clone(), sync_path, remote_base)
            .context("failed to convert local path to cloudreve uri")?;

        let mut request: FileURLService = FileURLService::default();
        request.uris.push(uri.to_string());
        let entity_url_res = self
            .cr_client
            .get_file_url(&request)
            .await
            .context("failed to get file url")?;

        // Get the download URL from the response
        let download_url = entity_url_res
            .urls
            .first()
            .context("no download URL in response")?
            .url
            .clone();

        // Calculate total bytes to fetch
        let total_bytes = range.end - range.start;

        // 4KB chunk size (required by Windows CFAPI)
        const CHUNK_SIZE: usize = 4096;
        // 64KB buffer for reading from network
        const BUFFER_SIZE: usize = 65536;

        // Create HTTP client and make a single range request
        let client = reqwest::Client::new();
        let range_header = format!("bytes={}-{}", range.start, range.end - 1);

        let response = client
            .get(&download_url)
            .header("Range", range_header)
            .send()
            .await
            .context("failed to send HTTP range request")?;

        if !response.status().is_success() && response.status().as_u16() != 206 {
            anyhow::bail!("HTTP request failed with status: {}", response.status());
        }

        // Stream the response and write in 4KB-aligned chunks
        let mut stream = response.bytes_stream();
        let mut current_offset = range.start;
        let mut bytes_transferred = 0u64;
        let mut accumulator: Vec<u8> = Vec::with_capacity(BUFFER_SIZE);

        use futures::StreamExt;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.context("failed to read chunk from stream")?;
            accumulator.extend_from_slice(&chunk);

            // Write out all aligned chunks at once if we have enough data
            if accumulator.len() >= CHUNK_SIZE {
                // Calculate how many complete aligned chunks we can write
                let aligned_size = (accumulator.len() / CHUNK_SIZE) * CHUNK_SIZE;
                let write_data = accumulator.drain(..aligned_size).collect::<Vec<u8>>();

                ticket.write_at(&write_data, current_offset).map_err(|e| {
                    anyhow::anyhow!("failed to write data at offset {}: {:?}", current_offset, e)
                })?;

                bytes_transferred += write_data.len() as u64;
                current_offset += write_data.len() as u64;

                // Report progress to Windows
                ticket
                    .report_progress(total_bytes, bytes_transferred)
                    .map_err(|e| anyhow::anyhow!("failed to report progress: {:?}", e))?;
            }
        }

        // Write any remaining data (last chunk, may be less than 4KB)
        if !accumulator.is_empty() {
            ticket.write_at(&accumulator, current_offset).map_err(|e| {
                anyhow::anyhow!("failed to write data at offset {}: {:?}", current_offset, e)
            })?;

            bytes_transferred += accumulator.len() as u64;
            current_offset += accumulator.len() as u64;

            // Final progress report
            ticket
                .report_progress(total_bytes, bytes_transferred)
                .map_err(|e| anyhow::anyhow!("failed to report progress: {:?}", e))?;
        }

        tracing::debug!(
            target: "drive::commands",
            bytes_transferred = bytes_transferred,
            total = total_bytes,
            "Fetch data progress"
        );

        tracing::info!(
            target: "drive::commands",
            path = %path.display(),
            bytes = total_bytes,
            "Fetch data completed"
        );

        Ok(())
    }
    pub async fn fetch_placeholders(&self, path: PathBuf) -> Result<GetPlacehodlerResult> {
        let config = self.config.read().await;
        let remote_base = config.remote_path.clone();
        let sync_path = config.sync_path.clone();
        drop(config);

        let uri = local_path_to_cr_uri(path.clone(), sync_path, remote_base)
            .context("failed to convert local path to cloudreve uri")?;
        let mut placehodlers: Vec<FileResponse> = Vec::new();

        let mut previous_response = None;
        loop {
            let response = self
                .cr_client
                .list_files_all(previous_response.as_ref(), &uri.to_string(), PAGE_SIZE)
                .await?;

            for file in &response.res.files {
                tracing::debug!(target: "drive::mounts", file = %file.name, "Server file");
            }

            placehodlers.extend(response.res.files.clone());
            let has_more: bool = response.more;
            previous_response = Some(response);

            if !has_more {
                break;
            }
        }

        tracing::debug!(target: "drive::mounts", uri = %uri.to_string(), "Fetch file list from cloudreve");

        Ok(GetPlacehodlerResult {
            files: placehodlers,
            local_path: path.clone(),
            remote_path: uri.clone(),
        })
    }

    pub async fn generate_thumbnail(&self, path: PathBuf) -> Result<Bytes> {
        let file_meta = self
            .inventory
            .query_by_path(path.to_str().unwrap_or(""))
            .context("failed to query metadata by path")?
            .ok_or_else(|| anyhow::anyhow!("no metadata found for path: {:?}", path))?;

        if file_meta
            .metadata
            .get(metadata::THUMBNAIL_DISABLED)
            .is_some()
        {
            return Err(anyhow::anyhow!("thumbnail disabled for path: {:?}", path));
        }

        let (sync_path, remote_base) = {
            let config = self.config.read().await;
            (config.sync_path.clone(), config.remote_path.to_string())
        };
        let uri = local_path_to_cr_uri(path.clone(), sync_path, remote_base)
            .context("failed to convert local path to cloudreve uri")?
            .to_string();
        let thumb_res = self.cr_client.get_file_thumb(uri.as_str(), None).await?;

        // Download the thumbnail
        let thumb_url = thumb_res.url;
        tracing::trace!(target: "drive::commands", thumb_url = %thumb_url, "Thumbnail URL");
        let thumb_response = reqwest::get(thumb_url).await?;
        // Make sure the response is successful
        if !thumb_response.status().is_success() {
            return Err(anyhow::anyhow!(
                "failed to download thumbnail: {:?}",
                thumb_response.status()
            ));
        }
        Ok(thumb_response.bytes().await?)
    }

    pub async fn rename_completed(&self, source: PathBuf, destination: PathBuf) -> Result<()> {
        // Commit rename in inventory
        self.inventory
            .rename_path(
                source
                    .to_str()
                    .context("failed to convert source path to string")?,
                destination
                    .to_str()
                    .context("failed to convert destination path to string")?,
            )
            .context("failed to rename path in inventory")?;

        // Cancel ongoing/pending tasks
        match self.task_queue.cancel_by_path(source.clone()).await {
            Ok(0) => {
                // Mark file as in-sync
                tracing::trace!(target: "drive::commands", path = %destination.display(), "Marking file as in-sync: OPEN");
                match OpenOptions::new()
                    .write_access()
                    .exclusive()
                    .open(&destination)
                {
                    Ok(mut handle) => {
                        tracing::trace!(target: "drive::commands", path = %destination.display(), "Marking file as in-sync");
                        if let Err(e) = handle.mark_in_sync(true, None) {
                            tracing::error!(target: "drive::commands", error = %e, "Failed to mark as in-sync");
                            return Err(e.into());
                        }
                        tracing::trace!(target: "drive::commands", path = %destination.display(), "Marked file as in-sync: complete");
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!(target: "drive::commands", error = %e, "Failed to open file");
                        Err(e.into())
                    }
                }
            }
            Ok(count) => {
                tracing::info!(target: "drive::commands", path = %source.display(), count = count, "Cancelled tasks");
                // We have tasks canceled, we need to trigger sync on the moved file
                self.command_tx
                    .send(MountCommand::Sync {
                        local_paths: vec![source.clone()],
                        mode: SyncMode::FullHierarchy,
                    })
                    .context("failed to send sync command")?;
                Ok(())
            }
            Err(e) => {
                tracing::error!(target: "drive::commands", error = %e, "Failed to cancel tasks");
                Err(e.into())
            }
        }
    }

    pub async fn rename(&self, source: PathBuf, target: PathBuf) -> Result<()> {
        let (sync_path, remote_path) = {
            let config = self.config.read().await;
            (config.sync_path.clone(), config.remote_path.to_string())
        };

        // if target or source is not under sync root, do nothing
        if !target.starts_with(&sync_path) {
            // Source is being moved out of sync root
            //self.event_blocker
            //    .register_once(&EventKind::Remove(RemoveKind::Any), source.clone());
            return Ok(());
        }
        if !source.starts_with(&sync_path) {
            // Target is being moved into sync root - block the create event
            self.event_blocker
                .register_once(&EventKind::Create(CreateKind::Any), target.clone());
            return Ok(());
        }

        // if target and src under the same dir, trigger rename call
        let target_parent = target.parent().context("root cannot be moved")?;
        let source_parent = source.parent().context("root cannot be moved")?;
        if target_parent == source_parent {
            match self
                .cr_client
                .rename_file(&RenameFileService {
                    uri: local_path_to_cr_uri(source.clone(), sync_path, remote_path)?.to_string(),
                    new_name: target
                        .file_name()
                        .context("target cannot be moved")?
                        .to_string_lossy()
                        .to_string(),
                })
                .await
            {
                Ok(_) => {
                    // Block the modify name events for rename (From for source, To for target)
                    self.event_blocker.register_once(
                        &EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                        source.clone(),
                    );
                    self.event_blocker.register_once(
                        &EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                        target.clone(),
                    );

                    self.task_queue.cancel_by_path(source.clone()).await?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::error!(target: "drive::commands", error = %e, "Failed to rename file");
                    return Err(e.into());
                }
            }
        }

        // Process move call
        match self
            .cr_client
            .move_files(&MoveFileService {
                uris: vec![
                    local_path_to_cr_uri(source.clone(), sync_path.clone(), remote_path.clone())?
                        .to_string(),
                ],
                dst: local_path_to_cr_uri(
                    target_parent.to_path_buf(),
                    sync_path.clone(),
                    remote_path.clone(),
                )?
                .to_string(),
                copy: None,
            })
            .await
        {
            Ok(_) => {
                // Block remove event for source and create event for target
                self.event_blocker
                    .register_once(&EventKind::Remove(RemoveKind::Any), source.clone());
                self.event_blocker
                    .register_once(&EventKind::Create(CreateKind::Any), target.clone());
                return Ok(());
            }
            Err(e) => {
                tracing::error!(target: "drive::commands", error = %e, "Failed to move file");
                return Err(e.into());
            }
        }
    }

    pub async fn process_fs_events(&self, events: GroupedFsEvents) -> Result<()> {
        for (event_kind, events) in events {
            // Filter out events that were pre-registered by rename operations
            let filtered_events = self.event_blocker.filter_events(events, &event_kind);

            if filtered_events.is_empty() {
                continue;
            }

            // Extract configuration once to avoid repeated lock acquisition
            let (sync_path, remote_base) = {
                let config = self.config.read().await;
                (config.sync_path.clone(), config.remote_path.to_string())
            };

            let path_uri_mappings =
                self.build_path_uri_mappings(&filtered_events, &sync_path, &remote_base);

            if path_uri_mappings.is_empty() {
                tracing::warn!(target: "drive::commands", "No valid URIs to process");
                return Ok(());
            }

            match event_kind {
                EventKind::Remove(_) => {
                    self.process_fs_delete_events(path_uri_mappings, sync_path, remote_base)
                        .await?
                }
                EventKind::Create(_) => {
                    self.process_fs_create_events(path_uri_mappings, sync_path, remote_base)
                        .await?
                }
                EventKind::Modify(_) => {
                    self.process_fs_modify_events(path_uri_mappings, sync_path, remote_base)
                        .await?
                }
                _ => (),
            }
        }
        Ok(())
    }

    async fn process_fs_modify_events(
        &self,
        path_uri_mappings: HashMap<String, PathBuf>,
        sync_path: PathBuf,
        remote_base: String,
    ) -> Result<()> {
        tracing::debug!(
            target: "drive::commands",
            uri_count = path_uri_mappings.len(),
            uris = ?path_uri_mappings,
            "Processing filesystem modify events"
        );

        for (remote_uri, path) in path_uri_mappings {
            let placeholder_info = match LocalFileInfo::from_path(path.as_path()) {
                Ok(info) => info,
                Err(e) => {
                    tracing::error!(target: "drive::commands", path = %path.display(), error = %e, "Failed to get local file info");
                    continue;
                }
            };
            if placeholder_info.is_directory() {
                continue;
            }

            // For pinned file but not on disk, hydrate it
            if placeholder_info.pinned() == PinState::Pinned && placeholder_info.partial_on_disk() {
                tracing::debug!(target: "drive::commands", path = %path.display(), "Hydrate pinned not on disk placeholder");
                let mut placeholder = match OpenOptions::new().open_win32(path.as_path()) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!(target: "drive::commands", path = %path.display(), error = %e, "Failed to open win32 file");
                        continue;
                    }
                };
                if let Err(e) = placeholder.hydrate(0..) {
                    tracing::error!(target: "drive::commands", path = %path.display(), error = %e, "Failed to hydrate placeholder");
                    continue;
                }
                continue;
            }

            // General modification, quque a upload task if not exist
            if !placeholder_info.in_sync() {
                tracing::debug!(target: "drive::commands", path = %path.display(), "Queuing upload task for modified file");
                let payload = TaskPayload::upload(path.clone());
                let result = self
                    .task_queue
                    .enqueue(payload)
                    .await
                    .context("Failed to enqueue upload task");
                if result.is_err() {
                    tracing::error!(target: "drive::commands", path = %path.display(), error = ?result, "Failed to enqueue upload task");
                    continue;
                }
                continue;
            }
        }

        Ok(())
    }

    async fn process_fs_create_events(
        &self,
        path_uri_mappings: HashMap<String, PathBuf>,
        sync_path: PathBuf,
        remote_base: String,
    ) -> Result<()> {
        tracing::debug!(
            target: "drive::commands",
            uri_count = path_uri_mappings.len(),
            uris = ?path_uri_mappings,
            "Processing filesystem create events"
        );

        for (remote_uri, path) in path_uri_mappings {
            let payload = TaskPayload::upload(path.clone());

            self.task_queue
                .enqueue(payload)
                .await
                .context("Failed to enqueue upload task")?;
        }

        Ok(())
    }

    /// Process filesystem delete events by synchronizing deletions with the remote server
    /// and updating the local inventory.
    ///
    /// This function:
    /// 1. Converts local paths to remote URIs
    /// 2. Sends batch delete request to the server
    /// 3. Handles partial failures in batch operations
    /// 4. Updates local inventory for successfully deleted files
    async fn process_fs_delete_events(
        &self,
        path_uri_mappings: HashMap<String, PathBuf>,
        sync_path: PathBuf,
        remote_base: String,
    ) -> Result<()> {
        tracing::debug!(
            target: "drive::commands",
            uri_count = path_uri_mappings.len(),
            uris = ?path_uri_mappings,
            "Processing filesystem delete events"
        );

        let uris: Vec<String> = path_uri_mappings.keys().cloned().collect();

        tracing::info!(
            target: "drive::commands",
            uri_count = uris.len(),
            "Sending batch delete request to server"
        );

        // Attempt to delete files on the remote server
        let delete_result = self
            .cr_client
            .delete_files(&DeleteFileService {
                uris: uris.clone(),
                unlink: None,
                skip_soft_delete: None,
            })
            .await;

        // Determine which files were successfully deleted
        let successful_paths = match delete_result {
            Ok(_) => {
                tracing::info!(
                    target: "drive::commands",
                    count = uris.len(),
                    "Successfully deleted all files from server"
                );
                // All deletions succeeded
                path_uri_mappings.values().cloned().collect()
            }
            Err(e) => {
                tracing::error!(
                    target: "drive::commands",
                    error = %e,
                    "Batch delete operation failed"
                );
                self.handle_delete_error(e, &path_uri_mappings).await?
            }
        };

        // Update local inventory to reflect successful deletions
        if !successful_paths.is_empty() {
            self.update_inventory_for_deletions(&successful_paths)
                .context("Failed to update local inventory after deletions")?;
        }

        Ok(())
    }

    /// Build a mapping from remote URIs to local paths for the given events.
    /// Logs warnings for any paths that cannot be converted to URIs.
    fn build_path_uri_mappings(
        &self,
        events: &[Event],
        sync_path: &Path,
        remote_base: &str,
    ) -> HashMap<String, PathBuf> {
        events
            .iter()
            .flat_map(|event| &event.paths)
            .filter_map(|path| {
                match local_path_to_cr_uri(
                    path.clone(),
                    sync_path.to_path_buf(),
                    remote_base.to_string(),
                ) {
                    Ok(uri) => Some((uri.to_string(), path.clone())),
                    Err(e) => {
                        tracing::warn!(
                            target: "drive::commands",
                            path = %path.display(),
                            error = %e,
                            "Failed to convert local path to remote URI"
                        );
                        None
                    }
                }
            })
            .collect()
    }

    /// Handle deletion errors and determine which paths were successfully deleted.
    /// Returns the list of paths that were successfully deleted despite partial failures.
    async fn handle_delete_error(
        &self,
        error: ApiError,
        path_uri_mappings: &HashMap<String, PathBuf>,
    ) -> Result<Vec<PathBuf>> {
        match error {
            ApiError::BatchError {
                message: _,
                aggregated_errors: Some(errors),
            } => {
                // Collect failed URIs
                let failed_uris: std::collections::HashSet<_> = errors.keys().cloned().collect();

                tracing::warn!(
                    target: "drive::commands",
                    failed_count = failed_uris.len(),
                    total_count = path_uri_mappings.len(),
                    "Partial batch delete failure"
                );

                let mut successful_paths = Vec::new();
                let mut failed_paths = Vec::new();

                for (uri, path) in path_uri_mappings {
                    if failed_uris.contains(uri) {
                        failed_paths.push(path.clone());
                    } else {
                        successful_paths.push(path.clone());
                    }
                }

                if !failed_paths.is_empty() {
                    tracing::info!(
                        target: "drive::commands",
                        failed_count = failed_paths.len(),
                        "Scheduling resync for failed deletions"
                    );
                    let command = MountCommand::Sync {
                        local_paths: failed_paths.clone(),
                        mode: SyncMode::PathOnly,
                    };
                    if let Err(e) = self.command_tx.send(command) {
                        tracing::error!(
                            target: "drive::commands",
                            error = %e,
                            "Failed to send Sync command"
                        );
                    }
                }

                Ok(successful_paths)
            }
            _ => {
                // For non-batch errors, all operations failed
                tracing::error!(
                    target: "drive::commands",
                    error = %error,
                    "Complete batch delete failure - no files were deleted"
                );
                let command = MountCommand::Sync {
                    local_paths: path_uri_mappings.values().cloned().collect(),
                    mode: SyncMode::PathOnly,
                };
                if let Err(e) = self.command_tx.send(command) {
                    tracing::error!(target: "drive::commands", error = %e, "Failed to send Sync command");
                }
                Ok(Vec::new())
            }
        }
    }

    /// Update the local inventory to remove entries for successfully deleted paths.
    fn update_inventory_for_deletions(&self, paths: &[PathBuf]) -> Result<()> {
        let path_strs: Vec<&str> = paths
            .iter()
            .filter_map(|path| {
                path.to_str().or_else(|| {
                    tracing::warn!(
                        target: "drive::commands",
                        path = ?path,
                        "Cannot convert path to string for inventory update"
                    );
                    None
                })
            })
            .collect();

        if !path_strs.is_empty() {
            let count = path_strs.len();
            self.inventory.batch_delete_by_path(path_strs)?;
            tracing::debug!(
                target: "drive::commands",
                count = count,
                "Updated local inventory after deletions"
            );
        }

        Ok(())
    }
}

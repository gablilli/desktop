use crate::{
    cfapi::{filter::ticket, utility::WriteAt},
    drive::{interop::GetPlacehodlerResult, mounts::Mount, utils::local_path_to_cr_uri},
};
use anyhow::{Context, Result};
use cloudreve_api::{
    api::{ExplorerApi, explorer::ExplorerApiExt},
    models::{
        explorer::{FileResponse, FileURLService},
        user::Token,
    },
};
use std::{ops::Range, path::PathBuf};
use tokio::sync::oneshot::Sender;
const PAGE_SIZE: i32 = 1000;

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
    ViewOnline { path: PathBuf },
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
}

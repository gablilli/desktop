//! Chunk-based upload logic with streaming support

use crate::inventory::InventoryDb;
use crate::uploader::UploaderConfig;
use crate::uploader::encrypt::EncryptionConfig;
use crate::uploader::error::{UploadError, UploadResult};
use crate::uploader::progress::{ChunkProgressInfo, ProgressCallback, ProgressUpdate};
use crate::uploader::providers::{self, PolicyType};
use crate::uploader::session::UploadSession;
use bytes::Bytes;
use cloudreve_api::Client as CrClient;
use futures::Stream;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, BufReader, ReadBuf, SeekFrom};
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Per-chunk progress tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkProgress {
    /// Chunk index
    pub index: usize,
    /// Bytes uploaded for this chunk
    pub loaded: u64,
    /// ETag returned by storage provider (for S3-like providers)
    pub etag: Option<String>,
}

impl ChunkProgress {
    /// Create a new chunk progress entry
    pub fn new(index: usize) -> Self {
        Self {
            index,
            loaded: 0,
            etag: None,
        }
    }

    /// Check if chunk upload is complete
    pub fn is_complete(&self) -> bool {
        self.loaded > 0
    }
}

/// Metadata about a single chunk (without the data)
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    /// Chunk index
    pub index: usize,
    /// Expected chunk size
    pub size: u64,
    /// Byte offset in file
    pub offset: u64,
}

impl ChunkInfo {
    /// Create new chunk info
    pub fn new(index: usize, offset: u64, size: u64) -> Self {
        Self {
            index,
            offset,
            size,
        }
    }
}

/// Buffer size for streaming reads (64KB)
const STREAM_BUFFER_SIZE: usize = 64 * 1024;

/// A limited async reader that reads only a specific range from a file,
/// optionally applying encryption on-the-fly.
pub struct ChunkReader {
    reader: BufReader<File>,
    encryption: Option<EncryptionConfig>,
    start_offset: u64,
    position: u64,
    remaining: u64,
}

impl ChunkReader {
    /// Create a new chunk reader for a specific byte range
    pub async fn new(
        path: &Path,
        offset: u64,
        size: u64,
        encryption: Option<EncryptionConfig>,
    ) -> io::Result<Self> {
        let file = File::open(path).await?;
        let mut reader = BufReader::with_capacity(STREAM_BUFFER_SIZE, file);
        reader.seek(SeekFrom::Start(offset)).await?;

        Ok(Self {
            reader,
            encryption,
            start_offset: offset,
            position: 0,
            remaining: size,
        })
    }

    /// Get the total size of this chunk
    pub fn size(&self) -> u64 {
        self.position + self.remaining
    }
}

impl AsyncRead for ChunkReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.remaining == 0 {
            return Poll::Ready(Ok(()));
        }

        // Limit read to remaining bytes
        let max_read = (self.remaining as usize).min(buf.remaining());
        let mut limited_buf = buf.take(max_read);
        let before = limited_buf.filled().len();

        // Pin the inner reader - this is safe because BufReader<File> is Unpin
        let reader = Pin::new(&mut self.reader);

        match reader.poll_read(cx, &mut limited_buf) {
            Poll::Ready(Ok(())) => {
                let bytes_read = limited_buf.filled().len() - before;
                if bytes_read == 0 {
                    // EOF reached
                    return Poll::Ready(Ok(()));
                }

                // Apply encryption if configured
                if let Some(ref config) = self.encryption {
                    let file_offset = self.start_offset + self.position;
                    // Get the newly read bytes and encrypt them in place
                    let start = buf.filled().len();
                    unsafe {
                        buf.assume_init(bytes_read);
                    }
                    buf.advance(bytes_read);
                    let filled = buf.filled_mut();
                    let encrypted_slice = &mut filled[start..start + bytes_read];
                    config.encrypt_at_offset(encrypted_slice, file_offset);
                } else {
                    unsafe {
                        buf.assume_init(bytes_read);
                    }
                    buf.advance(bytes_read);
                }

                self.position += bytes_read as u64;
                self.remaining -= bytes_read as u64;

                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A stream that yields chunks of bytes from a ChunkReader.
/// Uses tokio_util's ReaderStream internally for simplicity.
pub struct ChunkStream {
    inner: ReaderStream<ChunkReader>,
}

impl ChunkStream {
    /// Create a new chunk stream from a reader
    pub fn new(reader: ChunkReader) -> Self {
        Self {
            inner: ReaderStream::with_capacity(reader, STREAM_BUFFER_SIZE),
        }
    }

    /// Create a chunk stream from file path and chunk info
    pub async fn from_chunk(
        path: &Path,
        chunk: &ChunkInfo,
        encryption: Option<EncryptionConfig>,
    ) -> io::Result<Self> {
        let reader = ChunkReader::new(path, chunk.offset, chunk.size, encryption).await?;
        Ok(Self::new(reader))
    }
}

impl Stream for ChunkStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

/// Chunk uploader that handles uploading chunks to different providers
pub struct ChunkUploader {
    http_client: HttpClient,
    cr_client: Arc<CrClient>,
    policy_type: PolicyType,
    config: UploaderConfig,
}

impl ChunkUploader {
    /// Create a new chunk uploader
    pub fn new(
        http_client: HttpClient,
        cr_client: Arc<CrClient>,
        policy_type: PolicyType,
        config: UploaderConfig,
    ) -> Self {
        Self {
            http_client,
            cr_client,
            policy_type,
            config,
        }
    }

    /// Upload all chunks for a file
    pub async fn upload_all<P: ProgressCallback>(
        &self,
        local_path: &Path,
        session: &mut UploadSession,
        inventory: &InventoryDb,
        progress: &P,
        cancel_token: &CancellationToken,
    ) -> UploadResult<()> {
        info!(
            target: "uploader::chunk",
            local_path = %local_path.display(),
            num_chunks = session.num_chunks(),
            policy_type = ?self.policy_type,
            "Starting chunk upload"
        );

        // Get encryption config if needed
        let encryption = session
            .encrypt_metadata
            .as_ref()
            .map(|meta| EncryptionConfig::from_metadata(meta))
            .transpose()?;

        // Get pending chunks
        let pending_chunks = session.pending_chunks();
        if pending_chunks.is_empty() {
            info!(
                target: "uploader::chunk",
                "All chunks already uploaded"
            );
            return Ok(());
        }

        info!(
            target: "uploader::chunk",
            pending = pending_chunks.len(),
            total = session.num_chunks(),
            "Uploading pending chunks"
        );

        // Upload chunks sequentially
        // TODO: Implement concurrent chunk upload with proper ordering
        for chunk_index in pending_chunks {
            // Check for cancellation
            if cancel_token.is_cancelled() {
                return Err(UploadError::Cancelled);
            }

            // Get chunk info
            let (offset, _end) = session.chunk_range(chunk_index);
            let chunk_size = session.chunk_size_for(chunk_index);

            let chunk = ChunkInfo::new(chunk_index, offset, chunk_size);

            // Upload with retries (stream is created inside retry loop)
            let etag = self
                .upload_chunk_with_retry(
                    local_path,
                    &chunk,
                    session,
                    encryption.clone(),
                    cancel_token,
                )
                .await?;

            // Update session progress
            session.complete_chunk(chunk_index, etag);

            // Persist progress to database
            if let Err(e) =
                inventory.update_upload_session_progress(&session.id, &session.chunk_progress)
            {
                warn!(
                    target: "uploader::chunk",
                    error = %e,
                    "Failed to persist chunk progress"
                );
            }

            // Report progress
            self.report_progress(session, Some(chunk_index), progress);
        }

        Ok(())
    }

    /// Upload a single chunk with retry logic
    async fn upload_chunk_with_retry(
        &self,
        local_path: &Path,
        chunk: &ChunkInfo,
        session: &UploadSession,
        encryption: Option<EncryptionConfig>,
        cancel_token: &CancellationToken,
    ) -> UploadResult<Option<String>> {
        let mut last_error = None;

        for attempt in 0..=self.config.max_retries {
            if cancel_token.is_cancelled() {
                return Err(UploadError::Cancelled);
            }

            if attempt > 0 {
                let delay = self.calculate_retry_delay(attempt);
                debug!(
                    target: "uploader::chunk",
                    chunk = chunk.index,
                    attempt,
                    delay_ms = delay.as_millis(),
                    "Retrying chunk upload"
                );

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel_token.cancelled() => {
                        return Err(UploadError::Cancelled);
                    }
                }
            }

            // Create a fresh stream for each attempt
            let stream = ChunkStream::from_chunk(local_path, chunk, encryption.clone())
                .await
                .map_err(|e| {
                    UploadError::FileReadError(format!("Failed to create stream: {}", e))
                })?;

            match self.upload_chunk(chunk, stream, session).await {
                Ok(etag) => {
                    debug!(
                        target: "uploader::chunk",
                        chunk = chunk.index,
                        etag = ?etag,
                        "Chunk uploaded successfully"
                    );
                    return Ok(etag);
                }
                Err(e) => {
                    if !e.is_retryable() || attempt == self.config.max_retries {
                        error!(
                            target: "uploader::chunk",
                            chunk = chunk.index,
                            error = %e,
                            attempt,
                            "Chunk upload failed"
                        );
                        return Err(e);
                    }
                    warn!(
                        target: "uploader::chunk",
                        chunk = chunk.index,
                        error = %e,
                        attempt,
                        "Chunk upload failed, will retry"
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(UploadError::MaxRetriesExceeded {
            chunk_index: chunk.index,
            max_retries: self.config.max_retries,
        }))
    }

    /// Upload a single chunk (provider-specific)
    async fn upload_chunk(
        &self,
        chunk: &ChunkInfo,
        stream: ChunkStream,
        session: &UploadSession,
    ) -> UploadResult<Option<String>> {
        providers::upload_chunk(
            &self.http_client,
            &self.cr_client,
            self.policy_type,
            chunk,
            stream,
            session,
        )
        .await
    }

    /// Calculate retry delay with exponential backoff
    fn calculate_retry_delay(&self, attempt: u32) -> Duration {
        let base = self.config.retry_base_delay.as_millis() as u64;
        let delay_ms = base * (1 << attempt.min(10)); // Cap exponential growth
        let delay = Duration::from_millis(delay_ms);
        delay.min(self.config.retry_max_delay)
    }

    /// Report progress to callback
    fn report_progress<P: ProgressCallback>(
        &self,
        session: &UploadSession,
        current_chunk: Option<usize>,
        callback: &P,
    ) {
        let chunk_info: Vec<ChunkProgressInfo> = session
            .chunk_progress
            .iter()
            .map(|c| {
                let size = session.chunk_size_for(c.index);
                ChunkProgressInfo {
                    index: c.index,
                    size,
                    loaded: c.loaded,
                    complete: c.is_complete(),
                }
            })
            .collect();

        let update = ProgressUpdate::new(
            session.file_size,
            session.total_uploaded(),
            current_chunk,
            session.num_chunks(),
        )
        .with_chunk_progress(chunk_info);

        callback.on_progress(update);
    }
}

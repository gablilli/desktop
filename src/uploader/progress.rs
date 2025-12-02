//! Progress reporting for uploads

use std::sync::Arc;

/// Progress update information
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    /// Total file size
    pub total_size: u64,
    /// Total bytes uploaded
    pub uploaded: u64,
    /// Progress percentage (0.0 - 1.0)
    pub progress: f64,
    /// Current chunk index being uploaded
    pub current_chunk: Option<usize>,
    /// Total number of chunks
    pub total_chunks: usize,
    /// Per-chunk progress (optional)
    pub chunk_progress: Option<Vec<ChunkProgressInfo>>,
}

/// Per-chunk progress information
#[derive(Debug, Clone)]
pub struct ChunkProgressInfo {
    /// Chunk index
    pub index: usize,
    /// Chunk size
    pub size: u64,
    /// Bytes uploaded for this chunk
    pub loaded: u64,
    /// Whether this chunk is complete
    pub complete: bool,
}

impl ProgressUpdate {
    /// Create a new progress update
    pub fn new(
        total_size: u64,
        uploaded: u64,
        current_chunk: Option<usize>,
        total_chunks: usize,
    ) -> Self {
        let progress = if total_size > 0 {
            uploaded as f64 / total_size as f64
        } else {
            1.0
        };

        Self {
            total_size,
            uploaded,
            progress,
            current_chunk,
            total_chunks,
            chunk_progress: None,
        }
    }

    /// Add per-chunk progress information
    pub fn with_chunk_progress(mut self, chunks: Vec<ChunkProgressInfo>) -> Self {
        self.chunk_progress = Some(chunks);
        self
    }
}

/// Trait for receiving progress updates
pub trait ProgressCallback: Send + Sync {
    /// Called when upload progress changes
    fn on_progress(&self, update: ProgressUpdate);
}

/// No-op progress callback implementation
pub struct NoOpProgress;

impl ProgressCallback for NoOpProgress {
    fn on_progress(&self, _update: ProgressUpdate) {}
}

/// Closure-based progress callback
pub struct FnProgress<F>(pub F);

impl<F> ProgressCallback for FnProgress<F>
where
    F: Fn(ProgressUpdate) + Send + Sync,
{
    fn on_progress(&self, update: ProgressUpdate) {
        (self.0)(update)
    }
}

/// Arc wrapper for progress callbacks
impl<T: ProgressCallback> ProgressCallback for Arc<T> {
    fn on_progress(&self, update: ProgressUpdate) {
        (**self).on_progress(update)
    }
}

/// Box wrapper for progress callbacks
impl ProgressCallback for Box<dyn ProgressCallback> {
    fn on_progress(&self, update: ProgressUpdate) {
        (**self).on_progress(update)
    }
}


use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

/// Configuration for the logging system
pub struct LogConfig {
    /// Directory where log files will be stored
    pub log_dir: PathBuf,
    /// Prefix for log file names
    pub file_prefix: String,
    /// Maximum number of log files to keep (rotation)
    pub max_files: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cloudreve")
            .join("logs");

        Self {
            log_dir,
            file_prefix: "cloudreve-sync".to_string(),
            max_files: 5,
        }
    }
}

/// Initialize the logging system with both file and stdout output
///
/// This sets up:
/// - File logging with rotation (max 5 files by default)
/// - Stdout logging with colors
/// - Component-specific log targets (api, drive, events, sync)
/// - Configurable log levels via RUST_LOG environment variable
///
/// # Log Targets
/// - `api` - API requests and responses
/// - `api::health` - Health check endpoints
/// - `api::drives` - Drive management operations
/// - `api::sync` - Sync operations
/// - `api::sse` - Server-Sent Events
/// - `api::error` - API error responses
/// - `drive` - DriveManager operations
/// - `events` - Event broadcasting
/// - `main` - Application lifecycle
///
/// # Example
/// ```bash
/// # Set log level for all components
/// RUST_LOG=debug cargo run
///
/// # Set different levels for different components
/// RUST_LOG=api=debug,drive=info,events=trace cargo run
///
/// # Show only specific component
/// RUST_LOG=api::drives=debug cargo run
/// ```
pub fn init_logging(config: LogConfig) -> Result<LogGuard> {
    // Ensure log directory exists
    std::fs::create_dir_all(&config.log_dir).context("Failed to create log directory")?;

    // Create file appender with rotation
    // This will create files like: cloudreve-sync.log, cloudreve-sync.log.1, etc.
    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(&config.file_prefix)
        .filename_suffix("log")
        .max_log_files(config.max_files)
        .build(&config.log_dir)
        .context("Failed to create file appender")?;

    // Create non-blocking writer for file output
    // IMPORTANT: The guard MUST be kept alive for the entire application lifetime
    let (non_blocking_file, worker_guard) = tracing_appender::non_blocking(file_appender);

    // Configure environment filter with defaults
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "cloudreve_sync=info,\
             api=info,\
             drive=info,\
             events=info,\
             main=info,\
             tower_http=debug,\
             axum=info",
        )
    });

    // Create file layer (JSON format for structured logging)
    let file_layer = fmt::layer()
        .json()
        .with_writer(non_blocking_file)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(env_filter.clone());

    // Create stdout layer (human-readable with colors)
    let stdout_layer = fmt::layer()
        .compact()
        .with_target(true)
        .with_thread_ids(false)
        .with_line_number(true)
        .with_ansi(true)
        .with_filter(env_filter);

    // Initialize the subscriber with both layers
    tracing_subscriber::registry()
        .with(file_layer)
        .with(stdout_layer)
        .init();

    tracing::info!(
        target: "main",
        log_dir = %config.log_dir.display(),
        max_files = config.max_files,
        "Logging system initialized"
    );

    Ok(LogGuard {
        _worker_guard: worker_guard,
    })
}

/// Guard that ensures logs are flushed before exit
/// This wraps the WorkerGuard from tracing_appender which MUST be kept alive
/// for the entire application lifetime to ensure file logging works properly
pub struct LogGuard {
    _worker_guard: tracing_appender::non_blocking::WorkerGuard,
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        tracing::info!(target: "main", "Flushing logs before shutdown");
        // WorkerGuard will be dropped here, flushing remaining logs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_log_config() {
        let config = LogConfig::default();
        assert_eq!(config.file_prefix, "cloudreve-sync");
        assert_eq!(config.max_files, 5);
    }

    #[test]
    fn test_log_directory_creation() {
        let temp_dir = std::env::temp_dir().join("cloudreve_test_logs");
        let config = LogConfig {
            log_dir: temp_dir.clone(),
            file_prefix: "test".to_string(),
            max_files: 3,
        };

        let result = init_logging(config);
        assert!(result.is_ok());
        assert!(temp_dir.exists());

        // Keep the guard alive during test
        let _guard = result.unwrap();

        // Cleanup
        let _ = std::fs::remove_dir_all(temp_dir);
    }
}

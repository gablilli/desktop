mod api;
mod cfapi;
mod drive;
mod events;
mod inventory;
mod logging;
mod shellext;
mod tasks;

use anyhow::{Context, Result};
use api::{AppState, create_router};
use drive::manager::DriveManager;
use events::EventBroadcaster;
use logging::LogConfig;
use rust_i18n::set_locale;
use std::sync::Arc;
use sys_locale::get_locale;
use tasks::{TaskManager, TaskManagerConfig};
use tokio::signal;
use tower_http::trace::TraceLayer;

#[macro_use]
extern crate rust_i18n;

fn init_i18n() {
    let locale = get_locale().unwrap_or_else(|| String::from("en-US"));
    set_locale(locale.as_str());
}

#[tokio::main]
async fn main() -> Result<()> {
    init_i18n();
    // Initialize logging system with file rotation and component-specific targets
    // Keep the guard alive for the entire application lifetime
    let _log_guard = logging::init_logging(LogConfig::default())
        .context("Failed to initialize logging system")?;

    tracing::info!(target: "main", "🚀 Starting Cloudreve Sync Service...");

    // Initialize DriveManager
    tracing::info!(target: "main", "Initializing DriveManager...");
    let drive_manager = Arc::new(DriveManager::new().context("Failed to create DriveManager")?);

    // Spawn command processor for DriveManager
    drive_manager.spawn_command_processor().await;
    tracing::info!(target: "main", "DriveManager command processor started");

    // Load drive configurations from disk
    drive_manager
        .load()
        .await
        .context("Failed to load drive configurations")?;

    // Initialize EventBroadcaster
    let event_broadcaster = EventBroadcaster::new(100);
    tracing::info!(target: "main", "Event broadcasting system initialized");

    // Initialize and start the shell services (context menu handler) in a separate thread
    let mut shell_service =
        shellext::shell_service::init_and_start_service_task(drive_manager.clone());

    // Wait for shell services to initialize
    if let Err(e) = shell_service.wait_for_init() {
        tracing::error!(target: "main", "Warning: Failed to initialize shell services: {:?}", e);
        tracing::info!(target: "main", "Continuing without context menu handler...\n");
    } else {
        tracing::info!(target: "main", "Context menu handler initialized successfully!\n");
    }

    // Initialize TaskManager
    let task_config = TaskManagerConfig {
        max_workers: 4,
        completed_buffer_size: 100,
    };
    let task_manager = TaskManager::new(task_config);
    tracing::info!(target: "main", "Task manager initialized");

    // Create application state
    let state = AppState {
        drive_manager: drive_manager.clone(),
        event_broadcaster: event_broadcaster.clone(),
        task_manager: task_manager.clone(),
    };

    // Create router with middleware
    let app = create_router(state).layer(TraceLayer::new_for_http());

    // Bind to address
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context(format!("Failed to bind to {}", addr))?;

    tracing::info!(target: "main", "🌐 HTTP server listening on http://{}", addr);
    tracing::info!(target: "main", "📡 SSE endpoint available at http://{}/api/events", addr);
    tracing::info!(target: "main", "🔍 Health check available at http://{}/health", addr);

    // Broadcast initial connection status
    event_broadcaster.connection_status_changed(true);

    // Serve with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(
            drive_manager.clone(),
            event_broadcaster.clone(),
            task_manager.clone(),
        ))
        .await
        .context("Server error")?;

    tracing::info!(target: "main", "👋 Server shutdown complete");

    Ok(())
}

/// Wait for shutdown signal and perform cleanup
async fn shutdown_signal(
    drive_manager: Arc<DriveManager>,
    event_broadcaster: EventBroadcaster,
    task_manager: Arc<TaskManager>,
) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!(target: "main", "Received Ctrl+C signal");
        },
        _ = terminate => {
            tracing::info!(target: "main", "Received SIGTERM signal");
        },
    }

    tracing::info!(target: "main", "🛑 Shutting down gracefully...");

    // Shutdown drive manager
    tracing::info!(target: "main", "Shutting down drive manager...");
    drive_manager.shutdown().await;
    tracing::trace!(target: "main", "Drive manager shutdown complete");

    // Broadcast disconnection event
    event_broadcaster.connection_status_changed(false);

    // Give clients time to receive the disconnection event
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Shutdown task manager
    tracing::info!(target: "main", "Shutting down task manager...");
    task_manager.shutdown().await;
    tracing::trace!(target: "main", "Task manager shutdown complete");

    // Persist drive state
    tracing::info!(target: "main", "Persisting drive configurations...");
    if let Err(e) = drive_manager.persist().await {
        tracing::error!(target: "main", error = %e, "Failed to persist drive configurations");
    } else {
        tracing::info!(target: "main", "Drive configurations saved successfully");
    }

    // Additional cleanup can be added here
    // e.g., stopping active sync operations, closing connections, etc.
}

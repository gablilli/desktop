mod api;
mod drive;
mod events;
mod logging;

use anyhow::{Context, Result};
use api::{AppState, create_router};
use drive::manager::DriveManager;
use events::EventBroadcaster;
use logging::LogConfig;
use std::sync::Arc;
use tokio::signal;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging system with file rotation and component-specific targets
    // Keep the guard alive for the entire application lifetime
    let _log_guard = logging::init_logging(LogConfig::default())
        .context("Failed to initialize logging system")?;

    tracing::info!(target: "main", "🚀 Starting Cloudreve Sync Service...");

    // Initialize DriveManager
    tracing::info!(target: "main", "Initializing DriveManager...");
    let drive_manager = Arc::new(DriveManager::new().context("Failed to create DriveManager")?);

    // Load drive configurations from disk
    drive_manager
        .load()
        .await
        .context("Failed to load drive configurations")?;

    // Initialize EventBroadcaster
    let event_broadcaster = EventBroadcaster::new(100);
    tracing::info!(target: "main", "Event broadcasting system initialized");

    // Create application state
    let state = AppState {
        drive_manager: drive_manager.clone(),
        event_broadcaster: event_broadcaster.clone(),
    };

    // Create router with middleware
    let app = create_router(state).layer(TraceLayer::new_for_http());

    // Bind to address
    let addr = "127.0.0.1:3000";
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
        ))
        .await
        .context("Server error")?;

    tracing::info!(target: "main", "👋 Server shutdown complete");

    Ok(())
}

/// Wait for shutdown signal and perform cleanup
async fn shutdown_signal(drive_manager: Arc<DriveManager>, event_broadcaster: EventBroadcaster) {
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

    // Broadcast disconnection event
    event_broadcaster.connection_status_changed(false);

    // Give clients time to receive the disconnection event
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

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

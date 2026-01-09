use anyhow::Context;
use cloudreve_sync::{DriveManager, EventBroadcaster, LogConfig, LogGuard};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

mod commands;

/// Application state containing the drive manager and event broadcaster
pub struct AppState {
    pub drive_manager: Arc<DriveManager>,
    pub event_broadcaster: EventBroadcaster,
    // Keep the log guard alive for the entire application lifetime
    #[allow(dead_code)]
    log_guard: LogGuard,
}

/// Initialize the sync service (DriveManager, shell services, etc.)
async fn init_sync_service() -> anyhow::Result<(Arc<DriveManager>, EventBroadcaster, LogGuard)> {
    // Initialize i18n
    cloudreve_sync::init_i18n();

    // Initialize app root (Windows Package detection)
    cloudreve_sync::init_app_root();

    // Initialize logging system
    let log_guard = cloudreve_sync::logging::init_logging(LogConfig::default())
        .context("Failed to initialize logging system")?;

    tracing::info!(target: "main", "Starting Cloudreve Sync Service (Tauri)...");

    // Initialize DriveManager
    tracing::info!(target: "main", "Initializing DriveManager...");
    let drive_manager =
        Arc::new(DriveManager::new().context("Failed to create DriveManager")?);

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
        cloudreve_sync::shellext::shell_service::init_and_start_service_task(drive_manager.clone());

    // Wait for shell services to initialize
    if let Err(e) = shell_service.wait_for_init() {
        tracing::error!(target: "main", "Warning: Failed to initialize shell services: {:?}", e);
        tracing::info!(target: "main", "Continuing without context menu handler...");
    } else {
        tracing::info!(target: "main", "Shell services initialized successfully!");
    }

    // Broadcast initial connection status
    event_broadcaster.connection_status_changed(true);

    Ok((drive_manager, event_broadcaster, log_guard))
}

/// Spawn a task that bridges EventBroadcaster to Tauri events
fn spawn_event_bridge(app_handle: AppHandle, event_broadcaster: &EventBroadcaster) {
    let mut receiver = event_broadcaster.subscribe();

    tauri::async_runtime::spawn(async move {
        tracing::info!(target: "events", "Event bridge started");

        loop {
            match receiver.recv().await {
                Ok(event) => {
                    // Emit the event to all windows
                    if let Err(e) = app_handle.emit("sync-event", &event) {
                        tracing::error!(target: "events", error = %e, "Failed to emit event to frontend");
                    } else {
                        tracing::trace!(target: "events", event = ?event, "Event emitted to frontend");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(target: "events", skipped = n, "Event receiver lagged, some events were skipped");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::info!(target: "events", "Event broadcaster closed, stopping bridge");
                    break;
                }
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Initialize sync service in blocking context since setup isn't async
            let (drive_manager, event_broadcaster, log_guard) =
                tauri::async_runtime::block_on(async { init_sync_service().await })?;

            // Spawn event bridge to forward events to frontend
            spawn_event_bridge(app.handle().clone(), &event_broadcaster);

            // Store state in Tauri's managed state
            app.manage(AppState {
                drive_manager,
                event_broadcaster,
                log_guard,
            });

            tracing::info!(target: "main", "Tauri application setup complete");
            Ok(())
        })
        .on_window_event(|window, event| {
            // Handle window close to persist configuration
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                tracing::info!(target: "main", "Window close requested, initiating shutdown...");

                let app_handle = window.app_handle();
                let state: State<AppState> = app_handle.state();

                // Persist drive configurations
                let drive_manager = state.drive_manager.clone();
                let event_broadcaster = state.event_broadcaster.clone();

                tauri::async_runtime::block_on(async {
                    // Broadcast disconnection event
                    event_broadcaster.connection_status_changed(false);

                    // Shutdown drive manager
                    tracing::info!(target: "main", "Shutting down drive manager...");
                    drive_manager.shutdown().await;

                    // Persist drive state
                    tracing::info!(target: "main", "Persisting drive configurations...");
                    if let Err(e) = drive_manager.persist().await {
                        tracing::error!(target: "main", error = %e, "Failed to persist drive configurations");
                    } else {
                        tracing::info!(target: "main", "Drive configurations saved successfully");
                    }
                });

                tracing::info!(target: "main", "Shutdown complete");
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_drives,
            commands::add_drive,
            commands::remove_drive,
            commands::get_sync_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

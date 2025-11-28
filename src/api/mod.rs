mod error;
mod handlers;
mod sse;

pub use error::AppError;

use crate::drive::manager::DriveManager;
use crate::events::EventBroadcaster;
use axum::{
    Router,
    routing::{delete, get, post, put},
};
use serde::Serialize;
use std::sync::Arc;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub drive_manager: Arc<DriveManager>,
    pub event_broadcaster: EventBroadcaster,
}

/// Standard API response
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
        }
    }
}

/// Create the API router
pub fn create_router(state: AppState) -> Router {
    tracing::debug!(target: "api", "Creating API router");

    Router::new()
        // Health check
        .route("/health", get(handlers::health_check))
        // Drive management
        .route("/api/drives", get(handlers::list_drives))
        .route("/api/drives", post(handlers::add_drive))
        .route("/api/drives/:id", get(handlers::get_drive))
        .route("/api/drives/:id", put(handlers::update_drive))
        .route("/api/drives/:id", delete(handlers::remove_drive))
        // Sync operations
        .route("/api/drives/:id/sync", post(handlers::sync_command))
        .route("/api/drives/:id/status", get(handlers::get_sync_status))
        // Task management
        // .route("/api/tasks", get(task_handlers::list_tasks))
        // .route("/api/tasks", post(task_handlers::submit_task))
        // .route("/api/tasks/statistics", get(task_handlers::get_statistics))
        // .route("/api/tasks/config", put(task_handlers::update_config))
        // .route("/api/tasks/stop", post(task_handlers::stop_all_tasks))
        // .route(
        //     "/api/tasks/completed/clear",
        //     post(task_handlers::clear_completed),
        // )
        // .route("/api/tasks/:id", get(task_handlers::get_task))
        // .route("/api/tasks/:id/cancel", post(task_handlers::cancel_task))
        // Server-Sent Events for real-time updates
        .route("/api/events", get(sse::sse_handler))
        .with_state(state)
}

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
        // Server-Sent Events for real-time updates
        .route("/api/events", get(sse::sse_handler))
        .with_state(state)
}

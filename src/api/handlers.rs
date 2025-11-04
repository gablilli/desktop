use crate::drive::manager::DriveConfig;
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use std::collections::HashMap;

use super::{ApiResponse, AppError, AppState};

/// Request body for adding a drive
#[derive(Debug, Deserialize)]
pub struct AddDriveRequest {
    pub name: String,
    pub drive_type: String,
    pub sync_path: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Request body for updating a drive
#[derive(Debug, Deserialize)]
pub struct UpdateDriveRequest {
    pub name: Option<String>,
    pub sync_path: Option<String>,
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Request body for sync commands
#[derive(Debug, Deserialize)]
pub struct SyncCommandRequest {
    pub action: String, // "start" or "stop"
}

/// Health check endpoint
pub async fn health_check() -> Json<ApiResponse<serde_json::Value>> {
    tracing::debug!(target: "api::health", "Health check requested");
    Json(ApiResponse::success(serde_json::json!({
        "status": "healthy",
        "service": "cloudreve-sync"
    })))
}

/// List all drives
pub async fn list_drives(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<DriveConfig>>>, AppError> {
    tracing::debug!(target: "api::drives", "Listing all drives");
    let drives = state.drive_manager.list_drives().await;
    tracing::info!(target: "api::drives", "Retrieved {} drive(s)", drives.len());
    Ok(Json(ApiResponse::success(drives)))
}

/// Get a specific drive
pub async fn get_drive(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<DriveConfig>>, AppError> {
    tracing::debug!(target: "api::drives", drive_id = %id, "Getting drive details");
    match state.drive_manager.get_drive(&id).await {
        Some(drive) => {
            tracing::info!(target: "api::drives", drive_id = %id, "Drive found");
            Ok(Json(ApiResponse::success(drive)))
        }
        None => {
            tracing::warn!(target: "api::drives", drive_id = %id, "Drive not found");
            Err(AppError::NotFound(format!("Drive not found: {}", id)))
        }
    }
}

/// Add a new drive
pub async fn add_drive(
    State(state): State<AppState>,
    Json(req): Json<AddDriveRequest>,
) -> Result<Json<ApiResponse<DriveConfig>>, AppError> {
    let drive_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(target: "api::drives", drive_id = %drive_id, name = %req.name, "Adding new drive");

    let config = DriveConfig {
        id: drive_id.clone(),
        name: req.name.clone(),
        drive_type: req.drive_type,
        sync_path: req.sync_path.into(),
        enabled: req.enabled,
        extra: req.extra,
    };

    state.drive_manager.add_drive(config.clone()).await?;

    // Persist changes
    state.drive_manager.persist().await?;

    // Broadcast event
    state
        .event_broadcaster
        .drive_added(drive_id.clone(), req.name);

    tracing::info!(target: "api::drives", drive_id = %drive_id, "Drive added successfully");
    Ok(Json(ApiResponse::success(config)))
}

/// Update a drive
pub async fn update_drive(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDriveRequest>,
) -> Result<Json<ApiResponse<DriveConfig>>, AppError> {
    tracing::info!(target: "api::drives", drive_id = %id, "Updating drive");

    // Get existing drive
    let mut drive = state
        .drive_manager
        .get_drive(&id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Drive not found: {}", id)))?;

    // Update fields
    if let Some(name) = req.name {
        drive.name = name;
    }
    if let Some(sync_path) = req.sync_path {
        drive.sync_path = sync_path.into();
    }
    if let Some(enabled) = req.enabled {
        drive.enabled = enabled;
    }
    if let Some(extra) = req.extra {
        drive.extra = extra;
    }

    state.drive_manager.update_drive(&id, drive.clone()).await?;

    // Persist changes
    state.drive_manager.persist().await?;

    // Broadcast event
    state.event_broadcaster.drive_updated(id.clone());

    tracing::info!(target: "api::drives", drive_id = %id, "Drive updated successfully");
    Ok(Json(ApiResponse::success(drive)))
}

/// Remove a drive
pub async fn remove_drive(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    tracing::info!(target: "api::drives", drive_id = %id, "Removing drive");

    match state.drive_manager.remove_drive(&id).await? {
        Some(_) => {
            // Persist changes
            state.drive_manager.persist().await?;

            // Broadcast event
            state.event_broadcaster.drive_removed(id.clone());

            tracing::info!(target: "api::drives", drive_id = %id, "Drive removed successfully");
            Ok(Json(ApiResponse::success(serde_json::json!({
                "drive_id": id,
                "removed": true
            }))))
        }
        None => {
            tracing::warn!(target: "api::drives", drive_id = %id, "Drive not found for removal");
            Err(AppError::NotFound(format!("Drive not found: {}", id)))
        }
    }
}

/// Execute sync command (start/stop)
pub async fn sync_command(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SyncCommandRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    tracing::info!(target: "api::sync", drive_id = %id, action = %req.action, "Sync command received");

    // Verify drive exists
    state
        .drive_manager
        .get_drive(&id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Drive not found: {}", id)))?;

    match req.action.as_str() {
        "start" => {
            state.drive_manager.start_sync(&id).await?;
            state.event_broadcaster.sync_started(id.clone());
            tracing::info!(target: "api::sync", drive_id = %id, "Sync started");
            Ok(Json(ApiResponse::success(serde_json::json!({
                "drive_id": id,
                "action": "started"
            }))))
        }
        "stop" => {
            state.drive_manager.stop_sync(&id).await?;
            tracing::info!(target: "api::sync", drive_id = %id, "Sync stopped");
            Ok(Json(ApiResponse::success(serde_json::json!({
                "drive_id": id,
                "action": "stopped"
            }))))
        }
        _ => {
            tracing::warn!(target: "api::sync", drive_id = %id, action = %req.action, "Invalid sync action");
            Err(AppError::BadRequest(format!(
                "Invalid action: {}",
                req.action
            )))
        }
    }
}

/// Get sync status for a drive
pub async fn get_sync_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    tracing::debug!(target: "api::sync", drive_id = %id, "Getting sync status");

    // Verify drive exists
    state
        .drive_manager
        .get_drive(&id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Drive not found: {}", id)))?;

    let status = state.drive_manager.get_sync_status(&id).await?;
    Ok(Json(ApiResponse::success(status)))
}

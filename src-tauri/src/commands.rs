use crate::AppState;
use cloudreve_sync::DriveConfig;
use tauri::State;

/// Result type for Tauri commands
type CommandResult<T> = Result<T, String>;

/// List all configured drives
#[tauri::command]
pub async fn list_drives(state: State<'_, AppState>) -> CommandResult<Vec<DriveConfig>> {
    Ok(state.drive_manager.list_drives().await)
}

/// Add a new drive configuration
#[tauri::command]
pub async fn add_drive(state: State<'_, AppState>, config: DriveConfig) -> CommandResult<String> {
    state
        .drive_manager
        .add_drive(config)
        .await
        .map_err(|e| e.to_string())
}

/// Remove a drive by ID
#[tauri::command]
pub async fn remove_drive(
    state: State<'_, AppState>,
    drive_id: String,
) -> CommandResult<Option<DriveConfig>> {
    state
        .drive_manager
        .remove_drive(&drive_id)
        .await
        .map_err(|e| e.to_string())
}

/// Get sync status for a drive
#[tauri::command]
pub async fn get_sync_status(
    state: State<'_, AppState>,
    drive_id: String,
) -> CommandResult<serde_json::Value> {
    state
        .drive_manager
        .get_sync_status(&drive_id)
        .await
        .map_err(|e| e.to_string())
}

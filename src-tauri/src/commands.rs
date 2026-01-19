use crate::AppStateHandle;
use chrono::{Duration, Utc};
use cloudreve_sync::{Credentials, DriveConfig, StatusSummary};
#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;
use tauri::{
    utils::{config::WindowEffectsConfig, WindowEffect},
    webview::WebviewWindowBuilder,
    AppHandle, Manager, State, WebviewUrl,
};
use tauri_plugin_frame::WebviewWindowExt;
use tauri_plugin_positioner::{WindowExt, Position};
use uuid::Uuid;

/// Result type for Tauri commands
type CommandResult<T> = Result<T, String>;

/// List all configured drives
#[tauri::command]
pub async fn list_drives(state: State<'_, AppStateHandle>) -> CommandResult<Vec<DriveConfig>> {
    let app_state = state
        .get()
        .ok_or_else(|| "App not yet initialized".to_string())?;
    Ok(app_state.drive_manager.list_drives().await)
}

#[derive(serde::Deserialize)]
pub struct AddDriveArgs {
    pub site_url: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires: u64,
    pub refresh_token_expires: u64,
    pub drive_name: String,
    pub remote_path: String,
    pub local_path: String,
    pub user_id: String,
}

/// Add a new drive configuration
#[tauri::command]
pub async fn add_drive(
    state: State<'_, AppStateHandle>,
    config: AddDriveArgs,
) -> CommandResult<String> {
    let app_state = state
        .get()
        .ok_or_else(|| "App not yet initialized".to_string())?;

    // Generate a new UUID for the drive
    let drive_id = Uuid::new_v4().to_string();

    // Convert relative expiry times (seconds) to absolute RFC3339 timestamps
    let now = Utc::now();
    let access_expires = (now + Duration::seconds(config.access_token_expires as i64)).to_rfc3339();
    let refresh_expires =
        (now + Duration::seconds(config.refresh_token_expires as i64)).to_rfc3339();

    let drive_config = DriveConfig {
        id: drive_id,
        name: config.drive_name,
        instance_url: config.site_url,
        remote_path: config.remote_path,
        credentials: Credentials {
            access_token: Some(config.access_token),
            refresh_token: config.refresh_token,
            access_expires: Some(access_expires),
            refresh_expires,
        },
        sync_path: config.local_path.into(),
        icon_path: None,
        enabled: true,
        user_id: config.user_id,
        sync_root_id: None,
        ignore_patterns: Vec::new(),
        extra: Default::default(),
    };

    // Add drive to manager
    let id = app_state
        .drive_manager
        .add_drive(drive_config)
        .await
        .map_err(|e| e.to_string())?;

    // Persist drive configurations
    app_state
        .drive_manager
        .persist()
        .await
        .map_err(|e| e.to_string())?;

    Ok(id)
}

/// Remove a drive by ID
#[tauri::command]
pub async fn remove_drive(
    state: State<'_, AppStateHandle>,
    drive_id: String,
) -> CommandResult<Option<DriveConfig>> {
    let app_state = state
        .get()
        .ok_or_else(|| "App not yet initialized".to_string())?;
    app_state
        .drive_manager
        .remove_drive(&drive_id)
        .await
        .map_err(|e| e.to_string())
}

/// Get sync status for a drive
#[tauri::command]
pub async fn get_sync_status(
    state: State<'_, AppStateHandle>,
    drive_id: String,
) -> CommandResult<serde_json::Value> {
    let app_state = state
        .get()
        .ok_or_else(|| "App not yet initialized".to_string())?;
    app_state
        .drive_manager
        .get_sync_status(&drive_id)
        .await
        .map_err(|e| e.to_string())
}

/// Get status summary including all drives and recent tasks
#[tauri::command]
pub async fn get_status_summary(
    state: State<'_, AppStateHandle>,
    drive_id: Option<String>,
) -> CommandResult<StatusSummary> {
    let app_state = state
        .get()
        .ok_or_else(|| "App not yet initialized".to_string())?;
    app_state
        .drive_manager
        .get_status_summary(drive_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Show or create the main window
pub fn show_main_window(app: &AppHandle) {
    // Check if window already exists
    if let Some(window) = app.get_webview_window("main_popup") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }

    // Create new main window
    match WebviewWindowBuilder::new(app, "main_popup", WebviewUrl::App("index.html/#/popup".into()))
        .title("Cloudreve")
        .inner_size(370.0, 530.0)
        .resizable(false)
        .visible(false)
        .decorations(false)
        .minimizable(false)
        .build()
    {
        Ok(window) => {
            let _ = window.move_window(Position::TrayCenter);
            let _ = window.show();
            let _ = window.set_focus();
        }
        Err(e) => {
            tracing::error!(target: "main_popup", error = %e, "Failed to create main window");
        }
    }
}

/// Show or create the add-drive window
pub fn show_add_drive_window(app: &AppHandle) {
    // Check if window already exists
    if let Some(window) = app.get_webview_window("add-drive") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }

    // Create new add-drive window with mica effect
    let effects = WindowEffectsConfig {
        effects: vec![WindowEffect::Mica, WindowEffect::Acrylic],
        state: None,
        radius: None,
        color: None,
    };

    let builder = WebviewWindowBuilder::new(
        app,
        "add-drive",
        WebviewUrl::App("index.html/#/add-drive".into()),
    )
    .title("Add Drive")
    .inner_size(470.0, 630.0)
    .resizable(false)
    .visible(true)
    .transparent(true)
    .effects(effects)
    .decorations(false)
    .minimizable(false);

    // Platform-specific: title_bar_style and hidden_title are macOS-only
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(TitleBarStyle::Overlay)
        .hidden_title(true);

    match builder.build() {
        Ok(window) => {
            let _ = window.create_overlay_titlebar();
            let _ = window.set_focus();
        }
        Err(e) => {
            tracing::error!(target: "main", error = %e, "Failed to create add-drive window");
        }
    }
}

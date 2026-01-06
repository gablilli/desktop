use crate::drive::commands::{ConflictAction, ManagerCommand};
use crate::drive::manager::DriveManager;
use crate::inventory::ConflictState;
use crate::utils::app::AppRoot;
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use rust_i18n::t;
use std::sync::{Arc, Mutex};
use windows::{
    Win32::{Foundation::*, System::Com::*, UI::Shell::*},
    core::*,
};

/// Parent command that shows the "Resolve conflicts" submenu
#[implement(IExplorerCommand)]
pub struct ResolveConflictCommandHandler {
    drive_manager: Arc<DriveManager>,
    app_root: AppRoot,
}

impl ResolveConflictCommandHandler {
    pub fn new(drive_manager: Arc<DriveManager>, app_root: AppRoot) -> Self {
        Self {
            drive_manager,
            app_root,
        }
    }

    /// Check if the selected file has a pending conflict state
    fn has_pending_conflict(&self, items: Option<&IShellItemArray>) -> bool {
        let Some(items) = items else {
            return false;
        };

        unsafe {
            let count = match items.GetCount() {
                Ok(c) => c,
                Err(_) => return false,
            };

            // Only show for single file selection
            if count != 1 {
                return false;
            }

            let item = match items.GetItemAt(0) {
                Ok(i) => i,
                Err(_) => return false,
            };

            let display_name = match item.GetDisplayName(SIGDN_FILESYSPATH) {
                Ok(d) => d,
                Err(_) => return false,
            };

            let path_str = match display_name.to_string() {
                Ok(s) => s,
                Err(_) => return false,
            };

            // Query the inventory for conflict state
            let inventory = self.drive_manager.get_inventory();
            match inventory.query_by_path(&path_str) {
                Ok(Some(metadata)) => {
                    matches!(metadata.conflict_state, Some(ConflictState::Pending))
                }
                _ => false,
            }
        }
    }
}

impl IExplorerCommand_Impl for ResolveConflictCommandHandler_Impl {
    fn GetTitle(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let title = t!("resolveConflicts");
        let hstring = HSTRING::from(title.as_ref());
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetIcon(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let icon_path = format!("{}\\conflict1.ico", self.app_root.image_path());
        let hstring = HSTRING::from(icon_path);
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetToolTip(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        Err(Error::from(E_NOTIMPL))
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(GUID::from_u128(0x7b8f3c21_e5a4_4d89_b612_9f3e8c7a1b54))
    }

    fn GetState(&self, items: Option<&IShellItemArray>, _oktobeslow: BOOL) -> Result<u32> {
        if self.has_pending_conflict(items) {
            Ok(ECS_ENABLED.0 as u32)
        } else {
            Ok(ECS_HIDDEN.0 as u32)
        }
    }

    fn Invoke(
        &self,
        _selection: Option<&IShellItemArray>,
        _bindctx: Option<&IBindCtx>,
    ) -> Result<()> {
        // Parent command with subcommands should not be invoked directly
        Ok(())
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok((ECF_HASSUBCOMMANDS.0) as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Ok(
            ResolveConflictSubCommands::new(self.drive_manager.clone(), self.app_root.clone())
                .into(),
        )
    }
}

/// Enumerator for the conflict resolution subcommands
#[implement(IEnumExplorerCommand)]
pub struct ResolveConflictSubCommands {
    current: Mutex<usize>,
    drive_manager: Arc<DriveManager>,
    app_root: AppRoot,
}

impl ResolveConflictSubCommands {
    pub fn new(drive_manager: Arc<DriveManager>, app_root: AppRoot) -> Self {
        Self {
            current: Mutex::new(0),
            drive_manager,
            app_root,
        }
    }
}

type ConflictSubCommandFactory = fn(Arc<DriveManager>, AppRoot) -> IExplorerCommand;

fn create_keep_remote_command(
    drive_manager: Arc<DriveManager>,
    app_root: AppRoot,
) -> IExplorerCommand {
    ConflictActionCommandHandler::new(
        drive_manager,
        app_root,
        ConflictAction::KeepRemote,
        "acceptIncomming",
        "sync-from1.ico",
        GUID::from_u128(0x1a2b3c4d_5e6f_7890_abcd_ef1234567890),
    )
    .into()
}

fn create_overwrite_remote_command(
    drive_manager: Arc<DriveManager>,
    app_root: AppRoot,
) -> IExplorerCommand {
    ConflictActionCommandHandler::new(
        drive_manager,
        app_root,
        ConflictAction::OverwriteRemote,
        "overwriteRemote",
        "sync-to1.ico",
        GUID::from_u128(0x2b3c4d5e_6f78_90ab_cdef_123456789012),
    )
    .into()
}

fn create_save_as_new_command(
    drive_manager: Arc<DriveManager>,
    app_root: AppRoot,
) -> IExplorerCommand {
    ConflictActionCommandHandler::new(
        drive_manager,
        app_root,
        ConflictAction::SaveAsNew,
        "saveAsNew",
        "savenew1.ico",
        GUID::from_u128(0x3c4d5e6f_7890_abcd_ef12_345678901234),
    )
    .into()
}

const CONFLICT_SUB_COMMAND_FACTORIES: [ConflictSubCommandFactory; 3] = [
    create_keep_remote_command,
    create_overwrite_remote_command,
    create_save_as_new_command,
];

impl IEnumExplorerCommand_Impl for ResolveConflictSubCommands_Impl {
    fn Clone(&self) -> windows::core::Result<IEnumExplorerCommand> {
        let current = *self.current.lock().unwrap();
        Ok(ComObject::new(ResolveConflictSubCommands {
            current: Mutex::new(current),
            drive_manager: self.drive_manager.clone(),
            app_root: self.app_root.clone(),
        })
        .to_interface())
    }

    fn Next(
        &self,
        count: u32,
        mut commands: *mut Option<IExplorerCommand>,
        fetched: *mut u32,
    ) -> HRESULT {
        if count == 0 {
            if !fetched.is_null() {
                unsafe {
                    fetched.write(0);
                }
            }
            return S_OK;
        }

        if commands.is_null() {
            return E_POINTER;
        }

        let requested = count;
        let mut remaining = count as usize;
        let mut produced = 0u32;
        let mut current = self.current.lock().unwrap();

        while remaining > 0 && *current < CONFLICT_SUB_COMMAND_FACTORIES.len() {
            let factory = CONFLICT_SUB_COMMAND_FACTORIES[*current];
            let command = factory(self.drive_manager.clone(), self.app_root.clone());
            unsafe {
                commands.write(Some(command));
                commands = commands.add(1);
            }
            *current += 1;
            remaining -= 1;
            produced += 1;
        }

        if !fetched.is_null() {
            unsafe {
                fetched.write(produced);
            }
        }

        if produced == requested { S_OK } else { S_FALSE }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        let mut current = self.current.lock().unwrap();
        *current = 0;
        Ok(())
    }

    fn Skip(&self, count: u32) -> windows::core::Result<()> {
        let mut current = self.current.lock().unwrap();
        let len = CONFLICT_SUB_COMMAND_FACTORIES.len();
        *current = (*current + count as usize).min(len);
        Ok(())
    }
}

/// Handler for individual conflict action commands
#[implement(IExplorerCommand)]
pub struct ConflictActionCommandHandler {
    drive_manager: Arc<DriveManager>,
    app_root: AppRoot,
    action: ConflictAction,
    title_key: &'static str,
    icon_name: &'static str,
    guid: GUID,
}

impl ConflictActionCommandHandler {
    pub fn new(
        drive_manager: Arc<DriveManager>,
        app_root: AppRoot,
        action: ConflictAction,
        title_key: &'static str,
        icon_name: &'static str,
        guid: GUID,
    ) -> Self {
        Self {
            drive_manager,
            app_root,
            action,
            title_key,
            icon_name,
            guid,
        }
    }
}

impl IExplorerCommand_Impl for ConflictActionCommandHandler_Impl {
    fn GetTitle(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let title = t!(self.title_key);
        let hstring = HSTRING::from(title.as_ref());
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetIcon(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let icon_path = format!("{}\\{}", self.app_root.image_path(), self.icon_name);
        let hstring = HSTRING::from(icon_path);
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetToolTip(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        Err(Error::from(E_NOTIMPL))
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(self.guid)
    }

    fn GetState(&self, _items: Option<&IShellItemArray>, _oktobeslow: BOOL) -> Result<u32> {
        Ok(ECS_ENABLED.0 as u32)
    }

    fn Invoke(
        &self,
        selection: Option<&IShellItemArray>,
        _bindctx: Option<&IBindCtx>,
    ) -> Result<()> {
        tracing::debug!(
            target: "shellext::context_menu",
            action = ?self.action,
            "Conflict resolution command invoked"
        );

        let Some(items) = selection else {
            return Ok(());
        };

        unsafe {
            let count = items.GetCount()?;
            if count != 1 {
                return Ok(());
            }

            let item = items.GetItemAt(0)?;
            let display_name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path_str = display_name.to_string()?;

            tracing::debug!(
                target: "shellext::context_menu",
                path = %path_str,
                action = ?self.action,
                "Resolving conflict"
            );

            // Query the inventory to get file_id and drive_id
            let inventory = self.drive_manager.get_inventory();
            let file_meta = match inventory.query_by_path(&path_str) {
                Ok(Some(meta)) => meta,
                Ok(None) => {
                    tracing::warn!(
                        target: "shellext::context_menu",
                        path = %path_str,
                        "File not found in inventory"
                    );
                    return Ok(());
                }
                Err(e) => {
                    tracing::error!(
                        target: "shellext::context_menu",
                        path = %path_str,
                        error = %e,
                        "Failed to query inventory"
                    );
                    return Ok(());
                }
            };

            // Encode path for transport (same as toast.rs)
            let encoded_path = URL_SAFE.encode(path_str.as_bytes());

            // Send command through channel to async processor
            let command_tx = self.drive_manager.get_command_sender();
            if let Err(e) = command_tx.send(ManagerCommand::ResolveConflict {
                drive_id: file_meta.drive_id.to_string(),
                file_id: file_meta.id,
                path: encoded_path,
                action: self.action,
            }) {
                tracing::error!(
                    target: "shellext::context_menu",
                    error = %e,
                    "Failed to send ResolveConflict command"
                );
            }
        }

        Ok(())
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(Error::from(E_NOTIMPL))
    }
}

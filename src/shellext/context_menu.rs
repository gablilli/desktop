// Context menu handler for Windows Explorer
// This implements a COM object that provides a custom context menu item

use std::path::PathBuf;
use std::sync::Arc;

use windows::{
    Win32::{Foundation::*, System::Com::*, UI::Shell::*},
    core::*,
};

use crate::drive::commands::ManagerCommand;
use crate::drive::manager::DriveManager;
use rust_i18n::t;
use windows::ApplicationModel;

// UUID for our context menu handler - matches the C++ implementation
pub const CLSID_TEST_EXPLORER_COMMAND: GUID =
    GUID::from_u128(0x165cd069_d9c8_42b4_8e37_b6971afa4494);

#[implement(IExplorerCommand)]
pub struct TestExplorerCommandHandler {
    drive_manager: Arc<DriveManager>,
    images_path: String,
    #[allow(dead_code)]
    site: std::sync::Mutex<Option<IUnknown>>,
}

pub fn get_images_path() -> Result<String> {
    Ok(format!(
        "{}\\Images",
        ApplicationModel::Package::Current()?
            .InstalledLocation()?
            .Path()?
            .to_string(),
    ))
}

impl TestExplorerCommandHandler {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self {
            drive_manager,
            images_path: get_images_path().unwrap_or_default(),
            site: std::sync::Mutex::new(None),
        }
    }
}

impl IExplorerCommand_Impl for TestExplorerCommandHandler_Impl {
    fn GetTitle(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let title = t!("viewOnline");
        let hstring = HSTRING::from(title.as_ref());
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetIcon(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let icon_path = format!("{}\\web3.png", self.images_path);
        let hstring = HSTRING::from(icon_path);
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetToolTip(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        Err(Error::from(E_NOTIMPL))
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Err(Error::from(E_NOTIMPL))
    }

    fn GetState(&self, items: Option<&IShellItemArray>, _oktobeslow: BOOL) -> Result<u32> {
        let Some(items) = items else {
            // Not select anthing, but still triggerd from a folder
            return Ok(ECS_ENABLED.0 as u32);
        };

        unsafe {
            let count = items.GetCount()?;
            if count <= 1 {
                Ok(ECS_ENABLED.0 as u32)
            } else {
                Ok(ECS_HIDDEN.0 as u32)
            }
        }
    }

    fn Invoke(
        &self,
        selection: Option<&IShellItemArray>,
        _bindctx: Option<&IBindCtx>,
    ) -> Result<()> {
        tracing::debug!(target: "shellext::context_menu", "View online context menu command invoked");

        if let Some(items) = selection {
            unsafe {
                let count = items.GetCount()?;
                if count != 1 {
                    return Ok(());
                }

                // Get the first item
                let item = items.GetItemAt(0)?;
                let display_name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                let path_str = display_name.to_string()?;
                let path = PathBuf::from(path_str.clone());

                tracing::debug!(target: "shellext::context_menu", path = %path_str, "View online requested");

                // Send command through channel to async processor
                let command_tx = self.drive_manager.get_command_sender();

                if let Err(e) = command_tx.send(ManagerCommand::ViewOnline { path: path.clone() }) {
                    tracing::error!(target: "shellext::context_menu", error = %e, "Failed to send ViewOnline command");
                }
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

// Class factory for creating instances of our context menu handler
#[implement(IClassFactory)]
pub struct TestExplorerCommandFactory {
    drive_manager: Arc<DriveManager>,
}

impl TestExplorerCommandFactory {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self { drive_manager }
    }
}

impl IClassFactory_Impl for TestExplorerCommandFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        result: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(Error::from(CLASS_E_NOAGGREGATION));
        }

        let handler = TestExplorerCommandHandler::new(self.drive_manager.clone());
        let handler: IUnknown = handler.into();

        unsafe { handler.query(iid, result).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

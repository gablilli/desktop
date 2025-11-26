// Context menu handler for Windows Explorer
// This implements a COM object that provides a custom context menu item
use crate::drive::commands::ManagerCommand;
use crate::drive::manager::DriveManager;
use rust_i18n::t;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use windows::ApplicationModel;
use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
use windows::{
    Win32::{Foundation::*, System::Com::*, UI::Shell::*},
    core::*,
};

// UUID for our context menu handler - matches the C++ implementation
pub const CLSID_EXPLORER_COMMAND: GUID = GUID::from_u128(0x165cd069_d9c8_42b4_8e37_b6971afa4494);

pub fn get_images_path() -> Result<String> {
    Ok(format!(
        "{}\\Images",
        ApplicationModel::Package::Current()?
            .InstalledLocation()?
            .Path()?
            .to_string(),
    ))
}

struct SubCommandsData {
    commands: Vec<IExplorerCommand>,
    current: usize,
}

#[implement(IEnumExplorerCommand)]
pub struct SubCommands {
    inner: Mutex<SubCommandsData>,
    drive_manager: Arc<DriveManager>,
}

impl SubCommands {
    pub fn new(drive_manager: Arc<DriveManager>, commands: Vec<IExplorerCommand>) -> Self {
        Self {
            inner: Mutex::new(SubCommandsData {
                commands,
                current: 0,
            }),
            drive_manager: drive_manager.clone(),
        }
    }
}

impl IEnumExplorerCommand_Impl for SubCommands_Impl {
    fn Clone(&self) -> windows::core::Result<IEnumExplorerCommand> {
        tracing::trace!(target: "shellext::context_menu:sub_commands", "Clone called");
        let inner = self.inner.lock().unwrap();
        Ok(ComObject::new(SubCommands {
            inner: Mutex::new(SubCommandsData {
                commands: inner.commands.clone(),
                current: inner.current,
            }),
            drive_manager: self.drive_manager.clone(),
        })
        .to_interface())
    }

    fn Next(
        &self,
        mut count: u32,
        mut commands: *mut Option<IExplorerCommand>,
        fetched: *mut u32,
    ) -> HRESULT {
        tracing::trace!(target: "shellext::context_menu:sub_commands", count, "Next called");
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

        let mut inner = self.inner.lock().unwrap();
        let mut total_count = 0u32;

        while count > 0 && inner.current < inner.commands.len() {
            let command: ComObject<ViewOnlineCommandHandler> =
                ComObject::new(ViewOnlineCommandHandler::new(self.drive_manager.clone()).into());
            unsafe {
                commands.write(Some(command.to_interface()));
                tracing::trace!(target: "shellext::context_menu:sub_commands", "Next command written");
                commands = commands.add(1);
            }
            inner.current += 1;
            total_count += 1;
            count -= 1;
        }

        if !fetched.is_null() {
            tracing::trace!(target: "shellext::context_menu:sub_commands", total_count, "Total count written");
            unsafe {
                fetched.write(total_count);
            }
        }

        if total_count == 0 || inner.current >= inner.commands.len() {
            S_FALSE
        } else {
            S_OK
        }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        tracing::trace!(target: "shellext::context_menu:sub_commands", "Reset called");
        let mut inner = self.inner.lock().unwrap();
        inner.current = 0;
        Ok(())
    }

    fn Skip(&self, count: u32) -> windows::core::Result<()> {
        tracing::trace!(target: "shellext::context_menu:sub_commands", "Skip called");
        let mut inner = self.inner.lock().unwrap();
        inner.current = (inner.current + count as usize).min(inner.commands.len());
        Ok(())
    }
}

#[implement(IExplorerCommand, IInitializeCommand)]
pub struct ViewOnlineCommandHandler {
    drive_manager: Arc<DriveManager>,
    images_path: String,
    site: Option<IUnknown>,
}

impl ViewOnlineCommandHandler {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self {
            drive_manager,
            images_path: get_images_path().unwrap_or_default(),
            site: None,
        }
    }
}

impl IExplorerCommand_Impl for ViewOnlineCommandHandler_Impl {
    fn GetTitle(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let title = t!("viewOnline");
        let hstring = HSTRING::from(title.as_ref());
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetIcon(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let icon_path = format!("{}\\viewOnline.png", self.images_path);
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

impl IInitializeCommand_Impl for ViewOnlineCommandHandler_Impl {
    fn Initialize(
        &self,
        _command_name: &windows::core::PCWSTR,
        _property_bag: Option<&IPropertyBag>,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

#[implement(IExplorerCommand)]
pub struct CrExplorerCommandHandler {
    drive_manager: Arc<DriveManager>,
    images_path: String,

    #[allow(dead_code)]
    site: std::sync::Mutex<Option<IUnknown>>,
}

impl CrExplorerCommandHandler {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self {
            drive_manager: drive_manager.clone(),
            images_path: get_images_path().unwrap_or_default(),
            site: std::sync::Mutex::new(None),
        }
    }
}

impl IExplorerCommand_Impl for CrExplorerCommandHandler_Impl {
    fn GetTitle(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let hstring = HSTRING::from("Cloudreve");
        unsafe { SHStrDupW(&hstring) }
    }

    fn GetIcon(&self, _items: Option<&IShellItemArray>) -> Result<PWSTR> {
        let icon_path = format!("{}\\cloudreve_menu.png", self.images_path);
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
        Ok(ECS_ENABLED.0 as u32)
    }

    fn Invoke(
        &self,
        selection: Option<&IShellItemArray>,
        _bindctx: Option<&IBindCtx>,
    ) -> Result<()> {
        tracing::debug!(target: "shellext::context_menu", "View online context menu command invoked");
        Ok(())
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok((ECF_DEFAULT.0 | ECF_HASSUBCOMMANDS.0 | ECF_ISDROPDOWN.0) as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        tracing::trace!(target: "shellext::context_menu", "EnumSubCommands called");
        Ok(SubCommands::new(
            self.drive_manager.clone(),
            vec![ViewOnlineCommandHandler::new(self.drive_manager.clone()).into()],
        )
        .into())
    }
}

// Class factory for creating instances of our context menu handler
#[implement(IClassFactory)]
pub struct CrExplorerCommandFactory {
    drive_manager: Arc<DriveManager>,
}

impl CrExplorerCommandFactory {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self { drive_manager }
    }
}

impl IClassFactory_Impl for CrExplorerCommandFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        result: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(Error::from(CLASS_E_NOAGGREGATION));
        }

        let handler = CrExplorerCommandHandler::new(self.drive_manager.clone());
        let handler: IUnknown = handler.into();

        unsafe { handler.query(iid, result).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

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

#[implement(IEnumExplorerCommand)]
pub struct SubCommands {
    current: Mutex<usize>,
    drive_manager: Arc<DriveManager>,
    image_path: String,
}

impl SubCommands {
    pub fn new(drive_manager: Arc<DriveManager>, image_path: String) -> Self {
        Self {
            current: Mutex::new(0),
            drive_manager,
            image_path: get_images_path().unwrap_or_default(),
        }
    }
}

type SubCommandFactory = fn(Arc<DriveManager>, String) -> IExplorerCommand;

impl IEnumExplorerCommand_Impl for SubCommands_Impl {
    fn Clone(&self) -> windows::core::Result<IEnumExplorerCommand> {
        tracing::trace!(target: "shellext::context_menu:sub_commands", "Clone called");
        let current = *self.current.lock().unwrap();
        Ok(ComObject::new(SubCommands {
            current: Mutex::new(current),
            drive_manager: self.drive_manager.clone(),
            image_path: self.image_path.clone(),
        })
        .to_interface())
    }

    fn Next(
        &self,
        count: u32,
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

        let requested = count;
        let mut remaining = count as usize;
        let mut produced = 0u32;
        let mut current = self.current.lock().unwrap();

        while remaining > 0 && *current < SUB_COMMAND_FACTORIES.len() {
            let factory = SUB_COMMAND_FACTORIES[*current];
            let command = factory(self.drive_manager.clone(), self.image_path.clone());
            unsafe {
                commands.write(Some(command));
                tracing::trace!(target: "shellext::context_menu:sub_commands", "Next command written");
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
        tracing::trace!(target: "shellext::context_menu:sub_commands", "Reset called");
        let mut current = self.current.lock().unwrap();
        *current = 0;
        Ok(())
    }

    fn Skip(&self, count: u32) -> windows::core::Result<()> {
        tracing::trace!(target: "shellext::context_menu:sub_commands", "Skip called");
        let mut current = self.current.lock().unwrap();
        let len = SUB_COMMAND_FACTORIES.len();
        *current = (*current + count as usize).min(len);
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
    pub fn new(drive_manager: Arc<DriveManager>, images_path: String) -> Self {
        Self {
            drive_manager,
            images_path,
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
        tracing::trace!(target: "shellext::context_menu:view_online", "GetCanonicalName called");
        Ok(GUID::from_u128(0xe9206944_a659_434b_967b_27e15d2fef20))
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
        Ok(CLSID_EXPLORER_COMMAND)
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
        Ok(SubCommands::new(self.drive_manager.clone(), self.images_path.clone()).into())
    }
}

fn create_view_online_command(
    drive_manager: Arc<DriveManager>,
    images_path: String,
) -> IExplorerCommand {
    ViewOnlineCommandHandler::new(drive_manager, images_path).into()
}

const SUB_COMMAND_FACTORIES: [SubCommandFactory; 1] = [create_view_online_command];

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

use super::{SyncNowCommandHandler, ViewOnlineCommandHandler, get_images_path};
use crate::drive::manager::DriveManager;
use std::sync::{Arc, Mutex};
use windows::{
    Win32::{Foundation::*, UI::Shell::*},
    core::*,
};

#[implement(IEnumExplorerCommand)]
pub struct SubCommands {
    current: Mutex<usize>,
    drive_manager: Arc<DriveManager>,
    image_path: String,
}

impl SubCommands {
    pub fn new(drive_manager: Arc<DriveManager>, _image_path: String) -> Self {
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

macro_rules! sub_command_factory {
    ($name:ident, $handler:ident) => {
        fn $name(drive_manager: Arc<DriveManager>, images_path: String) -> IExplorerCommand {
            $handler::new(drive_manager, images_path).into()
        }
    };
}

sub_command_factory!(create_view_online_command, ViewOnlineCommandHandler);
sub_command_factory!(create_sync_now_command, SyncNowCommandHandler);

const SUB_COMMAND_FACTORIES: [SubCommandFactory; 2] =
    [create_view_online_command, create_sync_now_command];

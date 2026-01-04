use crate::drive::manager::DriveManager;
use crate::inventory::InventoryDb;
use crate::utils::app::{AppRoot, get_app_root};
use std::sync::Arc;
use windows::{
    Win32::{Foundation::*, System::Com::*, UI::Notifications::*},
    core::*,
};

pub const CLSID_TOAST_ACTIVATOR: GUID =
    GUID::from_u128(0xeffe04d9_151d_49da_9eb5_34e01442edfe);

#[implement(INotificationActivationCallback)]
pub struct ToastActivator {
    drive_manager: Arc<DriveManager>,
    inventory: Arc<InventoryDb>,
    app_root: AppRoot,
}

impl ToastActivator {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        let inventory = drive_manager.get_inventory();
        Self {
            drive_manager,
            app_root: get_app_root(),
            inventory,
        }
    }
}

impl INotificationActivationCallback_Impl for ToastActivator_Impl {
    fn Activate(
        &self,
        appusermodelid: &windows_core::PCWSTR,
        invokedargs: &windows_core::PCWSTR,
        data: *const NOTIFICATION_USER_INPUT_DATA,
        count: u32,
    ) -> windows_core::Result<()> {
        tracing::trace!(
            "Toast activated: appusermodelid={:?}, invokedargs={:?}, data={:?}, count={}",
            appusermodelid,
            invokedargs,
            data,
            count
        );
        // Parse the invoked arguments to determine the action
        unsafe {
            if invokedargs.is_null() {
                return Ok(());
            }
            let args = invokedargs.to_string();
            tracing::trace!( ?args,"Toast activated with arguments");
        }

        // Here you can add logic to handle different actions based on args
        // For example, open a specific file or navigate to a URL

        Ok(())
    }
}

// Class factory for creating instances of our toast activator
#[implement(IClassFactory)]
pub struct ToastActivatorFactory {
    drive_manager: Arc<DriveManager>,
}

impl ToastActivatorFactory {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self { drive_manager }
    }
}

impl IClassFactory_Impl for ToastActivatorFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        result: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(Error::from(CLASS_E_NOAGGREGATION));
        }

        let handler = ToastActivator::new(self.drive_manager.clone());
        let handler: IUnknown = handler.into();

        unsafe { handler.query(iid, result).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

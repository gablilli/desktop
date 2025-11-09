use crate::drive::manager::DriveManager;
use crate::utils::app::{AppRoot, get_app_root};
use std::sync::{Arc, Mutex};
use windows::Foundation::{EventRegistrationToken, TypedEventHandler, Uri};
use windows::{
    Storage::Provider::*,
    Win32::{Foundation::*, System::Com::*},
    core::*,
};

// UUID for our custom state handler - matches the C++ implementation
pub const CLSID_STATUS_UI_HANDLER: GUID = GUID::from_u128(0xb1d8ef74_822d_401a_a14a_25f45b1f70b7);

#[implement(IStorageProviderStatusUISource)]
pub struct StatusUIHandler {
    drive_manager: Arc<DriveManager>,
    app_root: AppRoot,
}

impl StatusUIHandler {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self {
            drive_manager,
            app_root: get_app_root(),
        }
    }
}

impl IStorageProviderStatusUISource_Impl for StatusUIHandler_Impl {
    fn GetStatusUI(&self) -> Result<StorageProviderStatusUI> {
        tracing::trace!(target: "shellext::status_ui", "GetStatusUI");
        let mut ui = StorageProviderStatusUI::new()?;
        let image_path = self.app_root.image_path();
        ui.SetProviderState(StorageProviderState::InSync)?;
        ui.SetProviderStateLabel(&HSTRING::from("Synced"))?;
        ui.SetProviderStateIcon(&Uri::CreateUri(&HSTRING::from(format!(
            "{}\\CloudIconSynced.svg",
            image_path
        )))?)?;

        let quota_ui = StorageProviderQuotaUI::new()?;
        quota_ui.SetQuotaUsedInBytes(1024 * 1024 * 10)?;
        quota_ui.SetQuotaUsedInBytes(1024 * 1024 * 5)?;
        quota_ui.SetQuotaUsedLabel(&HSTRING::from("10 MB Used"))?;
        ui.SetQuotaUI(&quota_ui)?;

        Ok(ui)
    }

    fn StatusUIChanged(
        &self,
        handler: Option<
            &TypedEventHandler<IStorageProviderStatusUISource, windows_core::IInspectable>,
        >,
    ) -> windows_core::Result<EventRegistrationToken> {
        if let Some(handler) = handler {
            let source: IStorageProviderStatusUISource = unsafe { self.this.cast()? };
            let handler = UIEvent(handler.clone());

            let _ = self.drive_manager.register_on_status_ui_changed(move || {
                tracing::trace!(target: "shellext::status_ui", "EventRegistrationToken: Invoking status UI changed callback");
                let _ = handler.Invoke(None, None);
            });
        }
        Ok(EventRegistrationToken::default())
    }

    fn RemoveStatusUIChanged(&self, _token: &EventRegistrationToken) -> windows_core::Result<()> {
        Ok(())
    }
}

#[implement(IStorageProviderStatusUISourceFactory)]
pub struct StatusUIHandlerFactory {
    drive_manager: Arc<DriveManager>,
}

impl StatusUIHandlerFactory {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self { drive_manager }
    }
}

impl IStorageProviderStatusUISourceFactory_Impl for StatusUIHandlerFactory_Impl {
    fn GetStatusUISource(&self, syncrootid: &HSTRING) -> Result<IStorageProviderStatusUISource> {
        let handler = StatusUIHandler::new(self.drive_manager.clone());
        let handler: IStorageProviderStatusUISource = handler.into();
        Ok(handler)
    }
}

struct UIEvent(TypedEventHandler<IStorageProviderStatusUISource, windows_core::IInspectable>);
unsafe impl Send for UIEvent {}

impl UIEvent {
    pub fn Invoke(
        &self,
        source: Option<&IStorageProviderStatusUISource>,
        args: Option<&IInspectable>,
    ) -> windows_core::Result<()> {
        self.0.Invoke(source, args)
    }
}

// Class factory for creating instances of our context menu handler
#[implement(IClassFactory)]
pub struct StatusUIHandlerFactoryFactory {
    drive_manager: Arc<DriveManager>,
}

impl StatusUIHandlerFactoryFactory {
    pub fn new(drive_manager: Arc<DriveManager>) -> Self {
        Self { drive_manager }
    }
}

impl IClassFactory_Impl for StatusUIHandlerFactoryFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        iid: *const GUID,
        result: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(Error::from(CLASS_E_NOAGGREGATION));
        }

        let handler = StatusUIHandlerFactory::new(self.drive_manager.clone());
        let handler: IUnknown = handler.into();

        unsafe { handler.query(iid, result).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

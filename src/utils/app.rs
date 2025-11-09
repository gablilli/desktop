use std::sync::{Arc, OnceLock};
use windows::ApplicationModel;

static APP_ROOT: OnceLock<Arc<String>> = OnceLock::new();

pub fn init_app_root() {
    let path = ApplicationModel::Package::Current()
        .and_then(|p| p.InstalledLocation())
        .and_then(|l| l.Path())
        .map(|p| p.to_string())
        .unwrap_or_else(|_| String::new());

    APP_ROOT.set(Arc::new(path)).ok();
}

pub fn get_app_root() -> AppRoot {
    AppRoot(APP_ROOT.get().expect("APP_ROOT not initialized").clone())
}
pub struct AppRoot(Arc<String>);

impl AppRoot {
    pub fn image_path(&self) -> String {
        format!("{}\\Images", self.0.as_str())
    }
}

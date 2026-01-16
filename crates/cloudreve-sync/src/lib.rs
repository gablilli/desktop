pub mod cfapi;
pub mod drive;
pub mod events;
pub mod inventory;
pub mod logging;
pub mod shellext;
pub mod tasks;
pub mod uploader;
pub mod utils;

// Re-export commonly used types
pub use drive::manager::DriveManager;
pub use drive::mounts::DriveConfig;
pub use events::{Event, EventBroadcaster};
pub use logging::{LogConfig, LogGuard};

#[macro_use]
extern crate rust_i18n;

i18n!("../../locales");

/// Initialize the application root path (Windows Package detection)
pub fn init_app_root() {
    utils::app::init_app_root();
}

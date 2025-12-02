mod db;
mod models;
pub(crate) mod schema;

pub use db::InventoryDb;
pub use models::{
    DriveProps, DrivePropsUpdate, FileMetadata, MetadataEntry, NewTaskRecord, TaskRecord,
    TaskStatus, TaskUpdate,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

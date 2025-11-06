use std::path::PathBuf;

use cloudreve_api::models::{explorer::FileResponse, uri::CrUri};
#[derive(Debug, Clone)]
pub struct GetPlacehodlerResult {
    pub files: Vec<FileResponse>,
    pub local_path: PathBuf,
    pub remote_path: CrUri,
}
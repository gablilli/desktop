use std::path::PathBuf;

use anyhow::{Context, Result};
use cloudreve_api::models::uri::CrUri;

pub fn local_path_to_cr_uri(path: PathBuf, root: PathBuf, remote_base: String) -> Result<CrUri> {
    let mut base = CrUri::new(&remote_base)?;
    
    // Strip the root from path to get the relative path
    let relative = path.strip_prefix(&root)
        .context("Path is not under root")?;
    
    // Convert to string with forward slashes (for URI compatibility)
    let relative_str = relative.to_str()
        .context("Path contains invalid UTF-8")?
        .replace("\\", "/");
    
    // Join the relative path to the base URI if not empty
    if !relative_str.is_empty() {
        base.join_raw(&relative_str);
    }
    
    Ok(base)
}

pub fn remote_path_to_local_relative_path(remote_path: &CrUri, remote_base: &CrUri) -> Result<PathBuf> {
    let remote_path_str = remote_path.path().clone();
    let remote_base_str = remote_base.path().clone();
    
    // 1. add ending slash if not presented to remote_base_str
    let remote_base_str = if !remote_base_str.ends_with('/') {
        remote_base_str + "/"
    } else {
        remote_base_str
    };
    
    // 2. remove remote_base_str from remote_path_str
    let relative_path = remote_path_str.strip_prefix(&remote_base_str)
        .context("Path is not under remote base")?;
    
    Ok(PathBuf::from(relative_path))
}

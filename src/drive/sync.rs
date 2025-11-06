use crate::{cfapi::{metadata::Metadata, placeholder_file::PlaceholderFile}, drive::utils::remote_path_to_local_relative_path};
use anyhow::Result;
use chrono::{DateTime, Utc};
use cloudreve_api::models::{
    explorer::{FileResponse, file_type},
    uri::CrUri,
};
use nt_time::FileTime;
use std::{ffi::OsString, path::PathBuf};

pub fn cloud_file_to_placeholder(
    file: &FileResponse,
    local_path: &PathBuf,
    remote_path: &CrUri,
) -> Result<PlaceholderFile> {
    let file_uri = CrUri::new(&file.path)?;
    let relative_path = remote_path_to_local_relative_path(&file_uri, &remote_path)?;
    tracing::trace!(target: "drive::sync", file_uri = %file_uri.to_string(), remote_path = %remote_path.to_string(), relative_path = %relative_path.to_string_lossy(), "Relative path");
    let primary_entity = OsString::from(file.primary_entity.as_ref().unwrap_or(&String::new()));
    // Remove leading slash if presented

    // Parse RFC time string to unix timestamp
    let created_at =
        FileTime::from_unix_time(file.created_at.parse::<DateTime<Utc>>()?.timestamp())?;
        let last_modified =
        FileTime::from_unix_time(file.updated_at.parse::<DateTime<Utc>>()?.timestamp())?;

    Ok(PlaceholderFile::new(relative_path)
        .metadata(
            match file.file_type == file_type::FOLDER {
                true => Metadata::directory(),
                false => Metadata::file(),
            }
            .size(file.size as u64)
            .changed(last_modified)
            .created(created_at),
        )
        .mark_in_sync()
        .overwrite()
        .blob(primary_entity.into_encoded_bytes()))
}

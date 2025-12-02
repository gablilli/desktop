//! Local and Remote policy upload implementation
//!
//! For Local policy: uploads chunks directly to Cloudreve server
//! For Remote policy: uploads chunks to slave nodes

use crate::uploader::chunk::{ChunkInfo, ChunkStream};
use crate::uploader::error::{UploadError, UploadResult};
use crate::uploader::session::UploadSession;
use cloudreve_api::Client as CrClient;
use cloudreve_api::api::ExplorerApi;
use reqwest::{Body, Client as HttpClient};
use std::sync::Arc;
use tracing::debug;

/// Upload a chunk for Local policy (via Cloudreve API)
pub async fn upload_chunk(
    http_client: &HttpClient,
    cr_client: &Arc<CrClient>,
    chunk: &ChunkInfo,
    stream: ChunkStream,
    session: &UploadSession,
) -> UploadResult<Option<String>> {
    // Check if this is a remote (slave) upload
    if let Some(url) = session.upload_url() {
        if !url.is_empty() && !url.starts_with("/") {
            // Remote slave upload
            return upload_chunk_remote(http_client, chunk, stream, session).await;
        }
    }

    // Local upload via Cloudreve API
    upload_chunk_local(cr_client, chunk, stream, session).await
}

/// Upload chunk to local Cloudreve server using streaming body
async fn upload_chunk_local(
    cr_client: &Arc<CrClient>,
    chunk: &ChunkInfo,
    stream: ChunkStream,
    session: &UploadSession,
) -> UploadResult<Option<String>> {
    debug!(
        target: "uploader::local",
        chunk = chunk.index,
        size = chunk.size,
        session_id = session.session_id(),
        "Uploading chunk to Cloudreve (streaming)"
    );

    let body = Body::wrap_stream(stream);

    cr_client
        .upload_chunk_stream(session.session_id(), chunk.index, chunk.size, body)
        .await
        .map_err(|e| UploadError::chunk_failed(chunk.index, e.to_string()))?;

    Ok(None)
}

/// Upload chunk to remote slave node using streaming body
async fn upload_chunk_remote(
    http_client: &HttpClient,
    chunk: &ChunkInfo,
    stream: ChunkStream,
    session: &UploadSession,
) -> UploadResult<Option<String>> {
    let url = session
        .upload_url()
        .ok_or_else(|| UploadError::chunk_failed(chunk.index, "No upload URL"))?;

    debug!(
        target: "uploader::remote",
        chunk = chunk.index,
        size = chunk.size,
        url = %url,
        "Uploading chunk to slave node (streaming)"
    );

    let credential = session.credential_string();
    let upload_url = format!("{}?chunk={}", url, chunk.index);

    let body = Body::wrap_stream(stream);

    let response = http_client
        .post(&upload_url)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", chunk.size)
        .header("Authorization", credential)
        .body(body)
        .send()
        .await
        .map_err(|e| UploadError::chunk_failed(chunk.index, e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(UploadError::chunk_failed(
            chunk.index,
            format!("HTTP {}: {}", status, body),
        ));
    }

    // Parse response to check for errors
    #[derive(serde::Deserialize)]
    struct SlaveResponse {
        code: i32,
        #[serde(default)]
        msg: String,
    }

    let response_text = response.text().await.unwrap_or_default();
    if let Ok(resp) = serde_json::from_str::<SlaveResponse>(&response_text) {
        if resp.code != 0 {
            return Err(UploadError::chunk_failed(
                chunk.index,
                format!("Slave error ({}): {}", resp.code, resp.msg),
            ));
        }
    }

    Ok(None)
}

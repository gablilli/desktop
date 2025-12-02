//! Qiniu Cloud Storage upload implementation

use crate::uploader::chunk::{ChunkInfo, ChunkStream};
use crate::uploader::error::{UploadError, UploadResult};
use crate::uploader::session::UploadSession;
use reqwest::{Body, Client as HttpClient};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Qiniu chunk upload response
#[derive(Debug, Deserialize)]
struct QiniuChunkResponse {
    etag: String,
    #[serde(default)]
    md5: String,
}

/// Qiniu error response
#[derive(Debug, Deserialize)]
struct QiniuError {
    error: String,
}

/// Qiniu completion request part info
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QiniuPartInfo {
    etag: String,
    part_number: usize,
}

/// Qiniu completion request
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QiniuCompleteRequest {
    parts: Vec<QiniuPartInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
}

/// Upload chunk to Qiniu using streaming
pub async fn upload_chunk(
    http_client: &HttpClient,
    chunk: &ChunkInfo,
    stream: ChunkStream,
    session: &UploadSession,
) -> UploadResult<Option<String>> {
    let base_url = session
        .upload_url()
        .ok_or_else(|| UploadError::chunk_failed(chunk.index, "No upload URL"))?;

    // Qiniu uses 1-based part numbers in URL
    let url = format!("{}/{}", base_url, chunk.index + 1);
    let credential = session.credential_string();

    debug!(
        target: "uploader::qiniu",
        chunk = chunk.index,
        size = chunk.size,
        url = %url,
        "Uploading chunk to Qiniu (streaming)"
    );

    let body = Body::wrap_stream(stream);

    let response = http_client
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", chunk.size)
        .header("Authorization", format!("UpToken {}", credential))
        .body(body)
        .send()
        .await
        .map_err(|e| UploadError::chunk_failed(chunk.index, e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        // Try to parse Qiniu error
        if let Ok(error) = serde_json::from_str::<QiniuError>(&body) {
            return Err(UploadError::chunk_failed(
                chunk.index,
                format!("Qiniu error: {}", error.error),
            ));
        }

        return Err(UploadError::chunk_failed(
            chunk.index,
            format!("HTTP {}: {}", status, body),
        ));
    }

    // Parse response to get ETag
    let chunk_response: QiniuChunkResponse = response.json().await.map_err(|e| {
        UploadError::chunk_failed(chunk.index, format!("Failed to parse response: {}", e))
    })?;

    Ok(Some(chunk_response.etag))
}

/// Complete Qiniu multipart upload
pub async fn complete_upload(
    http_client: &HttpClient,
    session: &UploadSession,
) -> UploadResult<()> {
    let url = session
        .upload_url()
        .ok_or_else(|| UploadError::CompletionFailed("No upload URL".to_string()))?;

    let credential = session.credential_string();

    // Build completion request
    let parts: Vec<QiniuPartInfo> = session
        .chunk_progress
        .iter()
        .filter_map(|c| {
            c.etag.as_ref().map(|etag| QiniuPartInfo {
                etag: etag.clone(),
                part_number: c.index + 1,
            })
        })
        .collect();

    let request = QiniuCompleteRequest {
        parts,
        mime_type: session.mime_type().map(|s| s.to_string()),
    };

    debug!(
        target: "uploader::qiniu",
        url = %url,
        parts = request.parts.len(),
        "Completing Qiniu multipart upload"
    );

    let response = http_client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("UpToken {}", credential))
        .json(&request)
        .send()
        .await
        .map_err(|e| UploadError::CompletionFailed(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        // Try to parse Qiniu error
        if let Ok(error) = serde_json::from_str::<QiniuError>(&body) {
            return Err(UploadError::CompletionFailed(format!(
                "Qiniu error: {}",
                error.error
            )));
        }

        return Err(UploadError::CompletionFailed(format!(
            "HTTP {}: {}",
            status, body
        )));
    }

    Ok(())
}

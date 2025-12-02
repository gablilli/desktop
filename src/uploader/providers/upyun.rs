//! Upyun upload implementation
//!
//! Upyun uses form-based upload with policy and authorization

use crate::uploader::chunk::{ChunkInfo, ChunkStream};
use crate::uploader::error::{UploadError, UploadResult};
use crate::uploader::session::UploadSession;
use reqwest::Client as HttpClient;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tracing::debug;

/// Upyun error response
#[derive(Debug, Deserialize)]
struct UpyunError {
    message: String,
    code: i32,
}

/// Upload to Upyun (single request, form-based) using streaming
///
/// Note: Upyun doesn't support chunked uploads in the same way as other providers.
/// The entire file is uploaded in a single form submission.
pub async fn upload_chunk(
    http_client: &HttpClient,
    chunk: &ChunkInfo,
    stream: ChunkStream,
    session: &UploadSession,
) -> UploadResult<Option<String>> {
    // Upyun only supports single-chunk uploads
    if chunk.index != 0 {
        return Err(UploadError::chunk_failed(
            chunk.index,
            "Upyun only supports single-chunk uploads",
        ));
    }

    let url = session
        .upload_url()
        .ok_or_else(|| UploadError::chunk_failed(chunk.index, "No upload URL"))?;

    let policy = session
        .upload_policy()
        .ok_or_else(|| UploadError::chunk_failed(chunk.index, "No upload policy"))?;

    let credential = session.credential_string();

    debug!(
        target: "uploader::upyun",
        size = chunk.size,
        url = %url,
        "Uploading file to Upyun (streaming)"
    );

    // Build multipart form with streaming body
    // Use Part::stream to create a streaming file part
    let body = reqwest::Body::wrap_stream(stream);
    let file_part = Part::stream_with_length(body, chunk.size)
        .file_name("file")
        .mime_str("application/octet-stream")
        .map_err(|e| UploadError::chunk_failed(chunk.index, e.to_string()))?;

    let mut form = Form::new()
        .text("policy", policy.to_string())
        .text("authorization", credential.to_string())
        .part("file", file_part);

    // Add MIME type if available
    if let Some(mime) = session.mime_type() {
        form = form.text("content-type", mime.to_string());
    }

    let response = http_client
        .post(url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| UploadError::chunk_failed(chunk.index, e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        // Try to parse Upyun error
        if let Ok(error) = serde_json::from_str::<UpyunError>(&body) {
            return Err(UploadError::upyun_error(error.code, error.message));
        }

        return Err(UploadError::chunk_failed(
            chunk.index,
            format!("HTTP {}: {}", status, body),
        ));
    }

    Ok(None)
}

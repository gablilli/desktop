use axum::{
    extract::State,
    response::{
        Sse,
        sse::{Event as SseEvent, KeepAlive},
    },
};
use futures::stream::{Stream, StreamExt};
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;

use super::AppState;

/// Server-Sent Events handler for real-time event streaming
pub async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    tracing::info!(target: "api::sse", "New SSE connection established");

    let receiver = state.event_broadcaster.subscribe();
    let stream = BroadcastStream::new(receiver);

    let event_stream = stream.filter_map(|result| async move {
        match result {
            Ok(event) => {
                // Serialize event to JSON
                match serde_json::to_string(&event) {
                    Ok(json) => {
                        tracing::trace!(target: "api::sse", event = %json, "Broadcasting event to SSE client");
                        Some(Ok(SseEvent::default().data(json)))
                    }
                    Err(e) => {
                        tracing::error!(target: "api::sse", error = %e, "Failed to serialize event");
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(target: "api::sse", error = %e, "Broadcast stream error");
                None
            }
        }
    });

    Sse::new(event_stream).keep_alive(KeepAlive::default())
}

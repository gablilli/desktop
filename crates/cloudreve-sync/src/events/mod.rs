use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing;

/// Different types of events that can be broadcast to GUI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    DriveAdded {
        drive_id: String,
        name: String,
    },
    DriveRemoved {
        drive_id: String,
    },
    DriveUpdated {
        drive_id: String,
    },
    SyncStarted {
        drive_id: String,
    },
    SyncCompleted {
        drive_id: String,
        files_synced: u64,
    },
    SyncProgress {
        drive_id: String,
        progress: f32,
        current_file: String,
    },
    SyncError {
        drive_id: String,
        error: String,
    },
    FileUploaded {
        drive_id: String,
        file_path: String,
    },
    FileDownloaded {
        drive_id: String,
        file_path: String,
    },
    ConnectionStatusChanged {
        connected: bool,
    },
    Custom {
        event_name: String,
        payload: serde_json::Value,
    },
}

/// Event broadcaster for Server-Sent Events (SSE)
#[derive(Clone)]
pub struct EventBroadcaster {
    sender: Arc<broadcast::Sender<Event>>,
}

impl EventBroadcaster {
    /// Create a new event broadcaster
    ///
    /// # Arguments
    /// * `capacity` - The capacity of the broadcast channel (default: 100)
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender: Arc::new(sender),
        }
    }

    /// Subscribe to events and get a receiver
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// Broadcast an event to all subscribers
    ///
    /// # Arguments
    /// * `event` - The event to broadcast
    ///
    /// # Returns
    /// The number of receivers that received the event
    pub fn broadcast(&self, event: Event) -> usize {
        match self.sender.send(event.clone()) {
            Ok(count) => {
                tracing::debug!(target: "events", subscribers = count, "Broadcast event to subscriber(s)");
                tracing::trace!(target: "events", event = ?event, "Event details");
                count
            }
            Err(e) => {
                tracing::warn!(target: "events", error = ?e, "Failed to broadcast event (no active subscribers)");
                0
            }
        }
    }

    /// Helper: Broadcast drive added event
    pub fn drive_added(&self, drive_id: String, name: String) {
        self.broadcast(Event::DriveAdded { drive_id, name });
    }

    /// Helper: Broadcast drive removed event
    pub fn drive_removed(&self, drive_id: String) {
        self.broadcast(Event::DriveRemoved { drive_id });
    }

    /// Helper: Broadcast drive updated event
    pub fn drive_updated(&self, drive_id: String) {
        self.broadcast(Event::DriveUpdated { drive_id });
    }

    /// Helper: Broadcast sync started event
    pub fn sync_started(&self, drive_id: String) {
        self.broadcast(Event::SyncStarted { drive_id });
    }

    /// Helper: Broadcast sync completed event
    pub fn sync_completed(&self, drive_id: String, files_synced: u64) {
        self.broadcast(Event::SyncCompleted {
            drive_id,
            files_synced,
        });
    }

    /// Helper: Broadcast sync progress event
    pub fn sync_progress(&self, drive_id: String, progress: f32, current_file: String) {
        self.broadcast(Event::SyncProgress {
            drive_id,
            progress,
            current_file,
        });
    }

    /// Helper: Broadcast sync error event
    pub fn sync_error(&self, drive_id: String, error: String) {
        self.broadcast(Event::SyncError { drive_id, error });
    }

    /// Helper: Broadcast file uploaded event
    pub fn file_uploaded(&self, drive_id: String, file_path: String) {
        self.broadcast(Event::FileUploaded {
            drive_id,
            file_path,
        });
    }

    /// Helper: Broadcast file downloaded event
    pub fn file_downloaded(&self, drive_id: String, file_path: String) {
        self.broadcast(Event::FileDownloaded {
            drive_id,
            file_path,
        });
    }

    /// Helper: Broadcast connection status changed event
    pub fn connection_status_changed(&self, connected: bool) {
        self.broadcast(Event::ConnectionStatusChanged { connected });
    }

    /// Helper: Broadcast custom event
    pub fn custom_event(&self, event_name: String, payload: serde_json::Value) {
        self.broadcast(Event::Custom {
            event_name,
            payload,
        });
    }

    /// Get the number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBroadcaster {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_broadcasting() {
        let broadcaster = EventBroadcaster::new(10);
        let mut receiver = broadcaster.subscribe();

        broadcaster.drive_added("drive-1".to_string(), "My Drive".to_string());

        let event = receiver.recv().await.unwrap();
        match event {
            Event::DriveAdded { drive_id, name } => {
                assert_eq!(drive_id, "drive-1");
                assert_eq!(name, "My Drive");
            }
            _ => panic!("Expected DriveAdded event"),
        }
    }
}

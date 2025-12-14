use crate::{
    cfapi::placeholder::LocalFileInfo,
    drive::{commands::MountCommand, mounts::Mount, sync::SyncMode},
};
use anyhow::{Context, Result};
use cloudreve_api::{
    api::explorer::FileEventsApi,
    models::explorer::{FileEvent, FileEventData, FileEventType},
};
use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};

const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF_SECS: u64 = 1;
const MAX_BACKOFF_SECS: u64 = 32;
const LONG_RETRY_DELAY_SECS: u64 = 3600; // 1 hour

struct BackoffState {
    retry_count: u32,
    current_delay: Duration,
}

impl BackoffState {
    fn new() -> Self {
        Self {
            retry_count: 0,
            current_delay: Duration::from_secs(INITIAL_BACKOFF_SECS),
        }
    }

    fn reset(&mut self) {
        self.retry_count = 0;
        self.current_delay = Duration::from_secs(INITIAL_BACKOFF_SECS);
    }

    fn next_delay(&mut self) -> Option<Duration> {
        if self.retry_count >= MAX_RETRIES {
            return None;
        }
        let delay = self.current_delay;
        self.retry_count += 1;
        self.current_delay =
            Duration::from_secs((self.current_delay.as_secs() * 2).min(MAX_BACKOFF_SECS));
        Some(delay)
    }
}

enum ListenResult {
    Error(anyhow::Error),
    ReconnectRequired,
    StreamEnded,
}

impl Mount {
    pub async fn process_remote_events(s: Arc<Self>) {
        tracing::info!(target: "drive::remote_events", "Listening to remote events");
        let mut backoff = BackoffState::new();

        let sync_path = {
            let config = s.config.read().await;
            config.sync_path.clone()
        };

        loop {
            let result = s.listen_remote_events().await;
            match result {
                ListenResult::ReconnectRequired => {
                    tracing::info!(target: "drive::remote_events", "Reconnect required, re-subscribing immediately");
                    backoff.reset();
                    continue;
                }
                ListenResult::StreamEnded => {
                    tracing::warn!(target: "drive::remote_events", "Event stream ended unexpectedly, reconnecting");
                    backoff.reset();
                    continue;
                }
                ListenResult::Error(e) => {
                    if let Some(delay) = backoff.next_delay() {
                        tracing::error!(
                            target: "drive::remote_events",
                            error = %e,
                            retry_count = backoff.retry_count,
                            delay_secs = delay.as_secs(),
                            "Failed to listen to remote events, retrying"
                        );
                        tokio::time::sleep(delay).await;
                    } else {
                        tracing::error!(
                            target: "drive::remote_events",
                            error = %e,
                            "Max retries reached, waiting 1 hour before retrying. Triggerring full sync..."
                        );
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        let _ = s.command_tx.send(MountCommand::Sync {
                            local_paths: vec![sync_path.clone()],
                            mode: SyncMode::FullHierarchy,
                        });
                        tokio::time::sleep(Duration::from_secs(LONG_RETRY_DELAY_SECS)).await;
                        backoff.reset();
                    }
                }
            }
        }
    }

    async fn listen_remote_events(&self) -> ListenResult {
        let (remote_base, sync_path) = {
            let config = self.config.read().await;
            (config.remote_path.clone(), config.sync_path.clone())
        };

        let mut subscription = match self.cr_client.subscribe_file_events(&remote_base).await {
            Ok(sub) => sub,
            Err(e) => return ListenResult::Error(e.into()),
        };

        loop {
            match subscription.next_event().await {
                Ok(Some(event)) => match event {
                    FileEvent::Event(data) => {
                        tracing::trace!(target: "drive::remote_events", event = ?data, "Handling file event");
                        if let Err(e) = self.handle_file_event(sync_path.clone(), data).await {
                            tracing::error!(target: "drive::remote_events", error = ?e, "Failed to handle file event");
                        }
                    }
                    FileEvent::Resumed => {
                        tracing::debug!(target: "drive::remote_events", "Subscription resumed");
                    }
                    FileEvent::Subscribed => {
                        tracing::info!(target: "drive::remote_events", "New subscribtion, triggger full sync...");
                        let _ = self.command_tx.send(MountCommand::Sync {
                            local_paths: vec![sync_path.clone()],
                            mode: SyncMode::FullHierarchy,
                        });
                    }
                    FileEvent::KeepAlive => {
                        tracing::trace!(target: "drive::remote_events", "Keep-alive");
                    }
                    FileEvent::ReconnectRequired => {
                        tracing::debug!(target: "drive::remote_events", "Reconnect required");
                        return ListenResult::ReconnectRequired;
                    }
                },
                Ok(None) => {
                    return ListenResult::StreamEnded;
                }
                Err(e) => {
                    return ListenResult::Error(e.into());
                }
            }
        }
    }

    async fn handle_file_event(&self, sync_root: PathBuf, event: FileEventData) -> Result<()> {
        let local_from_path = sync_root.join(PathBuf::from(event.from).strip_prefix("/")?);

        match event.event_type {
            FileEventType::Create => {
                self.sync_last_presented_parent(sync_root, local_from_path)
                    .await
            }
            FileEventType::Modify => Ok(()),
            FileEventType::Rename => Ok(()),
            FileEventType::Delete => Ok(()),
        }
    }

    async fn sync_last_presented_parent(
        &self,
        sync_root: PathBuf,
        local_path: PathBuf,
    ) -> Result<()> {
        let mut current_parent: Option<&Path> = None;
        let mut current = local_path.as_path();
        loop {
            current_parent = current.parent();
            if current_parent.is_none() || sync_root.parent() == current_parent {
                tracing::warn!(target: "drive::remote_events",sync_root=%sync_root.display(), local_path=%local_path.display(), "File event is not under sync root, skipping");
                return Ok(());
            }

            let parent_info = LocalFileInfo::from_path(current_parent.unwrap())
                .context("failed to get parent file info")?;
            if parent_info.exists {
                if !parent_info.is_placeholder() || parent_info.is_folder_populated() {
                    tracing::trace!(target: "drive::remote_events", parent_path=%current_parent.unwrap().display(), "Syncing parent path for new event");
                    self.command_tx
                        .send(MountCommand::Sync {
                            local_paths: vec![current.to_path_buf()],
                            mode: SyncMode::PathOnly,
                        })
                        .context("failed to send sync command")?;
                } else {
                    tracing::trace!(target: "drive::remote_events", parent_path=%current_parent.unwrap().display(), "Parent path is a placeholder and not populated, skipping");
                }
                return Ok(());
            }
            
            current = current_parent.unwrap();
        }
    }
}

use crate::drive::mounts::Mount;
use anyhow::Result;
use cloudreve_api::{api::explorer::FileEventsApi, models::explorer::FileEvent};
use std::{sync::Arc, time::Duration};

impl Mount {
    pub async fn process_remote_events(s: Arc<Self>) {
        tracing::info!(target: "drive::remote_events", "Listening to remote events");
        loop {
            let result = Self::listen_remote_events(s.clone()).await;
            if let Err(e) = result {
                tracing::error!(target: "drive::remote_events", error = %e, "Failed to listen to remote events");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }

    async fn listen_remote_events(s: Arc<Self>) -> Result<()> {
        let remote_base = {
            let config = s.config.read().await;
            config.remote_path.clone()
        };
        let mut subscription = s.cr_client.subscribe_file_events(&remote_base).await?;
        while let Some(event) = subscription.next_event().await? {
            match event {
                FileEvent::Event(data) => {
                    tracing::debug!(target: "drive::remote_events", data=?data,"Event from remote");
                }
                FileEvent::Resumed => {
                    tracing::debug!(target: "drive::remote_events", "Connection resumed")
                }
                FileEvent::Subscribed => {
                    tracing::debug!(target: "drive::remote_events", "Subscribed")
                }
                FileEvent::KeepAlive => {
                    tracing::debug!(target: "drive::remote_events", "Keep-alive")
                }
            }
        }
        Ok(())
    }
}

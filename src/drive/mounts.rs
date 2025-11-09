use crate::cfapi::root::{
    Connection, HydrationType, PopulationType, SecurityId, Session, SyncRootId, SyncRootIdBuilder,
    SyncRootInfo,
};
use crate::drive::callback::CallbackHandler;
use crate::drive::commands::MountCommand;
use crate::inventory::InventoryDb;
use crate::tasks::{TaskManager, TaskManagerConfig};
use ::serde::{Deserialize, Serialize};
use anyhow::{Context, Result};
use cloudreve_api::{Client, ClientConfig, models::user::Token};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::spawn;
use tokio::sync::{Mutex, RwLock, mpsc};
use url::Url;
use windows::Storage::Provider::StorageProviderSyncRootManager;
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DriveConfig {
    pub id: String,
    pub name: String,
    pub instance_url: String,
    pub remote_path: String,
    pub credentials: Credentials,
    pub sync_path: PathBuf,
    pub icon_path: Option<String>,
    pub enabled: bool,
    pub user_id: String,

    // Windows CFAPI
    pub sync_root_id: Option<SyncRootId>,

    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Credentials {
    pub access_token: Option<String>,
    pub refresh_token: String,
    pub refresh_expires: String,
    pub access_expires: Option<String>,
}

pub struct Mount {
    queue: Arc<TaskManager>,
    pub config: Arc<RwLock<DriveConfig>>,
    connection: Option<Connection<CallbackHandler>>,
    command_tx: mpsc::UnboundedSender<MountCommand>,
    command_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<MountCommand>>>>,
    processor_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub cr_client: Arc<Client>,
    pub inventory: Arc<InventoryDb>,
    pub id: String,
}

impl Mount {
    pub async fn new(config: DriveConfig, inventory: Arc<InventoryDb>) -> Self {
        let task_config = TaskManagerConfig {
            max_workers: 4,
            completed_buffer_size: 100,
        };
        let task_manager = TaskManager::new(task_config);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        // initialize the client with the credentials
        let client_config = ClientConfig::new(config.instance_url.clone());
        let mut cr_client = Client::new(client_config);
        cr_client
            .set_tokens_with_expiry(&Token {
                access_token: config.credentials.access_token.clone().unwrap_or_default(),
                refresh_token: config.credentials.refresh_token.clone(),
                access_expires: config
                    .credentials
                    .access_expires
                    .clone()
                    .unwrap_or_default(),
                refresh_expires: config.credentials.refresh_expires.clone(),
            })
            .await;
        let command_tx_clone = command_tx.clone();
        // Setup hooks to update the credentials in the config
        cr_client.set_on_credential_refreshed(Arc::new(move |token| {
            let command_tx = command_tx_clone.clone();
                Box::pin(async move {
                    let command = MountCommand::RefreshCredentials { credentials: token };
                    if let Err(e) = command_tx.send(command) {
                        tracing::error!(target: "drive::mounts", error = %e, "Failed to send RefreshCredentials command");
                    }
                })
        }));

        let id = config.id.clone();

        Self {
            config: Arc::new(RwLock::new(config)),
            queue: task_manager,
            connection: None,
            command_tx,
            command_rx: Arc::new(tokio::sync::Mutex::new(Some(command_rx))),
            processor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            cr_client: Arc::new(cr_client),
            inventory: inventory,
            id,
        }
    }

    pub async fn get_config(&self) -> DriveConfig {
        self.config.read().await.clone()
    }

    /// Get the sync path for the drive
    pub async fn get_sync_path(&self) -> PathBuf {
        self.config.read().await.sync_path.clone()
    }

    pub async fn start(&mut self) -> Result<()> {
        if !StorageProviderSyncRootManager::IsSupported()
            .context("Cloud Filter API is not supported")?
        {
            return Err(anyhow::anyhow!("Cloud Filter API is not supported"));
        }

        let mut write_guard = self.config.write().await;

        // if sync root id is not set, generate one
        if write_guard.sync_root_id.is_none() {
            write_guard.sync_root_id = Some(
                generate_sync_root_id(
                    &write_guard.instance_url,
                    &write_guard.name,
                    &write_guard.user_id,
                    &write_guard.sync_path,
                )
                .context("failed to generate sync root id")?,
            );
        }

        drop(write_guard);
        let config = self.config.read().await;

        let sync_root_id = config.sync_root_id.as_ref().unwrap();

        // Register sync root if not registered
        if !sync_root_id.is_registered()? {
            tracing::info!(target: "drive::mounts", id = %self.id, "Registering sync root");
            let mut sync_root_info = SyncRootInfo::default();
            sync_root_info.set_display_name(config.name.clone());
            sync_root_info.set_hydration_type(HydrationType::Progressive);
            sync_root_info.set_population_type(PopulationType::Full);
            if let Some(icon_path) = config.icon_path.as_ref() {
                sync_root_info.set_icon(format!("{},0", icon_path));
            }
            sync_root_info.set_version("1.0.0");
            sync_root_info
                .set_recycle_bin_uri("http://cloudmirror.example.com/recyclebin")
                .context("failed to set recycle bin uri")?;
            sync_root_info
                .set_path(Path::new(&config.sync_path))
                .context("failed to set sync root path")?;
            sync_root_info.add_custom_state(t!("shared").as_ref(), 1)?;
            sync_root_info.add_custom_state(t!("accessible").as_ref(), 2)?;
            sync_root_id
                .register(sync_root_info)
                .context("failed to register sync root")?;
        }

        tracing::info!(target: "drive::mounts",sync_path = %config.sync_path.display(), id = %self.id, "Connecting to sync root");
        let connection = Session::new()
            .connect(
                &config.sync_path,
                CallbackHandler::new(
                    config.clone(),
                    self.command_tx.clone(),
                    self.id.clone(),
                    self.inventory.clone(),
                ),
            )
            .context("failed to connect to sync root")?;

        self.connection = Some(connection);
        Ok(())
    }

    pub async fn spawn_command_processor(&self, s: Arc<Self>) {
        // Spawn the command processor task
        let mut command_rx_guard = self.command_rx.lock().await;
        if let Some(command_rx) = command_rx_guard.take() {
            let mount_id = self.id.to_string();
            let handle = tokio::spawn(async move {
                Self::process_commands(s, mount_id, command_rx).await;
            });
            *self.processor_handle.lock().await = Some(handle);
        }
    }

    /// Process commands from OS threads asynchronously
    async fn process_commands(
        s: Arc<Self>,
        mount_id: String,
        mut command_rx: mpsc::UnboundedReceiver<MountCommand>,
    ) {
        tracing::info!(target: "drive::mounts", id = %mount_id, "Command processor started");

        while let Some(command) = command_rx.recv().await {
            tracing::trace!(target: "drive::mounts", id = %mount_id, command = ?command, "Processing command");

            match command {
                MountCommand::FetchPlaceholders { path, response } => {
                    let s_clone = s.clone();
                    let mount_id_clone = mount_id.clone();
                    spawn(async move {
                        let result = s_clone.fetch_placeholders(path).await;
                        if let Err(e) = result {
                            tracing::error!(target: "drive::mounts", id = %mount_id_clone, error = %e, "Failed to fetch placeholders");
                            let _ = response.send(Err(e));
                            return;
                        }
                        tracing::debug!(target: "drive::mounts", id = %mount_id_clone, result = ?result, "Fetched placeholders");
                        let _ = response.send(result);
                    });
                }
                MountCommand::RefreshCredentials { credentials } => {
                    let mut config = s.config.write().await;
                    config.credentials.access_token = Some(credentials.access_token);
                    config.credentials.refresh_token = credentials.refresh_token;
                    config.credentials.refresh_expires = credentials.refresh_expires;
                    config.credentials.access_expires = Some(credentials.access_expires);
                    drop(config);
                }
            }
        }

        tracing::info!(target: "drive::mounts", id = %mount_id, "Command processor stopped");
    }

    async fn handle_fetch_placeholders(path: PathBuf) -> Result<()> {
        tracing::debug!(target: "drive::mounts", path = %path.display(), "FetchPlaceholders");
        Ok(())
    }

    pub async fn shutdown(&self) {
        tracing::info!(target: "drive::mounts", id=%self.id, "Shutting down Mount");

        // Close the command channel to signal the processor task to stop
        drop(self.command_tx.clone());

        // Wait for the processor task to finish
        if let Some(handle) = self.processor_handle.lock().await.take() {
            tracing::debug!(target: "drive::mounts", id=%self.id, "Waiting for command processor to finish");
            handle.abort();
        }

        if let Some(ref connection) = self.connection {
            connection.disconnect();
        }
        if let Some(sync_root_id) = self.config.read().await.sync_root_id.as_ref() {
            if let Err(e) = sync_root_id.unregister() {
                tracing::warn!(target: "drive::mounts", id=%self.id, error=%e, "Failed to unregister sync root");
            }
        }
        self.queue.shutdown().await;

        if let Err(e) = self.inventory.nuke_drive(&self.id) {
            tracing::error!(target: "drive::mounts", id=%self.id, error=%e, "Failed to nuke drive");
        }
    }
}

fn generate_sync_root_id(
    instance_url: &str,
    account_name: &str,
    user_id: &str,
    sync_path: &PathBuf,
) -> Result<SyncRootId> {
    // Parse the instance URL to get the hostname
    let url = Url::parse(instance_url)?;
    let hostname = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid URL: no host found"))?;

    // Generate a SHA-256 hash of the hostname
    let mut hasher = Sha256::new();
    hasher.update(hostname.as_bytes());
    hasher.update(sync_path.to_string_lossy().as_bytes());
    let hash_result = hasher.finalize();

    // Convert hash to hex string and truncate to reasonable length
    // Use first 16 characters (64 bits) of the hash for the provider name
    let hash_hex = format!("{:x}", hash_result);
    let provider_name = format!("cloudreve{}", &hash_hex[..16]);

    // Build the sync root ID
    let sync_root_id = SyncRootIdBuilder::new(provider_name)
        .user_security_id(SecurityId::current_user()?)
        .account_name(user_id)
        .build();

    Ok(sync_root_id)
}

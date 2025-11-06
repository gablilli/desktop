use crate::{
    cfapi::{
        error::{CResult, CloudErrorKind}, filter::{Filter, Request, SyncFilter, info, ticket}, placeholder_file::PlaceholderFile, root::{
            Connection, HydrationType, PopulationType, SecurityId, Session, SyncRootId,
            SyncRootIdBuilder, SyncRootInfo,
        }
    },
    drive::{interop::GetPlacehodlerResult, sync::cloud_file_to_placeholder, utils::local_path_to_cr_uri},
};
use ::serde::{Deserialize, Serialize};
use anyhow::{Context, Result};
use cloudreve_api::{Client, ClientConfig, api::explorer::ExplorerApiExt, models::{explorer::FileResponse, user::Token}};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, RwLock, mpsc, oneshot::{
    Sender, Receiver,
}};
use url::Url;
use windows::Storage::Provider::StorageProviderSyncRootManager;

use crate::tasks::{TaskManager, TaskManagerConfig};

const PAGE_SIZE: i32 = 1000;

/// Messages sent from OS threads (SyncFilter callbacks) to the async processing task
///
/// # Safety
/// This is safe because Windows CFAPI callbacks are designed to be invoked from arbitrary threads
/// and the data contained in Request, ticket, and info types are meant to be passed between threads
/// during the callback's lifetime.
#[derive(Debug)]
pub enum MountCommand {
    FetchPlaceholders {
        path: PathBuf,
        response: Sender<Result<GetPlacehodlerResult>>,
    },
    RefreshCredentials {
        credentials: Token,
    },
}

// SAFETY: Windows CFAPI is designed to allow callbacks from arbitrary threads.
// The Request, ticket, and info types contain data that is valid for the duration
// of the callback and can be safely transferred between threads.
unsafe impl Send for MountCommand {}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DriveConfig {
    pub id: Option<String>,
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
    config: Arc<RwLock<DriveConfig>>,
    connection: Option<Connection<CallbackHandler>>,
    command_tx: mpsc::UnboundedSender<MountCommand>,
    command_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<MountCommand>>>>,
    processor_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    cr_client: Arc<Client>,
}

impl Mount {
    pub async fn new(config: DriveConfig) -> Self {
        let task_config = TaskManagerConfig {
            max_workers: 4,
            completed_buffer_size: 100,
        };
        let task_manager = TaskManager::new(task_config);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        // initialize the client with the credentials
        let client_config = ClientConfig::new(config.instance_url.clone());
        let mut cr_client  = Client::new(client_config);
        cr_client.set_tokens_with_expiry(&Token {
            access_token: config.credentials.access_token.clone().unwrap_or_default(),
            refresh_token: config.credentials.refresh_token.clone(),
            access_expires: config.credentials.access_expires.clone().unwrap_or_default(),
            refresh_expires: config.credentials.refresh_expires.clone(),
        }).await;
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

        Self {
            config: Arc::new(RwLock::new(config)),
            queue: task_manager,
            connection: None,
            command_tx,
            command_rx: Arc::new(tokio::sync::Mutex::new(Some(command_rx))),
            processor_handle: Arc::new(tokio::sync::Mutex::new(None)),
            cr_client: Arc::new(cr_client),
        }
    }

    pub async fn get_config(&self) -> DriveConfig {
        self.config.read().await.clone()
    }

    pub async fn start(&mut self) -> Result<()> {
        if !StorageProviderSyncRootManager::IsSupported()
            .context("Cloud Filter API is not supported")?
        {
            return Err(anyhow::anyhow!("Cloud Filter API is not supported"));
        }

        let id = self.id().await;
        let mut write_guard = self.config.write().await;

        // if sync root id is not set, generate one
        if write_guard.sync_root_id.is_none() {
            write_guard.sync_root_id = Some(
                generate_sync_root_id(&write_guard.instance_url, &write_guard.name, &write_guard.user_id, &write_guard.sync_path)
                    .context("failed to generate sync root id")?,
            );
        }
        
        drop(write_guard);
        let config = self.config.read().await;

        let sync_root_id = config.sync_root_id.as_ref().unwrap();

        // Register sync root if not registered
        if !sync_root_id.is_registered()? {
            tracing::info!(target: "drive::mounts", id = %id, "Registering sync root");
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
            sync_root_id
                .register(sync_root_info)
                .context("failed to register sync root")?;
        }

        tracing::info!(target: "drive::mounts",sync_path = %config.sync_path.display(), id = %id, "Connecting to sync root");
        let connection = Session::new()
            .connect(
                &config.sync_path,
                CallbackHandler::new(config.clone(), self.command_tx.clone()),
            )
            .context("failed to connect to sync root")?;

        self.connection = Some(connection);
        Ok(())
    }

    pub async fn spawn_command_processor(&self, s: Arc<Self>) {
        // Spawn the command processor task
        let mut command_rx_guard = self.command_rx.lock().await;
        if let Some(command_rx) = command_rx_guard.take() {
            let mount_id = self.id().await;
            let handle = tokio::spawn(async move {
                Self::process_commands(s, mount_id, command_rx).await;
            });
            *self.processor_handle.lock().await = Some(handle);
        }
    }

    pub async fn id(&self) -> String {
        self.config
            .read()
            .await
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
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
                    let result = s.fetch_placeholders(path).await;
                    if let Err(e) = result {
                        tracing::error!(target: "drive::mounts", id = %mount_id, error = %e, "Failed to fetch placeholders");
                        let _ = response.send(Err(e));
                        continue;
                    }
                    tracing::debug!(target: "drive::mounts", id = %mount_id, result = ?result, "Fetched placeholders");
                    let _ = response.send(result);
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
        let id = self.id().await;

        tracing::info!(target: "drive::mounts", id=id, "Shutting down Mount");

        // Close the command channel to signal the processor task to stop
        drop(self.command_tx.clone());

        // Wait for the processor task to finish
        if let Some(handle) = self.processor_handle.lock().await.take() {
            tracing::debug!(target: "drive::mounts", id=id, "Waiting for command processor to finish");
            handle.abort();
        }

        if let Some(ref connection) = self.connection {
            connection.disconnect();
        }
        if let Some(sync_root_id) = self.config.read().await.sync_root_id.as_ref() {
            if let Err(e) = sync_root_id.unregister() {
                tracing::warn!(target: "drive::mounts", id=id, error=%e, "Failed to unregister sync root");
            }
        }
        self.queue.shutdown().await;
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

#[derive(Clone)]
pub struct CallbackHandler {
    config: DriveConfig,
    command_tx: mpsc::UnboundedSender<MountCommand>,
}

impl CallbackHandler {
    pub fn new(config: DriveConfig, command_tx: mpsc::UnboundedSender<MountCommand>) -> Self {
        Self { config, command_tx }
    }

    pub fn id(&self) -> String {
        self.config
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    }

    pub async fn sleep(&self) {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

impl SyncFilter for CallbackHandler {
    fn fetch_data(
        &self,
        request: crate::cfapi::filter::Request,
        ticket: crate::cfapi::filter::ticket::FetchData,
        info: crate::cfapi::filter::info::FetchData,
    ) -> crate::cfapi::error::CResult<()> {
        todo!()
    }

    fn deleted(&self, request: Request, _info: info::Deleted) {
        tracing::debug!(target: "drive::mounts", id = %self.id(), path = %request.path().display(), "Deleted");
    }

    fn delete(&self, request: Request, ticket: ticket::Delete, info: info::Delete) -> CResult<()> {
        tracing::debug!(target: "drive::mounts", id = %self.id(), path = %request.path().display(), "Delete");
        ticket.pass().unwrap();
        Ok(())
    }

    fn rename(&self, request: Request, ticket: ticket::Rename, info: info::Rename) -> CResult<()> {
        let src = request.path();
        let dest = info.target_path();
        tracing::debug!(target: "drive::mounts", id = %self.id(), source_path = %src.display(), target_path = %dest.display(), "Rename");
        Err(CloudErrorKind::NotSupported)
    }

    fn fetch_placeholders(
        &self,
        request: Request,
        ticket: ticket::FetchPlaceholders,
        info: info::FetchPlaceholders,
    ) -> CResult<()> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let command = MountCommand::FetchPlaceholders {
            path: request.path().to_path_buf(),
            response: response_tx,
        };
        if let Err(e) = self.command_tx.send(command) {
            tracing::error!(target: "drive::mounts", id = %self.id(), error = %e, "Failed to send FetchPlaceholders command");
            return Err(CloudErrorKind::NotSupported);
        }

            
         match response_rx.blocking_recv() {
            Ok(Ok(files)) => {
                tracing::debug!(target: "drive::mounts", id = %self.id(), files = %files.files.len(), "Received placeholders");
                let mut placeholders = files.files.iter()
                    .map(|file| cloud_file_to_placeholder(file, &files.local_path, &files.remote_path))
                    .filter_map(|result|{
                        if result.is_ok() {
                            Some(result.unwrap())
                        } else {
                            tracing::error!(target: "drive::mounts", id = %self.id(), error = %result.unwrap_err(), "Failed to convert cloud file to placeholder");
                            None
                        }
                    })
                    .collect::<Vec<PlaceholderFile>>();
                if let Err(e) = ticket.pass_with_placeholder(&mut placeholders) {
                    tracing::error!(target: "drive::mounts", id = %self.id(), error = %e, "Failed to pass placeholders");
                    return Err(CloudErrorKind::Unsuccessful);
                }
                tracing::debug!(target: "drive::mounts", id = %self.id(), placeholders = %placeholders.len(), "Passed placeholders");
                return Ok(());
            }
            _ => {}
        }

        Err(CloudErrorKind::Unsuccessful)
    }

    fn closed(&self, request: Request, info: info::Closed) {
        tracing::debug!(target: "drive::mounts", id = %self.id(), path = %request.path().display(), deleted = %info.deleted(), "Closed");
    }

    fn cancel_fetch_data(&self, _request: Request, _info: info::CancelFetchData) {
        tracing::debug!(target: "drive::mounts", id = %self.id(), "CancelFetchData");
    }

    fn validate_data(
        &self,
        request: Request,
        ticket: ticket::ValidateData,
        info: info::ValidateData,
    ) -> CResult<()> {
        tracing::debug!(target: "drive::mounts", id = %self.id(), "ValidateData");
        Err(CloudErrorKind::NotSupported)
    }

    fn cancel_fetch_placeholders(&self, request: Request, info: info::CancelFetchPlaceholders) {
        tracing::debug!(target: "drive::mounts", id = %self.id(), "CancelFetchPlaceholders");
    }

    fn opened(&self, request: Request, _info: info::Opened) {
        tracing::debug!(target: "drive::mounts", id = %self.id(), path = %request.path().display(), "Opened");
    }

    fn dehydrate(
        &self,
        request: Request,
        ticket: ticket::Dehydrate,
        info: info::Dehydrate,
    ) -> CResult<()> {
        tracing::debug!(
            target: "drive::mounts",
            id = %self.id(),
            reason = ?info.reason(),
            "Dehydrate"
        );
        Err(CloudErrorKind::NotSupported)
    }

    fn dehydrated(&self, _request: Request, info: info::Dehydrated) {
        tracing::debug!(
            target: "drive::mounts",
            id = %self.id(),
            reason = ?info.reason(),
            "Dehydrated"
        );
    }

    fn renamed(&self, _request: Request, info: info::Renamed) {
        let dest = info.source_path();
        tracing::debug!(target: "drive::mounts", id = %self.id(), dest_path = %dest.display(), "Renamed");
    }
}

impl Mount {
    pub async fn fetch_placeholders(&self, path: PathBuf) -> Result<GetPlacehodlerResult> {
        let config = self.config.read().await;
        let remote_base = config.remote_path.clone();
        let sync_path = config.sync_path.clone();
        drop(config);

        let uri = local_path_to_cr_uri(path.clone(), sync_path, remote_base)
            .context("failed to convert local path to cloudreve uri")?;
        let mut placehodlers: Vec<FileResponse> = Vec::new();
        
        let mut previous_response = None;
        loop {
            let response = self.cr_client.list_files_all(previous_response.as_ref(), &uri.to_string(), PAGE_SIZE).await?;
            
            for file in &response.res.files {
                tracing::debug!(target: "drive::mounts", file = %file.name, "Server file");
            }

            placehodlers.extend(response.res.files.clone());
            let has_more: bool = response.more;
            previous_response = Some(response);
            
            if !has_more {
                break;
            }
        }

        tracing::debug!(target: "drive::mounts", uri = %uri.to_string(), "Fetch file list from cloudreve");

        Ok(
            GetPlacehodlerResult {
                files: placehodlers,
                local_path: path.clone(),
                remote_path: uri.clone(),
            }
        )
    }
}

use crate::inventory::{InventoryDb, NewTaskRecord, TaskRecord, TaskStatus, TaskUpdate};
use crate::tasks::types::{TaskKind, TaskPayload, TaskProgress};
use crate::tasks::upload::UploadTask;
use anyhow::{Context, Result, anyhow};
use cloudreve_api::Client;
use dashmap::DashMap;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{
    Mutex, Notify, Semaphore,
    mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const PLACEHOLDER_STEPS: u32 = 500;

#[derive(Debug, Clone)]
pub struct TaskQueueConfig {
    pub max_concurrent: usize,
}

impl Default for TaskQueueConfig {
    fn default() -> Self {
        Self { max_concurrent: 2 }
    }
}

pub struct TaskQueue {
    pub drive_id: String,
    pub cr_client: Arc<Client>,
    pub inventory: Arc<InventoryDb>,
    config: TaskQueueConfig,
    semaphore: Arc<Semaphore>,
    command_tx: UnboundedSender<QueueCommand>,
    dispatcher_handle: Mutex<Option<JoinHandle<()>>>,
    inflight: AtomicUsize,
    idle_notify: Notify,
    shutting_down: AtomicBool,
    cancel_requested: AtomicBool,
    progress: DashMap<String, TaskProgress>,
    task_handles: DashMap<String, JoinHandle<()>>,
}

impl TaskQueue {
    pub async fn new(
        drive_id: impl Into<String>,
        cr_client: Arc<Client>,
        inventory: Arc<InventoryDb>,
        config: TaskQueueConfig,
    ) -> Arc<Self> {
        let drive_id = drive_id.into();
        let max_concurrent = config.max_concurrent.max(1);
        let sanitized_config = TaskQueueConfig { max_concurrent };

        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let queue = Arc::new(Self {
            drive_id,
            inventory,
            cr_client,
            config: sanitized_config,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            command_tx,
            dispatcher_handle: Mutex::new(None),
            inflight: AtomicUsize::new(0),
            idle_notify: Notify::new(),
            shutting_down: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            progress: DashMap::new(),
            task_handles: DashMap::new(),
        });

        queue.spawn_dispatcher(command_rx).await;
        if let Err(err) = queue.resume_incomplete_tasks().await {
            warn!(
                target: "tasks::queue",
                drive = %queue.drive_id,
                error = %err,
                "Failed to resume pending tasks from inventory"
            );
        }
        queue
    }

    pub fn max_concurrent(&self) -> usize {
        self.config.max_concurrent
    }

    pub fn drive_id(&self) -> &str {
        &self.drive_id
    }

    pub async fn enqueue(&self, payload: TaskPayload) -> Result<String> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(anyhow!("task queue is shutting down"));
        }

        let task_id = payload
            .task_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let mut record = NewTaskRecord::new(
            task_id.clone(),
            self.drive_id.clone(),
            payload.kind.as_str().to_string(),
            payload.local_path_display(),
        )
        .with_priority(payload.priority);

        if let Some(uri) = payload.remote_uri.clone() {
            record = record.with_remote_uri(uri);
        }

        match (payload.total_bytes, payload.processed_bytes) {
            (Some(total), Some(processed)) => {
                record = record.with_totals(total, processed);
            }
            (Some(total), None) => {
                record = record.with_totals(total, 0);
            }
            (None, Some(processed)) => {
                record = record.with_totals(0, processed);
            }
            _ => {}
        }

        if let Some(state) = payload.custom_state.clone() {
            record = record.with_custom_state(state);
        }

        self.inventory
            .insert_task(&record)
            .with_context(|| format!("Failed to persist task {}", task_id))?;

        let payload = payload.with_task_id(task_id.clone());
        self.dispatch_task(task_id.clone(), payload)?;
        Ok(task_id)
    }

    pub fn list_active_tasks(&self) -> Result<Vec<TaskRecord>> {
        self.inventory.list_tasks(
            Some(&self.drive_id),
            Some(&[TaskStatus::Pending, TaskStatus::Running]),
        )
    }

    pub async fn ongoing_progress(&self) -> Vec<TaskProgress> {
        self.progress
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    fn dispatch_task(&self, task_id: String, payload: TaskPayload) -> Result<()> {
        let command = QueueCommand::Enqueue(QueuedTask { task_id, payload });
        self.command_tx
            .send(command)
            .context("Task dispatcher closed")?;
        Ok(())
    }

    pub async fn persist_progress(
        &self,
        task_id: &str,
        progress: f64,
        processed_bytes: Option<i64>,
        total_bytes: Option<i64>,
        custom_state: Option<Value>,
    ) -> Result<()> {
        let clamped = progress.clamp(0.0, 1.0);
        if let Some(mut entry) = self.progress.get_mut(task_id) {
            entry.update(clamped, processed_bytes, total_bytes, custom_state);
            Ok(())
        } else {
            Err(anyhow!("No progress entry for task {}", task_id))
        }
    }

    pub async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }

        self.cancel_requested.store(true, Ordering::SeqCst);

        if let Err(err) = self.command_tx.send(QueueCommand::Shutdown) {
            warn!(target: "tasks::queue", error = %err, "Task queue dispatcher already closed");
        }

        if let Some(handle) = self.dispatcher_handle.lock().await.take() {
            handle.abort();
        }

        self.cancel_running_tasks().await;
        self.task_handles.clear();
        self.progress.clear();
    }

    async fn spawn_dispatcher(self: &Arc<Self>, command_rx: UnboundedReceiver<QueueCommand>) {
        let queue = Arc::clone(self);
        let handle = tokio::spawn(async move {
            queue.run_dispatch_loop(command_rx).await;
        });
        *self.dispatcher_handle.lock().await = Some(handle);
    }

    async fn run_dispatch_loop(self: Arc<Self>, mut command_rx: UnboundedReceiver<QueueCommand>) {
        info!(
            target: "tasks::queue",
            drive = %self.drive_id,
            concurrency = self.config.max_concurrent,
            "Task queue dispatcher started"
        );

        while let Some(command) = command_rx.recv().await {
            match command {
                QueueCommand::Enqueue(task) => {
                    self.launch_task(task).await;
                }
                QueueCommand::Shutdown => {
                    debug!(
                        target: "tasks::queue",
                        drive = %self.drive_id,
                        "Task queue dispatcher shutting down"
                    );
                    break;
                }
            }
        }

        info!(
            target: "tasks::queue",
            drive = %self.drive_id,
            "Task queue dispatcher stopped"
        );
    }

    async fn launch_task(self: &Arc<Self>, task: QueuedTask) {
        let permit = match self.semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(err) => {
                error!(
                    target: "tasks::queue",
                    drive = %self.drive_id,
                    error = %err,
                    "Failed to acquire semaphore permit"
                );
                if let Err(update_err) = self.inventory.update_task(
                    &task.task_id,
                    TaskUpdate {
                        status: Some(TaskStatus::Failed),
                        error: Some(Some("Failed to schedule task".to_string())),
                        ..Default::default()
                    },
                ) {
                    warn!(
                        target: "tasks::queue",
                        drive = %self.drive_id,
                        error = %update_err,
                        "Failed to persist scheduling failure"
                    );
                }
                return;
            }
        };

        self.inflight.fetch_add(1, Ordering::SeqCst);
        let queue_for_execute = Arc::clone(self);
        let queue_for_notify = Arc::clone(self);
        let task_id = task.task_id.clone();
        let handle_task_id = task_id.clone();

        let handle = tokio::spawn(async move {
            queue_for_execute.execute_task(task).await;
            drop(permit);
            queue_for_notify.inflight.fetch_sub(1, Ordering::SeqCst);
            queue_for_notify.idle_notify.notify_waiters();
            queue_for_notify.task_handles.remove(&handle_task_id);
        });

        self.task_handles.insert(task_id, handle);
    }

    async fn execute_task(self: Arc<Self>, task: QueuedTask) {
        if let Err(err) = self.inventory.update_task(
            &task.task_id,
            TaskUpdate {
                status: Some(TaskStatus::Running),
                ..Default::default()
            },
        ) {
            error!(
                target: "tasks::queue",
                drive = %self.drive_id,
                task_id = %task.task_id,
                error = %err,
                "Failed to mark task as running"
            );
            return;
        }

        self.register_progress_entry(&task).await;

        match self.run_placeholder_task(&task).await {
            Ok(TaskRunState::Completed) => {
                if let Err(err) = self.inventory.update_task(
                    &task.task_id,
                    TaskUpdate {
                        status: Some(TaskStatus::Completed),
                        ..Default::default()
                    },
                ) {
                    warn!(
                        target: "tasks::queue",
                        drive = %self.drive_id,
                        task_id = %task.task_id,
                        error = %err,
                        "Failed to mark task as completed"
                    );
                }
            }
            Ok(TaskRunState::Cancelled) => {
                if let Err(err) = self.inventory.update_task(
                    &task.task_id,
                    TaskUpdate {
                        status: Some(TaskStatus::Cancelled),
                        ..Default::default()
                    },
                ) {
                    warn!(
                        target: "tasks::queue",
                        drive = %self.drive_id,
                        task_id = %task.task_id,
                        error = %err,
                        "Failed to mark task as cancelled"
                    );
                }
                self.clear_progress_entry(&task.task_id).await;
                return;
            }
            Err(err) => {
                error!(
                    target: "tasks::queue",
                    drive = %self.drive_id,
                    task_id = %task.task_id,
                    error = ?err,
                    "Task execution failed"
                );
                if let Err(update_err) = self.inventory.update_task(
                    &task.task_id,
                    TaskUpdate {
                        status: Some(TaskStatus::Failed),
                        error: Some(Some(err.to_string())),
                        ..Default::default()
                    },
                ) {
                    warn!(
                        target: "tasks::queue",
                        drive = %self.drive_id,
                        task_id = %task.task_id,
                        error = %update_err,
                        "Failed to persist task failure state"
                    );
                }
                self.clear_progress_entry(&task.task_id).await;
                return;
            }
        }

        self.clear_progress_entry(&task.task_id).await;
    }

    async fn run_placeholder_task(&self, task: &QueuedTask) -> Result<TaskRunState> {
        info!(
            target: "tasks::queue",
            drive = %self.drive_id,
            task_id = %task.task_id,
            kind = task.payload.kind.as_str(),
            path = %task.payload.local_path_display(),
            "Executing task"
        );

        match &task.payload.kind {
            TaskKind::Upload => {
                let mut task_executor = UploadTask::new(
                    self.inventory.clone(),
                    self.cr_client.clone(),
                    self.drive_id.as_str(),
                    &task,
                );

                task_executor.execute().await?;
            }
            TaskKind::Download => todo!(),
        }

        // for step in 0..PLACEHOLDER_STEPS {
        //     if self.cancel_requested.load(Ordering::SeqCst) {
        //         return Ok(TaskRunState::Cancelled);
        //     }
        //     sleep(Duration::from_millis(250)).await;
        //     let progress = (step + 1) as f64 / PLACEHOLDER_STEPS as f64;
        //     let direction = match task.payload.kind {
        //         TaskKind::Upload => "upload",
        //         TaskKind::Download => "download",
        //     };
        //     let state = json!({
        //         "step": step + 1,
        //         "total_steps": PLACEHOLDER_STEPS,
        //         "local_path": task.payload.local_path_display(),
        //         "kind": direction,
        //     });
        //     if let Err(err) = self
        //         .persist_progress(
        //             &task.task_id,
        //             progress,
        //             task.payload.processed_bytes,
        //             task.payload.total_bytes,
        //             Some(state),
        //         )
        //         .await
        //     {
        //         warn!(
        //             target: "tasks::queue",
        //             drive = %self.drive_id,
        //             task_id = %task.task_id,
        //             error = %err,
        //             "Failed to persist placeholder progress"
        //         );
        //     }
        // }

        Ok(TaskRunState::Completed)
    }

    async fn wait_for_idle(&self) {
        while self.inflight.load(Ordering::SeqCst) > 0 {
            self.idle_notify.notified().await;
        }
    }

    async fn register_progress_entry(&self, task: &QueuedTask) {
        self.progress.insert(
            task.task_id.clone(),
            TaskProgress::from_payload(&task.task_id, &task.payload),
        );
    }

    async fn clear_progress_entry(&self, task_id: &str) {
        self.progress.remove(task_id);
    }

    async fn resume_incomplete_tasks(self: &Arc<Self>) -> Result<()> {
        let records = self.inventory.list_tasks(
            Some(&self.drive_id),
            Some(&[TaskStatus::Pending, TaskStatus::Running]),
        )?;

        if records.is_empty() {
            return Ok(());
        }

        let mut resumed = 0usize;
        for record in records {
            if record.status == TaskStatus::Running {
                if let Err(err) = self.inventory.update_task(
                    &record.id,
                    TaskUpdate {
                        status: Some(TaskStatus::Pending),
                        ..Default::default()
                    },
                ) {
                    warn!(
                        target: "tasks::queue",
                        drive = %self.drive_id,
                        task_id = %record.id,
                        error = ?err,
                        "Failed to reset task status during resume"
                    );
                    continue;
                }
            }

            let payload = match Self::payload_from_record(&record) {
                Ok(payload) => payload,
                Err(err) => {
                    warn!(
                        target: "tasks::queue",
                        drive = %self.drive_id,
                        task_id = %record.id,
                        error = %err,
                        "Failed to build payload for resumed task"
                    );
                    continue;
                }
            };

            if let Err(err) = self.dispatch_task(record.id.clone(), payload) {
                warn!(
                    target: "tasks::queue",
                    drive = %self.drive_id,
                    task_id = %record.id,
                    error = ?err,
                    "Failed to dispatch resumed task"
                );
                continue;
            }

            resumed += 1;
        }

        if resumed > 0 {
            info!(
                target: "tasks::queue",
                drive = %self.drive_id,
                count = resumed,
                "Resumed pending tasks from inventory"
            );
        }

        Ok(())
    }

    async fn cancel_running_tasks(&self) {
        let running: Vec<String> = self
            .progress
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        for task_id in running {
            if let Some((_, handle)) = self.task_handles.remove(&task_id) {
                handle.abort();
            }

            self.progress.remove(&task_id);
        }
    }

    fn payload_from_record(record: &TaskRecord) -> Result<TaskPayload> {
        let kind = TaskKind::from_str(&record.task_type)
            .ok_or_else(|| anyhow!("Unknown task type {}", record.task_type))?;

        let mut payload = TaskPayload::new(kind, PathBuf::from(&record.local_path))
            .with_priority(record.priority)
            .with_task_id(record.id.clone());

        if let Some(uri) = &record.remote_uri {
            payload = payload.with_remote_uri(uri.clone());
        }

        let total_bytes = record.total_bytes;
        let processed_bytes = record.processed_bytes;
        if total_bytes != 0 || processed_bytes != 0 {
            payload = payload.with_totals(processed_bytes, total_bytes);
        }

        if let Some(state) = &record.custom_state {
            payload = payload.with_custom_state(state.clone());
        }

        Ok(payload)
    }
}

enum TaskRunState {
    Completed,
    Cancelled,
}

enum QueueCommand {
    Enqueue(QueuedTask),
    Shutdown,
}

pub struct QueuedTask {
    pub task_id: String,
    pub payload: TaskPayload,
}

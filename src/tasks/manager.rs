use super::models::{
    Task, TaskCallback, TaskExecutor, TaskFilter, TaskId, TaskInfo, TaskPriority, TaskProperties,
    TaskStatus,
};
use super::queue::TaskQueue;
use super::worker::{TaskResult, WorkerPool};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing;

/// Configuration for TaskManager
#[derive(Debug, Clone)]
pub struct TaskManagerConfig {
    /// Maximum number of concurrent workers
    pub max_workers: usize,
    /// Maximum number of completed tasks to keep in history
    pub completed_buffer_size: usize,
}

impl Default for TaskManagerConfig {
    fn default() -> Self {
        Self {
            max_workers: 4,
            completed_buffer_size: 100,
        }
    }
}

/// Main task management system
pub struct TaskManager {
    config: Arc<RwLock<TaskManagerConfig>>,
    /// Pending tasks in priority queue
    pending_queue: Arc<Mutex<TaskQueue>>,
    /// Currently running task IDs (for tracking)
    running_task_ids: Arc<Mutex<HashMap<TaskId, TaskInfo>>>,
    /// Completed tasks buffer (FIFO)
    completed_tasks: Arc<Mutex<VecDeque<TaskInfo>>>,
    /// Worker pool
    worker_pool: Arc<Mutex<WorkerPool>>,
    /// Task processing handle
    processor_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl TaskManager {
    /// Create a new TaskManager with the given configuration
    pub fn new(config: TaskManagerConfig) -> Arc<Self> {
        let worker_pool = WorkerPool::new(config.max_workers);

        let manager = Arc::new(Self {
            config: Arc::new(RwLock::new(config)),
            pending_queue: Arc::new(Mutex::new(TaskQueue::new())),
            running_task_ids: Arc::new(Mutex::new(HashMap::new())),
            completed_tasks: Arc::new(Mutex::new(VecDeque::new())),
            worker_pool: Arc::new(Mutex::new(worker_pool)),
            processor_handle: Arc::new(Mutex::new(None)),
        });

        // Start the task processor
        manager.start_processor();

        tracing::info!(target: "tasks::manager", "TaskManager initialized");
        manager
    }

    /// Start the background task processor
    fn start_processor(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        let processor_handle = Arc::clone(&self.processor_handle);

        let handle = tokio::spawn(async move {
            tracing::info!(target: "tasks::manager", "Task processor started");

            loop {
                // Try to schedule pending tasks
                manager.schedule_pending_tasks().await;

                // Process task results
                if let Some(result) = manager.worker_pool.lock().await.next_result().await {
                    manager.handle_task_result(result).await;
                }

                // Small delay to prevent busy-waiting
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            }
        });

        // Store the handle in a separate spawned task to avoid blocking
        tokio::spawn(async move {
            *processor_handle.lock().await = Some(handle);
        });
    }

    /// Handle a completed task result
    async fn handle_task_result(&self, result: TaskResult) {
        let mut running = self.running_task_ids.lock().await;

        if let Some(mut task_info) = running.remove(&result.task_id) {
            task_info.status = result.status.clone();
            task_info.completed_at = Some(std::time::SystemTime::now());

            // Store error and result data from execution result
            task_info.error = result.execution_result.error.clone();
            task_info.result_data = result.execution_result.result_data.clone();

            // Add to completed tasks buffer
            let mut completed = self.completed_tasks.lock().await;
            completed.push_back(task_info);

            // Maintain buffer size limit
            let config = self.config.read().await;
            while completed.len() > config.completed_buffer_size {
                completed.pop_front();
            }

            tracing::debug!(
                target: "tasks::manager",
                task_id = %result.task_id,
                status = ?result.status,
                has_result_data = result.execution_result.result_data.is_some(),
                "Task result processed"
            );
        }
    }

    /// Schedule pending tasks if workers are available
    async fn schedule_pending_tasks(&self) {
        let worker_pool = self.worker_pool.lock().await;

        while worker_pool.has_capacity().await {
            let mut pending = self.pending_queue.lock().await;

            if let Some(mut task) = pending.pop() {
                drop(pending); // Release lock early

                task.status = TaskStatus::Running;
                task.started_at = Some(std::time::SystemTime::now());

                let task_id = task.id.clone();
                let task_priority = task.priority;

                tracing::info!(
                    target: "tasks::manager",
                    task_id = %task_id,
                    priority = ?task_priority,
                    "Scheduling task for execution"
                );

                // Create task info before moving the task
                let task_info = task.to_info().await;

                // Add to running tasks tracking
                self.running_task_ids
                    .lock()
                    .await
                    .insert(task_id.clone(), task_info);

                // Execute the task (task is moved here)
                if let Err(e) = worker_pool.execute(task).await {
                    tracing::error!(
                        target: "tasks::manager",
                        task_id = %task_id,
                        error = %e,
                        "Failed to execute task"
                    );

                    // Remove from running tasks tracking
                    self.running_task_ids.lock().await.remove(&task_id);
                }
            } else {
                break;
            }
        }
    }

    /// Submit a new task with custom executor and callback
    pub async fn submit_task(
        &self,
        task_id: TaskId,
        priority: TaskPriority,
        properties: TaskProperties,
        executor: TaskExecutor,
        callback: Option<TaskCallback>,
    ) -> Result<TaskId, String> {
        let mut task = Task::new(task_id.clone(), priority, properties, executor);

        if let Some(cb) = callback {
            task = task.with_callback(cb);
        }

        tracing::info!(
            target: "tasks::manager",
            task_id = %task_id,
            priority = ?priority,
            "Submitting new task"
        );

        // Add to pending queue
        self.pending_queue.lock().await.push(task);

        Ok(task_id)
    }

    /// Submit a task with a simple default executor (for API use)
    pub async fn submit_simple_task(
        &self,
        task_id: TaskId,
        priority: TaskPriority,
        properties: TaskProperties,
    ) -> Result<TaskId, String> {
        use super::models::TaskExecutionResult;

        // Create a simple default executor
        let executor: TaskExecutor = Arc::new(|props| {
            Box::pin(async move {
                // Simple task execution - just simulates work
                tracing::info!(target: "tasks::executor", "Executing simple task");

                for i in 0..10 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    let mut p = props.write().await;
                    p.progress = (i + 1) as f32 / 10.0;
                }

                TaskExecutionResult::ok()
            })
        });

        println!("Submitting task: {:?}", task_id);
        self.submit_task(task_id, priority, properties, executor, None)
            .await
    }

    /// Cancel a task (pending or running)
    pub async fn cancel_task(&self, task_id: &TaskId) -> Result<(), String> {
        tracing::info!(target: "tasks::manager", task_id = %task_id, "Cancelling task");

        // Try to remove from pending queue first
        let mut pending = self.pending_queue.lock().await;
        if let Some(mut task) = pending.remove(task_id) {
            task.status = TaskStatus::Cancelled;
            task.completed_at = Some(std::time::SystemTime::now());

            // Add to completed tasks
            let task_info = task.to_info().await;
            self.completed_tasks.lock().await.push_back(task_info);

            tracing::info!(target: "tasks::manager", task_id = %task_id, "Cancelled pending task");
            return Ok(());
        }
        drop(pending);

        // Try to cancel running task
        if self.worker_pool.lock().await.cancel(task_id).await {
            if let Some(mut task_info) = self.running_task_ids.lock().await.remove(task_id) {
                task_info.status = TaskStatus::Cancelled;
                task_info.completed_at = Some(std::time::SystemTime::now());

                self.completed_tasks.lock().await.push_back(task_info);
            }

            tracing::info!(target: "tasks::manager", task_id = %task_id, "Cancelled running task");
            Ok(())
        } else {
            Err(format!("Task not found: {}", task_id))
        }
    }

    /// Stop all tasks (pending and running)
    pub async fn stop_all_tasks(&self) -> usize {
        tracing::info!(target: "tasks::manager", "Stopping all tasks");

        let mut count = 0;

        // Cancel all pending tasks
        let mut pending = self.pending_queue.lock().await;
        let pending_tasks = pending.clear();
        count += pending_tasks.len();

        for mut task in pending_tasks {
            task.status = TaskStatus::Cancelled;
            task.completed_at = Some(std::time::SystemTime::now());
            let task_info = task.to_info().await;
            self.completed_tasks.lock().await.push_back(task_info);
        }
        drop(pending);

        // Cancel all running tasks
        let running_ids = self
            .running_task_ids
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        count += running_ids.len();

        for task_id in running_ids {
            let _ = self.cancel_task(&task_id).await;
        }

        tracing::info!(target: "tasks::manager", cancelled_count = count, "All tasks stopped");
        count
    }

    /// Get all tasks matching a filter
    pub async fn get_tasks(&self, filter: Option<TaskFilter>) -> Vec<TaskInfo> {
        let mut tasks = Vec::new();

        // Collect pending tasks
        let pending = self.pending_queue.lock().await;
        for task in pending.get_all() {
            let info = task.to_info().await;
            if let Some(ref f) = filter {
                if f.matches(&info) {
                    tasks.push(info);
                }
            } else {
                tasks.push(info);
            }
        }
        drop(pending);

        // Collect running tasks
        let running = self.running_task_ids.lock().await;
        for task_info in running.values() {
            if let Some(ref f) = filter {
                if f.matches(task_info) {
                    tasks.push(task_info.clone());
                }
            } else {
                tasks.push(task_info.clone());
            }
        }
        drop(running);

        // Collect completed tasks
        let completed = self.completed_tasks.lock().await;
        for info in completed.iter() {
            if let Some(ref f) = filter {
                if f.matches(info) {
                    tasks.push(info.clone());
                }
            } else {
                tasks.push(info.clone());
            }
        }

        tasks
    }

    /// Get a specific task by ID
    pub async fn get_task(&self, task_id: &TaskId) -> Option<TaskInfo> {
        // Check pending
        let pending = self.pending_queue.lock().await;
        for task in pending.get_all() {
            if &task.id == task_id {
                return Some(task.to_info().await);
            }
        }
        drop(pending);

        // Check running
        if let Some(task_info) = self.running_task_ids.lock().await.get(task_id) {
            return Some(task_info.clone());
        }

        // Check completed
        let completed = self.completed_tasks.lock().await;
        for info in completed.iter() {
            if &info.id == task_id {
                return Some(info.clone());
            }
        }

        None
    }

    /// Get task statistics
    pub async fn get_statistics(&self) -> TaskStatistics {
        let pending_count = self.pending_queue.lock().await.len();
        let running_count = self.running_task_ids.lock().await.len();
        let completed = self.completed_tasks.lock().await;

        let completed_count = completed
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        let failed_count = completed
            .iter()
            .filter(|t| t.status == TaskStatus::Failed)
            .count();
        let cancelled_count = completed
            .iter()
            .filter(|t| t.status == TaskStatus::Cancelled)
            .count();

        let config = self.config.read().await;

        TaskStatistics {
            pending_count,
            running_count,
            completed_count,
            failed_count,
            cancelled_count,
            max_workers: config.max_workers,
            completed_buffer_size: config.completed_buffer_size,
        }
    }

    /// Update max workers setting
    pub async fn set_max_workers(&self, max_workers: usize) {
        tracing::info!(target: "tasks::manager", max_workers, "Updating max workers");

        self.config.write().await.max_workers = max_workers;
        self.worker_pool.lock().await.set_max_workers(max_workers);
    }

    /// Update completed buffer size
    pub async fn set_completed_buffer_size(&self, buffer_size: usize) {
        tracing::info!(target: "tasks::manager", buffer_size, "Updating completed buffer size");

        self.config.write().await.completed_buffer_size = buffer_size;

        // Trim existing buffer if needed
        let mut completed = self.completed_tasks.lock().await;
        while completed.len() > buffer_size {
            completed.pop_front();
        }
    }

    /// Clear completed tasks buffer
    pub async fn clear_completed_tasks(&self) -> usize {
        let mut completed = self.completed_tasks.lock().await;
        let count = completed.len();
        completed.clear();
        tracing::info!(target: "tasks::manager", cleared_count = count, "Cleared completed tasks");
        count
    }

    /// Shutdown the task manager
    pub async fn shutdown(&self) {
        tracing::info!(target: "tasks::manager", "Shutting down TaskManager");

        // Stop all tasks
        self.stop_all_tasks().await;

        // Shutdown worker pool
        self.worker_pool.lock().await.shutdown().await;

        // Stop processor
        if let Some(handle) = self.processor_handle.lock().await.take() {
            handle.abort();
        }

        tracing::info!(target: "tasks::manager", "TaskManager shutdown complete");
    }
}

/// Task statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskStatistics {
    pub pending_count: usize,
    pub running_count: usize,
    pub completed_count: usize,
    pub failed_count: usize,
    pub cancelled_count: usize,
    pub max_workers: usize,
    pub completed_buffer_size: usize,
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

pub type TaskId = String;

/// Result of task execution with optional custom result data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionResult {
    /// Whether the task succeeded
    pub success: bool,
    /// Error message if task failed
    pub error: Option<String>,
    /// Custom result data (JSON value for flexibility)
    pub result_data: Option<serde_json::Value>,
}

impl TaskExecutionResult {
    /// Create a successful result with optional data
    pub fn success(result_data: Option<serde_json::Value>) -> Self {
        Self {
            success: true,
            error: None,
            result_data,
        }
    }

    /// Create a failed result with error message
    pub fn failure(error: String) -> Self {
        Self {
            success: false,
            error: Some(error),
            result_data: None,
        }
    }

    /// Create a successful result without data (for backwards compatibility)
    pub fn ok() -> Self {
        Self::success(None)
    }
}

/// Task priority levels (higher value = higher priority)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

impl Default for TaskPriority {
    fn default() -> Self {
        TaskPriority::Normal
    }
}

/// Task status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Task is waiting in queue
    Pending,
    /// Task is currently being executed
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed with error
    Failed,
    /// Task was cancelled
    Cancelled,
}

/// Task type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Upload,
    Download,
    Sync,
    Delete,
    Copy,
    Move,
    Custom(String),
}

/// Task properties - stores task metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProperties {
    /// Task type
    pub task_type: TaskType,
    /// Target file path (if applicable)
    pub target_path: Option<PathBuf>,
    /// Source file path (for copy/move operations)
    pub source_path: Option<PathBuf>,
    /// Drive ID associated with this task
    pub drive_id: Option<String>,
    /// Current progress (0.0 - 1.0)
    pub progress: f32,
    /// Total size in bytes
    pub total_size: Option<u64>,
    /// Processed size in bytes
    pub processed_size: Option<u64>,
    /// Additional metadata
    #[serde(flatten)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl TaskProperties {
    pub fn new(task_type: TaskType) -> Self {
        Self {
            task_type,
            target_path: None,
            source_path: None,
            drive_id: None,
            progress: 0.0,
            total_size: None,
            processed_size: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_target_path(mut self, path: PathBuf) -> Self {
        self.target_path = Some(path);
        self
    }

    pub fn with_source_path(mut self, path: PathBuf) -> Self {
        self.source_path = Some(path);
        self
    }

    pub fn with_drive_id(mut self, drive_id: String) -> Self {
        self.drive_id = Some(drive_id);
        self
    }

    pub fn with_metadata(mut self, key: String, value: serde_json::Value) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

/// Task execution function type - now returns TaskExecutionResult for custom results
pub type TaskExecutor = Arc<
    dyn Fn(
            Arc<RwLock<TaskProperties>>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = TaskExecutionResult> + Send>>
        + Send
        + Sync,
>;

/// Task completion callback type - now receives TaskExecutionResult
pub type TaskCallback = Arc<
    dyn Fn(
            TaskId,
            TaskStatus,
            TaskExecutionResult,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Represents a task in the queue
pub struct Task {
    /// Unique task ID
    pub id: TaskId,
    /// Task priority
    pub priority: TaskPriority,
    /// Task properties (thread-safe for progress updates)
    pub properties: Arc<RwLock<TaskProperties>>,
    /// Task status
    pub status: TaskStatus,
    /// Creation time
    pub created_at: SystemTime,
    /// Start time (when execution began)
    pub started_at: Option<SystemTime>,
    /// Completion time
    pub completed_at: Option<SystemTime>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Custom execution result data
    pub result_data: Option<serde_json::Value>,
    /// Task executor function
    pub(crate) executor: TaskExecutor,
    /// Optional completion callback
    pub(crate) callback: Option<TaskCallback>,
}

impl Task {
    pub fn new(
        id: TaskId,
        priority: TaskPriority,
        properties: TaskProperties,
        executor: TaskExecutor,
    ) -> Self {
        Self {
            id,
            priority,
            properties: Arc::new(RwLock::new(properties)),
            status: TaskStatus::Pending,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error: None,
            result_data: None,
            executor,
            callback: None,
        }
    }

    pub fn with_callback(mut self, callback: TaskCallback) -> Self {
        self.callback = Some(callback);
        self
    }

    /// Get task info for serialization (without executor/callback)
    pub async fn to_info(&self) -> TaskInfo {
        let properties = self.properties.read().await.clone();
        TaskInfo {
            id: self.id.clone(),
            priority: self.priority,
            properties,
            status: self.status.clone(),
            created_at: self.created_at,
            started_at: self.started_at,
            completed_at: self.completed_at,
            error: self.error.clone(),
            result_data: self.result_data.clone(),
        }
    }
}

/// Serializable task information
#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub id: TaskId,
    pub priority: TaskPriority,
    pub properties: TaskProperties,
    pub status: TaskStatus,
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub error: Option<String>,
    pub result_data: Option<serde_json::Value>,
}

/// Filter for searching tasks
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub task_type: Option<TaskType>,
    pub target_path: Option<PathBuf>,
    pub drive_id: Option<String>,
    pub status: Option<TaskStatus>,
}

impl TaskFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_type(mut self, task_type: TaskType) -> Self {
        self.task_type = Some(task_type);
        self
    }

    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.target_path = Some(path);
        self
    }

    pub fn with_drive_id(mut self, drive_id: String) -> Self {
        self.drive_id = Some(drive_id);
        self
    }

    pub fn with_status(mut self, status: TaskStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Check if task matches this filter
    pub fn matches(&self, task_info: &TaskInfo) -> bool {
        if let Some(ref filter_type) = self.task_type {
            if &task_info.properties.task_type != filter_type {
                return false;
            }
        }

        if let Some(ref filter_path) = self.target_path {
            if task_info.properties.target_path.as_ref() != Some(filter_path) {
                return false;
            }
        }

        if let Some(ref filter_drive_id) = self.drive_id {
            if task_info.properties.drive_id.as_ref() != Some(filter_drive_id) {
                return false;
            }
        }

        if let Some(ref filter_status) = self.status {
            if &task_info.status != filter_status {
                return false;
            }
        }

        true
    }
}

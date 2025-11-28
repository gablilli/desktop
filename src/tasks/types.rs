use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskKind {
    Upload,
    Download,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskKind::Upload => "upload",
            TaskKind::Download => "download",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "upload" => Some(TaskKind::Upload),
            "download" => Some(TaskKind::Download),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskPayload {
    pub task_id: Option<String>,
    pub kind: TaskKind,
    pub local_path: PathBuf,
    pub remote_uri: Option<String>,
    pub priority: i32,
    pub total_bytes: Option<i64>,
    pub processed_bytes: Option<i64>,
    pub custom_state: Option<Value>,
}

impl TaskPayload {
    pub fn new(kind: TaskKind, local_path: impl Into<PathBuf>) -> Self {
        Self {
            task_id: None,
            kind,
            local_path: local_path.into(),
            remote_uri: None,
            priority: 0,
            total_bytes: None,
            processed_bytes: None,
            custom_state: None,
        }
    }

    pub fn upload(local_path: impl Into<PathBuf>) -> Self {
        Self::new(TaskKind::Upload, local_path)
    }

    pub fn download(local_path: impl Into<PathBuf>) -> Self {
        Self::new(TaskKind::Download, local_path)
    }

    pub fn with_task_id(mut self, id: impl Into<String>) -> Self {
        self.task_id = Some(id.into());
        self
    }

    pub fn with_remote_uri(mut self, remote_uri: impl Into<String>) -> Self {
        self.remote_uri = Some(remote_uri.into());
        self
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_totals(mut self, processed: i64, total: i64) -> Self {
        self.processed_bytes = Some(processed);
        self.total_bytes = Some(total);
        self
    }

    pub fn with_custom_state(mut self, state: Value) -> Self {
        self.custom_state = Some(state);
        self
    }

    pub fn local_path_display(&self) -> String {
        self.local_path.as_path().to_string_lossy().into_owned()
    }

    pub fn remote_uri(&self) -> Option<&str> {
        self.remote_uri.as_deref()
    }

    pub fn custom_state(&self) -> Option<&Value> {
        self.custom_state.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct TaskProgress {
    pub task_id: String,
    pub kind: TaskKind,
    pub local_path: String,
    pub remote_uri: Option<String>,
    pub progress: f64,
    pub processed_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub custom_state: Option<Value>,
}

impl TaskProgress {
    pub fn from_payload(task_id: impl Into<String>, payload: &TaskPayload) -> Self {
        Self {
            task_id: task_id.into(),
            kind: payload.kind,
            local_path: payload.local_path_display(),
            remote_uri: payload.remote_uri.clone(),
            progress: 0.0,
            processed_bytes: payload.processed_bytes,
            total_bytes: payload.total_bytes,
            custom_state: payload.custom_state.clone(),
        }
    }

    pub fn update(
        &mut self,
        progress: f64,
        processed_bytes: Option<i64>,
        total_bytes: Option<i64>,
        custom_state: Option<Value>,
    ) {
        self.progress = progress;
        if let Some(bytes) = processed_bytes {
            self.processed_bytes = Some(bytes);
        }
        if let Some(total) = total_bytes {
            self.total_bytes = Some(total);
        }
        if let Some(state) = custom_state {
            self.custom_state = Some(state);
        }
    }
}

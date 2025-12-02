use super::{FileMetadata, MetadataEntry, NewTaskRecord, TaskRecord, TaskStatus, TaskUpdate};
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use diesel::OptionalExtension;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool, PooledConnection};
use diesel::sql_types::Text;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use dirs::home_dir;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use super::schema::file_metadata::{self, dsl as file_metadata_dsl};
use super::schema::task_queue::{self, dsl as task_queue_dsl};
use super::schema::upload_sessions::{self, dsl as upload_sessions_dsl};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations/inventory");

/// SQLite-backed inventory database that relies on Diesel for schema management.
pub struct InventoryDb {
    pool: Arc<Pool<ConnectionManager<SqliteConnection>>>,
}

impl InventoryDb {
    /// Create or open the inventory database at the default location (~/.cloudreve/meta.db)
    pub fn new() -> Result<Self> {
        let db_path = Self::get_db_path()?;
        Self::with_path(db_path)
    }

    /// Create or open the inventory database at a specific path.
    /// The schema is automatically migrated to the latest version on startup.
    pub fn with_path(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create inventory db parent dir {}",
                    parent.display()
                )
            })?;
        }

        let database_url = path
            .to_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Invalid inventory database path"))?;

        run_migrations(&database_url)?;

        let manager = ConnectionManager::<SqliteConnection>::new(database_url);
        let pool = Pool::builder()
            .max_size(1)
            .build(manager)
            .context("Failed to build inventory database connection pool")?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    fn get_db_path() -> Result<PathBuf> {
        let home = home_dir().ok_or_else(|| anyhow!("Unable to determine home directory"))?;
        Ok(home.join(".cloudreve").join("meta.db"))
    }

    fn connection(&self) -> Result<PooledConnection<ConnectionManager<SqliteConnection>>> {
        self.pool
            .get()
            .context("Failed to get connection from inventory pool")
    }

    pub fn batch_insert(&self, entries: &[MetadataEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let rows: Vec<NewFileMetadata> = entries
            .iter()
            .map(NewFileMetadata::try_from)
            .collect::<Result<_>>()?;

        let mut conn = self.connection()?;
        diesel::insert_into(file_metadata::table)
            .values(&rows)
            .execute(&mut conn)
            .context("Failed to batch insert inventory metadata")?;
        Ok(())
    }

    pub fn nuke_drive(&self, drive: &str) -> Result<()> {
        let mut conn = self.connection()?;
        diesel::delete(
            file_metadata_dsl::file_metadata.filter(file_metadata_dsl::drive_id.eq(drive)),
        )
        .execute(&mut conn)
        .context("Failed to delete inventory rows for drive")?;
        Ok(())
    }

    /// Insert a new file metadata entry
    pub fn insert(&self, entry: &MetadataEntry) -> Result<usize> {
        let mut conn = self.connection()?;
        let new_entry = NewFileMetadata::try_from(entry)?;
        diesel::insert_into(file_metadata::table)
            .values(&new_entry)
            .execute(&mut conn)
            .context("Failed to insert inventory metadata")
    }

    /// Update an existing file metadata entry by local path
    pub fn update(&self, entry: &MetadataEntry) -> Result<bool> {
        let mut conn = self.connection()?;
        let changeset = FileMetadataChangeset::from_entry(entry)?;
        let rows_affected = diesel::update(
            file_metadata_dsl::file_metadata
                .filter(file_metadata_dsl::local_path.eq(&entry.local_path)),
        )
        .set(changeset)
        .execute(&mut conn)
        .context("Failed to update inventory metadata")?;
        Ok(rows_affected > 0)
    }

    /// Insert or update a file metadata entry (upsert based on local_path)
    pub fn upsert(&self, entry: &MetadataEntry) -> Result<usize> {
        let mut conn = self.connection()?;
        let mut insert_data = NewFileMetadata::try_from(entry)?;
        let update_data = FileMetadataChangeset::from_entry(entry)?;

        diesel::insert_into(file_metadata::table)
            .values(&insert_data)
            .on_conflict(file_metadata::local_path)
            .do_update()
            .set(update_data)
            .execute(&mut conn)
            .context("Failed to upsert inventory metadata")
    }

    /// Query file metadata by local path
    pub fn query_by_path(&self, path: &str) -> Result<Option<FileMetadata>> {
        let mut conn = self.connection()?;
        let row = file_metadata_dsl::file_metadata
            .filter(file_metadata_dsl::local_path.eq(path))
            .first::<FileMetadataRow>(&mut conn)
            .optional()
            .context("Failed to query inventory metadata by path")?;

        row.map(FileMetadata::try_from).transpose()
    }

    /// Batch delete file metadata by local path
    pub fn batch_delete_by_path(&self, paths: Vec<&str>) -> Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }

        let mut conn = self.connection()?;
        let affected = (&mut *conn)
            .transaction::<i64, diesel::result::Error, _>(|tx_conn| {
                let mut total: i64 = 0;
                for path in &paths {
                    total += diesel::delete(
                        file_metadata_dsl::file_metadata
                            .filter(file_metadata_dsl::local_path.eq(path)),
                    )
                    .execute(tx_conn)? as i64;

                    let prefix = format!("{}/%", path);
                    total += diesel::delete(
                        file_metadata_dsl::file_metadata
                            .filter(file_metadata_dsl::local_path.like(&prefix)),
                    )
                    .execute(tx_conn)? as i64;
                }
                Ok(total)
            })
            .context("Failed to batch delete inventory metadata")?;

        Ok(affected > 0)
    }

    /// Get total count of entries in the database
    pub fn count(&self) -> Result<i64> {
        let mut conn = self.connection()?;
        file_metadata_dsl::file_metadata
            .count()
            .get_result(&mut conn)
            .context("Failed to count inventory metadata")
    }

    /// Clear all entries from the database
    pub fn clear(&self) -> Result<()> {
        let mut conn = self.connection()?;
        diesel::delete(file_metadata::table)
            .execute(&mut conn)
            .context("Failed to clear inventory metadata")?;
        Ok(())
    }

    /// Insert a task queue record if no pending/running task with the same type and path exists.
    /// Returns `true` if the task was inserted, `false` if a duplicate was found.
    pub fn insert_task_if_not_exist(&self, task: &NewTaskRecord) -> Result<bool> {
        let mut conn = self.connection()?;

        // Check if a pending or running task with the same type and path already exists
        let active_statuses = vec![
            TaskStatus::Pending.as_str().to_string(),
            TaskStatus::Running.as_str().to_string(),
        ];

        let existing: Option<String> = task_queue_dsl::task_queue
            .filter(task_queue_dsl::drive_id.eq(&task.drive_id))
            .filter(task_queue_dsl::task_type.eq(&task.task_type))
            .filter(task_queue_dsl::local_path.eq(&task.local_path))
            .filter(task_queue_dsl::status.eq_any(&active_statuses))
            .select(task_queue_dsl::id)
            .first(&mut conn)
            .optional()
            .context("Failed to check for existing task")?;

        if existing.is_some() {
            return Ok(false);
        }

        let row = NewTaskRow::try_from(task)?;
        diesel::insert_into(task_queue::table)
            .values(&row)
            .execute(&mut conn)
            .context("Failed to insert task queue record")?;
        Ok(true)
    }

    /// Update task queue record
    pub fn update_task(&self, task_id: &str, update: TaskUpdate) -> Result<()> {
        if update.is_empty() {
            return Ok(());
        }

        let mut conn = self.connection()?;
        let changeset = TaskChangeset::try_from(update)?;
        diesel::update(task_queue_dsl::task_queue.filter(task_queue_dsl::id.eq(task_id)))
            .set(changeset)
            .execute(&mut conn)?;
        Ok(())
    }

    /// List task queue records with optional filters
    pub fn list_tasks(
        &self,
        drive_id: Option<&str>,
        statuses: Option<&[TaskStatus]>,
    ) -> Result<Vec<TaskRecord>> {
        let mut conn = self.connection()?;
        let mut query = task_queue_dsl::task_queue.into_boxed();

        if let Some(drive) = drive_id {
            query = query.filter(task_queue_dsl::drive_id.eq(drive));
        }

        if let Some(status_filter) = statuses {
            let values: Vec<String> = status_filter
                .iter()
                .map(|status| status.as_str().to_string())
                .collect();
            query = query.filter(task_queue_dsl::status.eq_any(values));
        }

        let rows = query
            .order(task_queue_dsl::created_at.asc())
            .load::<TaskRow>(&mut conn)
            .context("Failed to query task queue records")?;

        rows.into_iter()
            .map(TaskRecord::try_from)
            .collect::<Result<Vec<_>>>()
    }

    /// Delete a completed/failed task entry
    pub fn delete_task(&self, task_id: &str) -> Result<()> {
        let mut conn = self.connection()?;
        diesel::delete(task_queue_dsl::task_queue.filter(task_queue_dsl::id.eq(task_id)))
            .execute(&mut conn)
            .context("Failed to delete task queue record")?;
        Ok(())
    }

    /// Cancel all pending/running tasks matching a path or its descendants.
    /// Returns the list of task IDs that were cancelled.
    pub fn cancel_tasks_by_path(&self, drive_id: &str, path: &str) -> Result<Vec<String>> {
        let mut conn = self.connection()?;

        // Find tasks that match the exact path or are descendants (path starts with "path/")
        let prefix = format!("{}{}", path, std::path::MAIN_SEPARATOR);
        let active_statuses = vec![
            TaskStatus::Pending.as_str().to_string(),
            TaskStatus::Running.as_str().to_string(),
        ];

        let matching_tasks: Vec<TaskRow> = task_queue_dsl::task_queue
            .filter(task_queue_dsl::drive_id.eq(drive_id))
            .filter(task_queue_dsl::status.eq_any(&active_statuses))
            .filter(
                task_queue_dsl::local_path
                    .eq(path)
                    .or(task_queue_dsl::local_path.like(format!("{}%", prefix))),
            )
            .load(&mut conn)
            .context("Failed to query tasks by path")?;

        let task_ids: Vec<String> = matching_tasks.iter().map(|t| t.id.clone()).collect();

        if !task_ids.is_empty() {
            let cancelled_status = TaskStatus::Cancelled.as_str().to_string();
            let now = chrono::Utc::now().timestamp();

            diesel::update(task_queue_dsl::task_queue.filter(task_queue_dsl::id.eq_any(&task_ids)))
                .set((
                    task_queue_dsl::status.eq(&cancelled_status),
                    task_queue_dsl::updated_at.eq(now),
                ))
                .execute(&mut conn)
                .context("Failed to cancel tasks by path")?;
        }

        Ok(task_ids)
    }

    /// Get task status by task ID
    pub fn get_task_status(&self, task_id: &str) -> Result<Option<TaskStatus>> {
        let mut conn = self.connection()?;
        let row: Option<String> = task_queue_dsl::task_queue
            .filter(task_queue_dsl::id.eq(task_id))
            .select(task_queue_dsl::status)
            .first(&mut conn)
            .optional()
            .context("Failed to query task status")?;

        match row {
            Some(status_str) => Ok(TaskStatus::from_str(&status_str)),
            None => Ok(None),
        }
    }

    // =========================================================================
    // Upload Session Methods
    // =========================================================================

    /// Insert a new upload session
    pub fn insert_upload_session(&self, session: &crate::uploader::UploadSession) -> Result<()> {
        let mut conn = self.connection()?;
        let row = UploadSessionRow::from_session(session)?;
        diesel::insert_into(upload_sessions::table)
            .values(&row)
            .execute(&mut conn)
            .context("Failed to insert upload session")?;
        Ok(())
    }

    /// Get upload session by task ID
    pub fn get_upload_session(
        &self,
        task_id: &str,
    ) -> Result<Option<crate::uploader::UploadSession>> {
        let mut conn = self.connection()?;
        let row = upload_sessions_dsl::upload_sessions
            .filter(upload_sessions_dsl::task_id.eq(task_id))
            .first::<UploadSessionQueryRow>(&mut conn)
            .optional()
            .context("Failed to query upload session")?;

        row.map(crate::uploader::UploadSession::try_from)
            .transpose()
    }

    /// Get upload session by session ID
    pub fn get_upload_session_by_id(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::uploader::UploadSession>> {
        let mut conn = self.connection()?;
        let row = upload_sessions_dsl::upload_sessions
            .filter(upload_sessions_dsl::id.eq(session_id))
            .first::<UploadSessionQueryRow>(&mut conn)
            .optional()
            .context("Failed to query upload session by ID")?;

        row.map(crate::uploader::UploadSession::try_from)
            .transpose()
    }

    /// Update upload session chunk progress
    pub fn update_upload_session_progress(
        &self,
        session_id: &str,
        chunk_progress: &[crate::uploader::ChunkProgress],
    ) -> Result<()> {
        let mut conn = self.connection()?;
        let progress_json =
            serde_json::to_string(chunk_progress).context("Failed to serialize chunk progress")?;
        let now = Utc::now().timestamp();

        diesel::update(
            upload_sessions_dsl::upload_sessions.filter(upload_sessions_dsl::id.eq(session_id)),
        )
        .set((
            upload_sessions_dsl::chunk_progress.eq(&progress_json),
            upload_sessions_dsl::updated_at.eq(now),
        ))
        .execute(&mut conn)
        .context("Failed to update upload session progress")?;

        Ok(())
    }

    /// Delete upload session
    pub fn delete_upload_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.connection()?;
        diesel::delete(
            upload_sessions_dsl::upload_sessions.filter(upload_sessions_dsl::id.eq(session_id)),
        )
        .execute(&mut conn)
        .context("Failed to delete upload session")?;
        Ok(())
    }

    /// Delete all upload sessions for a task
    pub fn delete_upload_sessions_by_task(&self, task_id: &str) -> Result<()> {
        let mut conn = self.connection()?;
        diesel::delete(
            upload_sessions_dsl::upload_sessions
                .filter(upload_sessions_dsl::task_id.eq(task_id)),
        )
        .execute(&mut conn)
        .context("Failed to delete upload sessions for task")?;
        Ok(())
    }

    /// Delete expired upload sessions
    pub fn delete_expired_upload_sessions(&self) -> Result<usize> {
        let mut conn = self.connection()?;
        let now = Utc::now().timestamp();
        let deleted = diesel::delete(
            upload_sessions_dsl::upload_sessions
                .filter(upload_sessions_dsl::expires_at.lt(now)),
        )
        .execute(&mut conn)
        .context("Failed to delete expired upload sessions")?;
        Ok(deleted)
    }

    /// Rename or move a file/folder and update all its descendants.
    /// Uses two UPDATE queries: one for the exact path, one for descendants.
    /// Only replaces the prefix portion to avoid issues with duplicate path segments.
    ///
    /// Returns the number of rows updated.
    pub fn rename_path(&self, old_path: &str, new_path: &str) -> Result<usize> {
        if old_path == new_path {
            return Ok(0);
        }

        let mut conn = self.connection()?;
        let old_prefix = format!("{}{}", old_path, std::path::MAIN_SEPARATOR);
        let new_prefix = format!("{}{}", new_path, std::path::MAIN_SEPARATOR);
        let descendant_like = format!("{}%", old_prefix);

        let total = (&mut *conn)
            .transaction::<usize, diesel::result::Error, _>(|tx_conn| {
                let exact = diesel::update(
                    file_metadata_dsl::file_metadata
                        .filter(file_metadata_dsl::local_path.eq(old_path)),
                )
                .set((
                    file_metadata_dsl::local_path.eq(new_path),
                ))
                .execute(tx_conn)?;

                let descendants = diesel::sql_query(
                    "UPDATE file_metadata \
                     SET local_path = ? || substr(local_path, length(?) + 1) \
                     WHERE local_path LIKE ?",
                )
                .bind::<Text, _>(&new_prefix)
                .bind::<Text, _>(&old_prefix)
                .bind::<Text, _>(&descendant_like)
                .execute(tx_conn)?;

                Ok(exact + descendants)
            })
            .context("Failed to rename metadata path")?;

        Ok(total)
    }
}

fn run_migrations(database_url: &str) -> Result<()> {
    let mut conn = SqliteConnection::establish(database_url)
        .with_context(|| format!("Failed to open inventory database at {}", database_url))?;
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|err| anyhow!("Failed to run inventory database migrations: {err}"))?;
    Ok(())
}

#[derive(Queryable)]
struct FileMetadataRow {
    id: i64,
    drive_id: String,
    is_folder: bool,
    local_path: String,
    created_at: i64,
    updated_at: i64,
    etag: String,
    metadata: String,
    props: Option<String>,
    permissions: String,
    shared: bool,
    size: i64,
}

#[derive(Queryable)]
struct TaskRow {
    id: String,
    drive_id: String,
    task_type: String,
    local_path: String,
    status: String,
    progress: f64,
    total_bytes: i64,
    processed_bytes: i64,
    priority: i32,
    custom_state: Option<String>,
    error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<TaskRow> for TaskRecord {
    type Error = anyhow::Error;

    fn try_from(row: TaskRow) -> Result<Self> {
        let status = TaskStatus::from_str(&row.status)
            .ok_or_else(|| anyhow!("Unknown task status value {}", row.status))?;
        let custom_state = match row.custom_state {
            Some(json) => Some(
                serde_json::from_str(&json).context("Failed to deserialize task custom_state")?,
            ),
            None => None,
        };

        Ok(TaskRecord {
            id: row.id,
            drive_id: row.drive_id,
            task_type: row.task_type,
            local_path: row.local_path,
            status,
            progress: row.progress,
            total_bytes: row.total_bytes,
            processed_bytes: row.processed_bytes,
            priority: row.priority,
            custom_state,
            error: row.error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Insertable)]
#[diesel(table_name = task_queue)]
struct NewTaskRow {
    id: String,
    drive_id: String,
    task_type: String,
    local_path: String,
    status: String,
    progress: f64,
    total_bytes: i64,
    processed_bytes: i64,
    priority: i32,
    custom_state: Option<String>,
    error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<&NewTaskRecord> for NewTaskRow {
    type Error = anyhow::Error;

    fn try_from(record: &NewTaskRecord) -> Result<Self> {
        Ok(Self {
            id: record.id.clone(),
            drive_id: record.drive_id.clone(),
            task_type: record.task_type.clone(),
            local_path: record.local_path.clone(),
            status: record.status.as_str().to_string(),
            progress: record.progress,
            total_bytes: record.total_bytes,
            processed_bytes: record.processed_bytes,
            priority: record.priority,
            custom_state: match &record.custom_state {
                Some(value) => Some(
                    serde_json::to_string(value)
                        .context("Failed to serialize task custom_state")?,
                ),
                None => None,
            },
            error: record.error.clone(),
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
    }
}

#[derive(AsChangeset)]
#[diesel(table_name = task_queue)]
struct TaskChangeset {
    status: Option<String>,
    progress: Option<f64>,
    total_bytes: Option<i64>,
    processed_bytes: Option<i64>,
    custom_state: Option<Option<String>>,
    error: Option<Option<String>>,
    updated_at: i64,
}

impl TaskChangeset {
    fn try_from(update: TaskUpdate) -> Result<Self> {
        let custom_state = match update.custom_state {
            Some(Some(value)) => Some(Some(
                serde_json::to_string(&value).context("Failed to serialize task custom_state")?,
            )),
            Some(None) => Some(None),
            None => None,
        };

        let error_state = match update.error {
            Some(Some(err)) => Some(Some(err)),
            Some(None) => Some(None),
            None => None,
        };

        Ok(Self {
            status: update.status.map(|status| status.as_str().to_string()),
            progress: update.progress,
            total_bytes: update.total_bytes,
            processed_bytes: update.processed_bytes,
            custom_state,
            error: error_state,
            updated_at: Utc::now().timestamp(),
        })
    }
}

#[derive(Insertable)]
#[diesel(table_name = file_metadata)]
struct NewFileMetadata {
    drive_id: String,
    is_folder: bool,
    local_path: String,
    created_at: i64,
    updated_at: i64,
    etag: String,
    metadata: String,
    props: Option<String>,
    permissions: String,
    shared: bool,
    size: i64,
}

#[derive(AsChangeset)]
#[diesel(table_name = file_metadata)]
struct FileMetadataChangeset {
    drive_id: String,
    is_folder: bool,
    updated_at: i64,
    etag: String,
    metadata: String,
    props: Option<String>,
    permissions: String,
    shared: bool,
    size: i64,
}

impl TryFrom<FileMetadataRow> for FileMetadata {
    type Error = anyhow::Error;

    fn try_from(row: FileMetadataRow) -> Result<Self> {
        let metadata_map: HashMap<String, String> =
            serde_json::from_str(&row.metadata).context("Failed to deserialize metadata column")?;
        let props_value = match row.props {
            Some(json) => {
                Some(serde_json::from_str(&json).context("Failed to deserialize props column")?)
            }
            None => None,
        };

        Ok(FileMetadata {
            id: row.id,
            drive_id: Uuid::parse_str(&row.drive_id).context("Failed to parse drive_id column")?,
            is_folder: row.is_folder,
            local_path: row.local_path,
            created_at: row.created_at,
            updated_at: row.updated_at,
            etag: row.etag,
            metadata: metadata_map,
            props: props_value,
            permissions: row.permissions,
            shared: row.shared,
            size: row.size,
        })
    }
}

impl TryFrom<&MetadataEntry> for NewFileMetadata {
    type Error = anyhow::Error;

    fn try_from(entry: &MetadataEntry) -> Result<Self> {
        Ok(Self {
            drive_id: entry.drive_id.to_string(),
            is_folder: entry.is_folder,
            local_path: entry.local_path.clone(),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            etag: entry.etag.clone(),
            metadata: serde_json::to_string(&entry.metadata)
                .context("Failed to serialize metadata map")?,
            props: entry
                .props
                .as_ref()
                .map(|p| serde_json::to_string(p))
                .transpose()
                .context("Failed to serialize props field")?,
            permissions: entry.permissions.clone(),
            shared: entry.shared,
            size: entry.size,
        })
    }
}

impl FileMetadataChangeset {
    fn from_entry(entry: &MetadataEntry) -> Result<Self> {
        Ok(Self {
            drive_id: entry.drive_id.to_string(),
            is_folder: entry.is_folder,
            updated_at: entry.updated_at,
            etag: entry.etag.clone(),
            metadata: serde_json::to_string(&entry.metadata)
                .context("Failed to serialize metadata map")?,
            props: entry
                .props
                .as_ref()
                .map(|p| serde_json::to_string(p))
                .transpose()
                .context("Failed to serialize props field")?,
            permissions: entry.permissions.clone(),
            shared: entry.shared,
            size: entry.size,
        })
    }
}

// =========================================================================
// Upload Session Row Types
// =========================================================================

#[derive(Queryable)]
struct UploadSessionQueryRow {
    id: String,
    task_id: String,
    drive_id: String,
    local_path: String,
    remote_uri: String,
    file_size: i64,
    chunk_size: i64,
    policy_type: String,
    session_data: String,
    chunk_progress: String,
    encrypt_metadata: Option<String>,
    expires_at: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = upload_sessions)]
struct UploadSessionRow {
    id: String,
    task_id: String,
    drive_id: String,
    local_path: String,
    remote_uri: String,
    file_size: i64,
    chunk_size: i64,
    policy_type: String,
    session_data: String,
    chunk_progress: String,
    encrypt_metadata: Option<String>,
    expires_at: i64,
    created_at: i64,
    updated_at: i64,
}

impl UploadSessionRow {
    fn from_session(session: &crate::uploader::UploadSession) -> Result<Self> {
        let credential_json = serde_json::to_string(session.credential())
            .context("Failed to serialize upload credential")?;
        let chunk_progress_json = serde_json::to_string(&session.chunk_progress)
            .context("Failed to serialize chunk progress")?;
        let encrypt_metadata_json = session
            .encrypt_metadata
            .as_ref()
            .map(|m| serde_json::to_string(m))
            .transpose()
            .context("Failed to serialize encrypt metadata")?;

        Ok(Self {
            id: session.id.clone(),
            task_id: session.task_id.clone(),
            drive_id: session.drive_id.clone(),
            local_path: session.local_path.clone(),
            remote_uri: session.remote_uri.clone(),
            file_size: session.file_size as i64,
            chunk_size: session.chunk_size as i64,
            policy_type: session.policy_type().as_str().to_string(),
            session_data: credential_json,
            chunk_progress: chunk_progress_json,
            encrypt_metadata: encrypt_metadata_json,
            expires_at: session.expires_at,
            created_at: session.created_at,
            updated_at: session.updated_at,
        })
    }
}

impl TryFrom<UploadSessionQueryRow> for crate::uploader::UploadSession {
    type Error = anyhow::Error;

    fn try_from(row: UploadSessionQueryRow) -> Result<Self> {
        let credential: cloudreve_api::models::explorer::UploadCredential =
            serde_json::from_str(&row.session_data)
                .context("Failed to deserialize upload credential")?;
        let chunk_progress: Vec<crate::uploader::ChunkProgress> =
            serde_json::from_str(&row.chunk_progress)
                .context("Failed to deserialize chunk progress")?;
        let encrypt_metadata = row
            .encrypt_metadata
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .context("Failed to deserialize encrypt metadata")?;

        // Reconstruct the session
        let mut session = crate::uploader::UploadSession::new(
            row.task_id,
            row.drive_id,
            row.local_path,
            row.remote_uri,
            row.file_size as u64,
            credential,
        );

        // Restore persisted state
        session.id = row.id;
        session.chunk_progress = chunk_progress;
        session.encrypt_metadata = encrypt_metadata;
        session.created_at = row.created_at;
        session.updated_at = row.updated_at;

        Ok(session)
    }
}

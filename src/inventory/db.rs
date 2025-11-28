use super::{FileMetadata, MetadataEntry};
use anyhow::Result;
use dirs::home_dir;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS file_metadata (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    drive_id TEXT NOT NULL,
    is_folder INTEGER NOT NULL,
    local_path TEXT NOT NULL UNIQUE,
    remote_uri TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    etag TEXT NOT NULL,
    metadata TEXT NOT NULL,
    props TEXT,
    permissions TEXT,
    shared BOOLEAN NOT NULL,
    size INTEGER NOT NULL,
    UNIQUE(local_path)
);

CREATE INDEX IF NOT EXISTS idx_drive_id ON file_metadata(drive_id);
CREATE INDEX IF NOT EXISTS idx_local_path ON file_metadata(local_path);
CREATE INDEX IF NOT EXISTS idx_updated_at ON file_metadata(updated_at);
"#;

/// SQLite-backed inventory database for file metadata
pub struct InventoryDb {
    conn: Arc<Mutex<Connection>>,
}

impl InventoryDb {
    /// Create or open the inventory database at the default location
    /// (~/.cloudreve/meta.db)
    pub fn new() -> Result<Self> {
        let db_path = Self::get_db_path()?;
        Self::with_path(db_path)
    }

    /// Create or open the inventory database at a specific path
    pub fn with_path(path: PathBuf) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&path)?;
        let db = InventoryDb {
            conn: Arc::new(Mutex::new(conn)),
        };

        db.init_schema()?;
        Ok(db)
    }

    /// Get the default database path (~/.cloudreve/meta.db)
    fn get_db_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or(anyhow::anyhow!("Unable to determine home directory"))?;
        Ok(home.join(".cloudreve").join("meta.db"))
    }

    /// Initialize the database schema
    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    pub fn batch_insert(&self, entries: &[MetadataEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        for entry in entries {
            let metadata_json = serde_json::to_string(&entry.metadata)?;
            let props_json = entry
                .props
                .as_ref()
                .map(|p| serde_json::to_string(p))
                .transpose()?;

            tx.execute(
                r#"
                INSERT INTO file_metadata 
                (drive_id, is_folder, local_path, remote_uri, created_at, updated_at, etag, metadata, props, permissions, shared, size)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    entry.drive_id.to_string(),
                    entry.is_folder,
                    entry.local_path,
                    entry.remote_uri,
                    entry.created_at,
                    entry.updated_at,
                    entry.etag,
                    metadata_json,
                    props_json,
                    entry.permissions,
                    entry.shared,
                    entry.size,
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn nuke_drive(&self, drive_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM file_metadata WHERE drive_id = ?1",
            params![drive_id],
        )?;
        Ok(())
    }

    /// Insert a new file metadata entry
    pub fn insert(&self, entry: &MetadataEntry) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        let metadata_json = serde_json::to_string(&entry.metadata)?;
        let props_json = entry
            .props
            .as_ref()
            .map(|p| serde_json::to_string(p))
            .transpose()?;

        conn.execute(
            r#"
            INSERT INTO file_metadata 
            (drive_id, is_folder, local_path, remote_uri, created_at, updated_at, etag, metadata, props, permissions, shared, size)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,?11,?12 )
            "#,
            params![
                entry.drive_id.to_string(),
                entry.is_folder,
                entry.local_path,
                entry.remote_uri,
                entry.created_at,
                entry.updated_at,
                entry.etag,
                metadata_json,
                props_json,
                entry.permissions,
                entry.shared,
                entry.size,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Update an existing file metadata entry by local path
    pub fn update(&self, entry: &MetadataEntry) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        let metadata_json = serde_json::to_string(&entry.metadata)?;
        let props_json = entry
            .props
            .as_ref()
            .map(|p| serde_json::to_string(p))
            .transpose()?;

        let rows_affected = conn.execute(
            r#"
            UPDATE file_metadata 
            SET drive_id = ?1, 
                is_folder = ?2,
                remote_uri = ?3, 
                updated_at = ?4, 
                etag = ?5, 
                metadata = ?6, 
                props = ?7,
                permissions = ?8,
                shared = ?9,
                size = ?11
            WHERE local_path = ?10
            "#,
            params![
                entry.drive_id.to_string(),
                entry.is_folder,
                entry.remote_uri,
                now,
                entry.etag,
                metadata_json,
                props_json,
                entry.permissions,
                entry.shared,
                entry.local_path,
                entry.size,
            ],
        )?;

        Ok(rows_affected > 0)
    }

    /// Insert or update a file metadata entry (upsert based on local_path)
    pub fn upsert(&self, entry: &MetadataEntry) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        let metadata_json = serde_json::to_string(&entry.metadata)?;
        let props_json = entry
            .props
            .as_ref()
            .map(|p| serde_json::to_string(p))
            .transpose()?;

        conn.execute(
            r#"
            INSERT INTO file_metadata 
            (drive_id, is_folder, local_path, remote_uri, created_at, updated_at, etag, metadata, props, permissions, shared, size)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(local_path) DO UPDATE SET
                drive_id = excluded.drive_id,
                is_folder = excluded.is_folder,
                remote_uri = excluded.remote_uri,
                updated_at = excluded.updated_at,
                etag = excluded.etag,
                metadata = excluded.metadata,
                props = excluded.props,
                permissions = excluded.permissions,
                shared = excluded.shared,
                size = excluded.size
            "#,
            params![
                entry.drive_id.to_string(),
                entry.is_folder,
                entry.local_path,
                entry.remote_uri,
                now,
                now,
                entry.etag,
                metadata_json,
                props_json,
                entry.permissions,
                entry.shared,
                entry.size,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Query file metadata by local path
    pub fn query_by_path(&self, local_path: &str) -> Result<Option<FileMetadata>> {
        let conn = self.conn.lock().unwrap();

        let result = conn
            .query_row(
                r#"
                SELECT id, drive_id, is_folder, local_path, remote_uri, created_at, updated_at, etag, metadata, props, permissions, shared, size
                FROM file_metadata
                WHERE local_path = ?1
                "#,
                params![local_path],
                |row| {
                    let drive_id_str: String = row.get(1)?;
                    let is_folder: bool = row.get(2)?;
                    let metadata_json: String = row.get(8)?;
                    let props_json: Option<String> = row.get(9)?;

                    Ok(FileMetadata {
                        id: row.get(0)?,
                        drive_id: Uuid::parse_str(&drive_id_str).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        is_folder,
                        local_path: row.get(3)?,
                        remote_uri: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        etag: row.get(7)?,
                        metadata: serde_json::from_str(&metadata_json).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                7,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        props: props_json
                            .map(|s| serde_json::from_str(&s))
                            .transpose()
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    8,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?,
                        permissions: row.get(10)?,
                        shared: row.get(11)?,
                        size: row.get(12)?,
                    })
                },
            )
            .optional()?;

        Ok(result)
    }

    /// Batch delete file metadata by local path
    pub fn batch_delete_by_path(&self, local_path: Vec<&str>) -> Result<bool> {
        // Batch delete items by paths, delete exact matched path, also all path under the given path
        if local_path.is_empty() {
            return Ok(false);
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let mut total_affected = 0;

        for path in local_path {
            // Delete the exact path
            let rows_affected = tx.execute(
                "DELETE FROM file_metadata WHERE local_path = ?1",
                params![path],
            )?;
            total_affected += rows_affected;

            // Delete all paths under this path (children)
            // Add a path separator to ensure we only match paths that are under this directory
            let path_prefix = format!("{}/", path);
            let rows_affected = tx.execute(
                "DELETE FROM file_metadata WHERE local_path LIKE ?1",
                params![format!("{}%", path_prefix)],
            )?;
            total_affected += rows_affected;
        }

        tx.commit()?;
        Ok(total_affected > 0)
    }

    /// Get total count of entries in the database
    pub fn count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM file_metadata", [], |row| row.get(0))?;

        Ok(count)
    }

    /// Clear all entries from the database
    pub fn clear(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM file_metadata", [])?;
        Ok(())
    }
}

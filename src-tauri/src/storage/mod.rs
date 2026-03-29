use crate::download::DownloadRecord;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct NewDownloadRecord {
    pub url: String,
    pub file_name: String,
    pub save_path: PathBuf,
    pub total_bytes: Option<u64>,
    pub expected_checksum: Option<String>,
    pub scheduled_at: Option<String>,
    pub bandwidth_limit_kbps: Option<u64>,
}

pub struct Storage {
    connection: Arc<Mutex<Connection>>,
}

impl Storage {
    pub fn clone_for_task(&self) -> Self {
        Self {
            connection: self.connection.clone(),
        }
    }
}

fn path_to_string(path: &PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

impl Storage {
    pub fn open(db_path: &std::path::Path) -> Result<Self, String> {
        let connection = Connection::open(db_path)
            .map_err(|error| format!("failed to open database: {error}"))?;

        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS downloads (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    url TEXT NOT NULL,
                    file_name TEXT NOT NULL,
                    save_path TEXT NOT NULL,
                    total_bytes INTEGER,
                    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
                    status TEXT NOT NULL,
                    error_message TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );",
            )
            .map_err(|error| format!("failed to initialize database schema: {error}"))?;

        let add_column = |col: &str, col_type: &str| {
            let sql = format!("ALTER TABLE downloads ADD COLUMN {col} {col_type}");
            let _ = connection.execute(&sql, []);
        };
        add_column("expected_checksum", "TEXT");
        add_column("actual_checksum", "TEXT");
        add_column("checksum_status", "TEXT");
        add_column("scheduled_at", "TEXT");
        add_column("bandwidth_limit_kbps", "INTEGER");

        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS app_settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );",
            )
            .map_err(|error| format!("failed to create settings table: {error}"))?;

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn insert_download(&self, record: NewDownloadRecord) -> Result<DownloadRecord, String> {
        let connection = self.connection.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO downloads (
                    url, file_name, save_path, total_bytes, downloaded_bytes,
                    status, created_at, updated_at, expected_checksum,
                    scheduled_at, bandwidth_limit_kbps
                ) VALUES (?1, ?2, ?3, ?4, 0, 'queued', ?5, ?5, ?6, ?7, ?8)",
                params![
                    record.url,
                    record.file_name,
                    path_to_string(&record.save_path),
                    record.total_bytes.map(|v| v as i64),
                    now,
                    record.expected_checksum,
                    record.scheduled_at,
                    record.bandwidth_limit_kbps.map(|v| v as i64),
                ],
            )
            .map_err(|error| format!("failed to insert download record: {error}"))?;

        let id = connection.last_insert_rowid();
        drop(connection);
        self.get_download(id)
    }

    pub fn get_download(&self, id: i64) -> Result<DownloadRecord, String> {
        let connection = self.connection.lock().unwrap();
        connection
            .query_row(
                "SELECT id, url, file_name, save_path, total_bytes, downloaded_bytes, status,
                        error_message, expected_checksum, actual_checksum, checksum_status,
                        scheduled_at, bandwidth_limit_kbps
                 FROM downloads WHERE id = ?1",
                params![id],
                |row| {
                    Ok(DownloadRecord {
                        id: row.get("id")?,
                        url: row.get("url")?,
                        file_name: row.get("file_name")?,
                        save_path: row.get("save_path")?,
                        total_bytes: row.get::<_, Option<i64>>("total_bytes")?.map(|v| v as u64),
                        downloaded_bytes: row.get::<_, i64>("downloaded_bytes")? as u64,
                        status: row.get("status")?,
                        error_message: row.get("error_message")?,
                        expected_checksum: row.get("expected_checksum")?,
                        actual_checksum: row.get("actual_checksum")?,
                        checksum_status: row.get("checksum_status")?,
                        scheduled_at: row.get("scheduled_at")?,
                        bandwidth_limit_kbps: row.get::<_, Option<i64>>("bandwidth_limit_kbps")?.map(|v| v as u64),
                    })
                },
            )
            .map_err(|error| format!("failed to fetch download record: {error}"))
    }

    pub fn list_downloads(&self) -> Result<Vec<DownloadRecord>, String> {
        let connection = self.connection.lock().unwrap();
        let mut stmt = connection
            .prepare(
                "SELECT id, url, file_name, save_path, total_bytes, downloaded_bytes, status,
                        error_message, expected_checksum, actual_checksum, checksum_status,
                        scheduled_at, bandwidth_limit_kbps
                 FROM downloads ORDER BY id DESC",
            )
            .map_err(|error| format!("failed to prepare download list query: {error}"))?;

        let records = stmt
            .query_map([], |row| {
                Ok(DownloadRecord {
                    id: row.get("id")?,
                    url: row.get("url")?,
                    file_name: row.get("file_name")?,
                    save_path: row.get("save_path")?,
                    total_bytes: row.get::<_, Option<i64>>("total_bytes")?.map(|v| v as u64),
                    downloaded_bytes: row.get::<_, i64>("downloaded_bytes")? as u64,
                    status: row.get("status")?,
                    error_message: row.get("error_message")?,
                    expected_checksum: row.get("expected_checksum")?,
                    actual_checksum: row.get("actual_checksum")?,
                    checksum_status: row.get("checksum_status")?,
                    scheduled_at: row.get("scheduled_at")?,
                    bandwidth_limit_kbps: row.get::<_, Option<i64>>("bandwidth_limit_kbps")?.map(|v| v as u64),
                })
            })
            .map_err(|error| format!("failed to list downloads: {error}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    pub fn set_status(
        &self,
        id: i64,
        status: &str,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        error_message: Option<&str>,
    ) -> Result<(), String> {
        let connection = self.connection.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        connection
            .execute(
                "UPDATE downloads SET status = ?1, downloaded_bytes = ?2, total_bytes = ?3,
                        error_message = ?4, updated_at = ?5 WHERE id = ?6",
                params![
                    status,
                    downloaded_bytes as i64,
                    total_bytes.map(|v| v as i64),
                    error_message,
                    now,
                    id,
                ],
            )
            .map_err(|error| format!("failed to update download status: {error}"))?;
        Ok(())
    }

    pub fn delete_completed(&self) -> Result<u64, String> {
        let connection = self.connection.lock().unwrap();
        let count = connection
            .execute(
                "DELETE FROM downloads WHERE status IN ('completed', 'failed', 'cancelled')",
                [],
            )
            .map_err(|error| format!("failed to clear completed downloads: {error}"))?;
        Ok(count as u64)
    }

    pub fn set_checksum_verification(
        &self,
        id: i64,
        actual_checksum: Option<&str>,
        checksum_status: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), String> {
        let connection = self.connection.lock().unwrap();
        connection
            .execute(
                "UPDATE downloads SET actual_checksum = ?1, checksum_status = ?2,
                        error_message = COALESCE(?3, error_message) WHERE id = ?4",
                params![actual_checksum, checksum_status, error_message, id],
            )
            .map_err(|error| format!("failed to update checksum verification: {error}"))?;
        Ok(())
    }

    pub fn get_resumable_downloads(&self) -> Result<Vec<DownloadRecord>, String> {
        let connection = self.connection.lock().unwrap();
        let mut stmt = connection
            .prepare(
                "SELECT id, url, file_name, save_path, total_bytes, downloaded_bytes, status,
                        error_message, expected_checksum, actual_checksum, checksum_status,
                        scheduled_at, bandwidth_limit_kbps
                 FROM downloads WHERE status IN ('queued', 'in_progress', 'scheduled')
                 ORDER BY id ASC",
            )
            .map_err(|error| format!("failed to query resumable downloads: {error}"))?;

        let records = stmt
            .query_map([], |row| {
                Ok(DownloadRecord {
                    id: row.get("id")?,
                    url: row.get("url")?,
                    file_name: row.get("file_name")?,
                    save_path: row.get("save_path")?,
                    total_bytes: row.get::<_, Option<i64>>("total_bytes")?.map(|v| v as u64),
                    downloaded_bytes: row.get::<_, i64>("downloaded_bytes")? as u64,
                    status: row.get("status")?,
                    error_message: row.get("error_message")?,
                    expected_checksum: row.get("expected_checksum")?,
                    actual_checksum: row.get("actual_checksum")?,
                    checksum_status: row.get("checksum_status")?,
                    scheduled_at: row.get("scheduled_at")?,
                    bandwidth_limit_kbps: row.get::<_, Option<i64>>("bandwidth_limit_kbps")?.map(|v| v as u64),
                })
            })
            .map_err(|error| format!("failed to list resumable downloads: {error}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        let connection = self.connection.lock().unwrap();
        let result = connection.query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );
        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(format!("failed to read setting: {error}")),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let connection = self.connection.lock().unwrap();
        connection
            .execute(
                "INSERT OR REPLACE INTO app_settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|error| format!("failed to write setting: {error}"))?;
        Ok(())
    }
}

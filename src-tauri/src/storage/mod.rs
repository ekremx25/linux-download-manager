use crate::download::DownloadRecord;
use crate::library;
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
    pub category: String,
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
        add_column("category", "TEXT");
        add_column("resume_job", "TEXT");

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

    // Kept out of DownloadRecord/IPC: captured headers can contain credentials.
    pub fn save_resume_job(&self, job: &crate::app::QueuedDownload) -> Result<(), String> {
        let json = serde_json::to_string(job).map_err(|e| e.to_string())?;
        self.connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE downloads SET resume_job = ?1 WHERE id = ?2",
                params![json, job.id],
            )
            .map_err(|e| format!("failed to save resume information: {e}"))?;
        Ok(())
    }

    pub fn load_resume_job(&self, id: i64) -> Result<Option<crate::app::QueuedDownload>, String> {
        let json: Option<String> = self
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT resume_job FROM downloads WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to read resume information: {e}"))?;
        json.map(|value| {
            serde_json::from_str(&value)
                .map_err(|_| "Saved resume information is invalid".to_string())
        })
        .transpose()
    }

    pub fn insert_download(&self, record: NewDownloadRecord) -> Result<DownloadRecord, String> {
        let connection = self.connection.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO downloads (
                    url, file_name, save_path, total_bytes, downloaded_bytes,
                    status, created_at, updated_at, expected_checksum,
                    scheduled_at, bandwidth_limit_kbps, category
                ) VALUES (?1, ?2, ?3, ?4, 0, 'queued', ?5, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.url,
                    record.file_name,
                    path_to_string(&record.save_path),
                    record.total_bytes.map(|v| v as i64),
                    now,
                    record.expected_checksum,
                    record.scheduled_at,
                    record.bandwidth_limit_kbps.map(|v| v as i64),
                    record.category,
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
                        scheduled_at, bandwidth_limit_kbps, category
                 FROM downloads WHERE id = ?1",
                params![id],
                |row| {
                    let file_name: String = row.get("file_name")?;
                    Ok(DownloadRecord {
                        id: row.get("id")?,
                        url: row.get("url")?,
                        file_name: file_name.clone(),
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
                        // Rows written before categories existed are classified
                        // on read through the same table, never a second copy.
                        category: row
                            .get::<_, Option<String>>("category")?
                            .unwrap_or_else(|| library::classify(&file_name).to_string()),
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
                        scheduled_at, bandwidth_limit_kbps, category
                 FROM downloads ORDER BY id DESC",
            )
            .map_err(|error| format!("failed to prepare download list query: {error}"))?;

        let records = stmt
            .query_map([], |row| {
                let file_name: String = row.get("file_name")?;
                Ok(DownloadRecord {
                    id: row.get("id")?,
                    url: row.get("url")?,
                    file_name: file_name.clone(),
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
                    // Rows written before categories existed are classified on
                    // read through the same table, never a duplicated copy.
                    category: row
                        .get::<_, Option<String>>("category")?
                        .unwrap_or_else(|| library::classify(&file_name).to_string()),
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

    /// Removes a single history entry. The downloaded file on disk is never
    /// touched; only the list/history record goes away.
    pub fn delete_download(&self, id: i64) -> Result<bool, String> {
        let connection = self.connection.lock().unwrap();
        let count = connection
            .execute("DELETE FROM downloads WHERE id = ?1", params![id])
            .map_err(|error| format!("failed to clear download entry: {error}"))?;
        Ok(count > 0)
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
                        scheduled_at, bandwidth_limit_kbps, category
                 FROM downloads WHERE status IN ('queued', 'in_progress', 'scheduled')
                 ORDER BY id ASC",
            )
            .map_err(|error| format!("failed to query resumable downloads: {error}"))?;

        let records = stmt
            .query_map([], |row| {
                let file_name: String = row.get("file_name")?;
                Ok(DownloadRecord {
                    id: row.get("id")?,
                    url: row.get("url")?,
                    file_name: file_name.clone(),
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
                    // Rows written before categories existed are classified on
                    // read through the same table, never a duplicated copy.
                    category: row
                        .get::<_, Option<String>>("category")?
                        .unwrap_or_else(|| library::classify(&file_name).to_string()),
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

#[cfg(test)]
mod category_tests {
    use super::*;

    fn temp_db(name: &str) -> (PathBuf, Storage) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".temp_files");
        std::fs::create_dir_all(&root).unwrap();
        let dir = root.join(format!("storage-tests-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("downloads.sqlite3");
        let storage = Storage::open(&path).unwrap();
        (dir, storage)
    }

    fn insert(storage: &Storage, file_name: &str, category: &str) -> i64 {
        storage
            .insert_download(NewDownloadRecord {
                url: "https://example.com/f".to_string(),
                file_name: file_name.to_string(),
                save_path: PathBuf::from("/tmp").join(file_name),
                total_bytes: Some(100),
                expected_checksum: None,
                scheduled_at: None,
                bandwidth_limit_kbps: None,
                category: category.to_string(),
            })
            .unwrap()
            .id
    }

    #[test]
    fn category_survives_a_restart_and_legacy_rows_are_classified_on_read() {
        let (dir, storage) = temp_db("category");
        let iso = insert(&storage, "ubuntu.iso", "iso");
        let video = insert(&storage, "movie.mkv", "video");

        // A row written before categories existed, classified on read instead.
        storage
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO downloads (url, file_name, save_path, total_bytes, downloaded_bytes, status, created_at, updated_at)
                 VALUES ('https://example.com/legacy', 'legacy.jpg', '/tmp/legacy.jpg', 100, 0, 'paused', 'now', 'now')",
                [],
            )
            .unwrap();

        // Reopening the same file is what an application restart does.
        drop(storage);
        let reopened = Storage::open(&dir.join("downloads.sqlite3")).unwrap();
        let all = reopened.list_downloads().unwrap();
        let find = |name: &str| all.iter().find(|r| r.file_name == name).unwrap().category.clone();

        assert_eq!(find("ubuntu.iso"), "iso");
        assert_eq!(find("movie.mkv"), "video");
        assert_eq!(
            find("legacy.jpg"),
            "image",
            "a row without a stored category must still be classified"
        );
        assert_eq!(reopened.get_download(iso).unwrap().category, "iso");
        assert_eq!(reopened.get_download(video).unwrap().category, "video");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

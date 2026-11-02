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
    // SQLite may reopen journals using the descriptor-anchored database path.
    directory: Arc<std::fs::File>,
}

impl Storage {
    pub fn clone_for_task(&self) -> Self {
        Self {
            connection: self.connection.clone(),
            directory: self.directory.clone(),
        }
    }
}

fn path_to_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

// Open each component relative to a held directory, never following symlinks.
// Permissions are changed through file descriptors, not attacker-replaceable paths.
#[cfg(target_os = "linux")]
fn private_directory(path: &std::path::Path) -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::path::Component;
    const O_DIRECTORY: i32 = 0o200000;
    const O_NOFOLLOW: i32 = 0o400000;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?.join(path)
    };
    let mut directory = std::fs::File::open("/").map_err(|e| e.to_string())?;
    let mut child_seen = false;
    for component in absolute.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            _ => return Err("App data path must not contain parent components".into()),
        };
        child_seen = true;
        let child = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
        match std::fs::DirBuilder::new().mode(0o700).create(&child) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("failed to create private directory: {e}")),
        }
        directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_DIRECTORY | O_NOFOLLOW)
            .open(child)
            .map_err(|e| format!("failed to open private directory: {e}"))?;
    }
    if !child_seen {
        return Err("App data directory cannot be the filesystem root".into());
    }
    directory
        .set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("failed to protect app data directory: {e}"))?;
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn private_database(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    const O_NOFOLLOW: i32 = 0o400000;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .map_err(|e| format!("failed to open private database file: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err("Database files must be regular files with one link".into());
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("failed to protect database file: {e}"))
}

#[cfg(not(target_os = "linux"))]
fn open_private_database(_: &std::path::Path) -> Result<(std::fs::File, Connection), String> {
    Err("Private database storage is supported on Linux only".into())
}

#[cfg(target_os = "linux")]
fn open_private_database(db_path: &std::path::Path) -> Result<(std::fs::File, Connection), String> {
    use std::os::fd::AsRawFd;
    let parent = db_path.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let directory = private_directory(parent)?;
    let name = db_path.file_name().ok_or("Database file name is missing")?;
    let anchored = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
    private_database(&anchored)?;
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sibling = anchored.as_os_str().to_os_string();
        sibling.push(suffix);
        let sibling = PathBuf::from(sibling);
        match std::fs::symlink_metadata(&sibling) {
            Ok(_) => private_database(&sibling)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("failed to inspect database sidecar: {e}")),
        }
    }
    // The proc descriptor path intentionally resolves our held directory;
    // untrusted components and final files were opened with O_NOFOLLOW above.
    let connection = Connection::open_with_flags(&anchored,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|error| format!("failed to open database: {error}"))?;
    connection.execute_batch("PRAGMA secure_delete = ON;")
        .map_err(|e| format!("failed to protect stored credentials: {e}"))?;
    Ok((directory, connection))
}

impl Storage {
    pub fn open(db_path: &std::path::Path) -> Result<Self, String> {
        let (directory, connection) = open_private_database(db_path)?;

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
        connection.execute(
            "UPDATE downloads SET resume_job = NULL
             WHERE status IN ('completed', 'cancelled') AND resume_job IS NOT NULL", [],
        ).map_err(|e| format!("failed to clear terminal request credentials: {e}"))?;

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
            directory: Arc::new(directory),
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

    /// Clear the old schedule in both persisted representations in one write.
    pub fn prepare_resume(&self, job: &mut crate::app::QueuedDownload) -> Result<(), String> {
        job.scheduled_at = None;
        let json = serde_json::to_string(job).map_err(|e| e.to_string())?;
        let changed = self.connection.lock().unwrap().execute(
            "UPDATE downloads SET resume_job = ?1, scheduled_at = NULL,
                status = 'queued', error_message = NULL, actual_checksum = NULL,
                checksum_status = NULL, updated_at = ?2
             WHERE id = ?3 AND status IN ('paused', 'failed', 'cancelled')",
            params![json, chrono::Utc::now().to_rfc3339(), job.id],
        ).map_err(|e| format!("failed to persist resumed download: {e}"))?;
        if changed != 1 {
            return Err("Only paused, failed, or cancelled downloads can be resumed".into());
        }
        Ok(())
    }

    pub fn update_download_url(&self, id: i64, url: &str) -> Result<(), String> {
        self.connection
            .lock()
            .unwrap()
            .execute("UPDATE downloads SET url = ?1 WHERE id = ?2", params![url, id])
            .map_err(|e| format!("failed to update download URL: {e}"))?;
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
                        bandwidth_limit_kbps: row
                            .get::<_, Option<i64>>("bandwidth_limit_kbps")?
                            .map(|v| v as u64),
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
                    bandwidth_limit_kbps: row
                        .get::<_, Option<i64>>("bandwidth_limit_kbps")?
                        .map(|v| v as u64),
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
                        error_message = ?4, updated_at = ?5,
                        resume_job = CASE WHEN ?1 IN ('completed', 'cancelled')
                            THEN NULL ELSE resume_job END WHERE id = ?6",
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
                "DELETE FROM downloads WHERE status = 'completed'",
                [],
            )
            .map_err(|error| format!("failed to clear completed downloads: {error}"))?;
        Ok(count as u64)
    }

    /// Publish a terminal outcome only after integrity verification has finished.
    pub fn finish_download(
        &self,
        id: i64,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        actual_checksum: Option<&str>,
        checksum_status: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), String> {
        let status = if error_message.is_none()
            && matches!(checksum_status, None | Some("verified")) {
            "completed"
        } else {
            "failed"
        };
        self.connection.lock().unwrap().execute(
            "UPDATE downloads SET status = ?1, downloaded_bytes = ?2, total_bytes = ?3,
                actual_checksum = ?4, checksum_status = ?5, error_message = ?6,
                updated_at = ?7, resume_job = CASE WHEN ?1 = 'completed'
                    THEN NULL ELSE resume_job END WHERE id = ?8",
            params![status, downloaded_bytes as i64, total_bytes.map(|v| v as i64),
                actual_checksum, checksum_status, error_message,
                chrono::Utc::now().to_rfc3339(), id],
        ).map_err(|e| format!("failed to finalize download: {e}"))?;
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
                    bandwidth_limit_kbps: row
                        .get::<_, Option<i64>>("bandwidth_limit_kbps")?
                        .map(|v| v as u64),
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
        let root = std::env::var_os("LDM_TEST_ROOT").map(PathBuf::from).unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join(".temp_files")
        });
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
        let find = |name: &str| {
            all.iter()
                .find(|r| r.file_name == name)
                .unwrap()
                .category
                .clone()
        };

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

    #[test]
    fn terminal_status_clears_credentials_but_retryable_statuses_keep_them() {
        let (dir, storage) = temp_db("credential-lifecycle");
        for status in ["paused", "failed", "queued", "in_progress", "scheduled", "completed", "cancelled"] {
            let id = insert(&storage, &format!("{status}.bin"), "other");
            storage.connection.lock().unwrap().execute(
                "UPDATE downloads SET resume_job = ?1 WHERE id = ?2",
                params!["private-fixture-credentials", id],
            ).unwrap();
            storage.set_status(id, status, 1, Some(100), None).unwrap();
            let context: Option<String> = storage.connection.lock().unwrap().query_row(
                "SELECT resume_job FROM downloads WHERE id = ?1", params![id], |r| r.get(0),
            ).unwrap();
            assert_eq!(context.is_none(), matches!(status, "completed" | "cancelled"), "{status}");
        }
        // Old versions retained terminal credentials; opening storage repairs
        // those rows without discarding paused/failed explicit-resume context.
        storage.connection.lock().unwrap().execute(
            "UPDATE downloads SET resume_job = 'legacy-private-credentials'", [],
        ).unwrap();
        drop(storage);
        let reopened = Storage::open(&dir.join("downloads.sqlite3")).unwrap();
        for record in reopened.list_downloads().unwrap() {
            let context: Option<String> = reopened.connection.lock().unwrap().query_row(
                "SELECT resume_job FROM downloads WHERE id = ?1", params![record.id], |r| r.get(0),
            ).unwrap();
            assert_eq!(context.is_none(), matches!(record.status.as_str(), "completed" | "cancelled"));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resume_persistence_rolls_back_all_fields_when_the_write_fails() {
        let (dir, storage) = temp_db("resume-rollback");
        let id = insert(&storage, "scheduled.bin", "other");
        let mut job = crate::app::QueuedDownload {
            transfer: Default::default(), id,
            url: "https://example.invalid/fixture".into(),
            fallback_urls: vec![], audio_url: None, source_page_url: None,
            http_headers: std::collections::HashMap::new(), format: None,
            force_ytdlp: false, stream_manifest: false,
            target_path: dir.join("scheduled.bin"), resumable_hint: true,
            total_bytes_hint: Some(100), expected_checksum: None,
            scheduled_at: Some("2099-01-01T00:00:00+00:00".into()), bandwidth_limit_kbps: None,
        };
        storage.save_resume_job(&job).unwrap();
        storage.connection.lock().unwrap().execute_batch(
            "UPDATE downloads SET status = 'paused', scheduled_at = '2099-01-01T00:00:00+00:00';
             CREATE TRIGGER reject_resume BEFORE UPDATE OF resume_job ON downloads
             BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;"
        ).unwrap();
        assert!(storage.prepare_resume(&mut job).is_err());
        let record = storage.get_download(id).unwrap();
        assert_eq!(record.status, "paused");
        assert_eq!(record.scheduled_at.as_deref(), Some("2099-01-01T00:00:00+00:00"));
        assert_eq!(storage.load_resume_job(id).unwrap().unwrap().scheduled_at, record.scheduled_at);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_storage_restricts_new_and_existing_directory_and_database_modes() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, storage) = temp_db("private-modes");
        let db = dir.join("downloads.sqlite3");
        let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&db), 0o600);
        drop(storage);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        let reopened = Storage::open(&db).unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&db), 0o600);
        assert_eq!(reopened.connection.lock().unwrap().query_row("PRAGMA secure_delete", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_storage_rejects_symlinked_directory_database_and_sidecar_without_touching_targets() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (dir, storage) = temp_db("private-symlinks");
        drop(storage);
        let outside = dir.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.join("linked-directory");
        symlink(&outside, &link).unwrap();
        assert!(Storage::open(&link.join("downloads.sqlite3")).is_err());
        assert!(Storage::open(&link.join("child/downloads.sqlite3")).is_err());
        assert_eq!(std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!outside.join("downloads.sqlite3").exists());
        let sentinel = dir.join("sentinel");
        std::fs::write(&sentinel, b"do not modify").unwrap();
        std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o644)).unwrap();
        let db = dir.join("downloads.sqlite3");
        std::fs::remove_file(&db).unwrap();
        symlink(&sentinel, &db).unwrap();
        assert!(Storage::open(&db).is_err());
        std::fs::remove_file(&db).unwrap();
        drop(Storage::open(&db).unwrap());
        let journal = dir.join("downloads.sqlite3-journal");
        symlink(&sentinel, &journal).unwrap();
        assert!(Storage::open(&db).is_err());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"do not modify");
        assert_eq!(std::fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777, 0o644);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_storage_rejects_hardlinked_database_without_chmod_of_the_other_link() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, storage) = temp_db("private-hardlink");
        drop(storage);
        let db = dir.join("downloads.sqlite3");
        let other = dir.join("other-link");
        std::fs::hard_link(&db, &other).unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Storage::open(&db).is_err());
        assert_eq!(std::fs::metadata(&other).unwrap().permissions().mode() & 0o777, 0o644);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clear_completed_keeps_stopped_and_failed_downloads() {
        let (dir, storage) = temp_db("clear-completed");
        for status in ["completed", "paused", "failed", "cancelled"] {
            let id = insert(&storage, &format!("{status}.mp4"), "video");
            storage.set_status(id, status, 0, None, None).unwrap();
        }

        assert_eq!(storage.delete_completed().unwrap(), 1);
        let remaining = storage.list_downloads().unwrap();
        assert_eq!(remaining.len(), 3);
        assert!(remaining.iter().all(|record| record.status != "completed"));
        assert!(remaining.iter().any(|record| record.status == "paused"));
        assert!(remaining.iter().any(|record| record.status == "failed"));
        assert!(remaining.iter().any(|record| record.status == "cancelled"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

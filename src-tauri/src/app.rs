use crate::browser::{
    acknowledge_staged_browser_request, load_staged_browser_requests,
    quarantine_staged_browser_request,
};
use crate::download::{DownloadOptions, DownloadService};
use crate::jobs::{DownloadJobRequest, queue_download_request};
use crate::storage::Storage;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::platform::{resolve_app_data_dir, resolve_default_download_dir};

pub const DOWNLOAD_STATE_EVENT: &str = "download://state";

/// Settings key for the "Enable desktop notifications" preference.
/// Stored as `1`/`0` in the existing settings table, so no new config
/// system is introduced and the value survives a restart.
pub const DESKTOP_NOTIFICATIONS_SETTING: &str = "desktop_notifications_enabled";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStateEvent {
    pub id: i64,
    pub status: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub active_parts: Option<usize>,
    pub error_message: Option<String>,
}

/// The mutable half of a download progress event. Grouped into one struct so
/// `emit_download_event` stays inside the clippy argument budget, and so call
/// sites read as a labelled snapshot instead of a positional list where
/// swapping two `Option<u64>` fields would still type-check.
#[derive(Debug, Clone, Default)]
pub struct DownloadEventUpdate<'a> {
    pub status: &'a str,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub active_parts: Option<usize>,
    pub error_message: Option<&'a str>,
}

struct DownloadQueueState {
    active: HashMap<i64, JoinHandle<()>>,
    pending: VecDeque<QueuedDownload>,
}

#[derive(Clone)]
pub struct QueuedDownload {
    pub id: i64,
    pub url: String,
    pub fallback_urls: Vec<String>,
    pub audio_url: Option<String>,
    pub source_page_url: Option<String>,
    pub http_headers: HashMap<String, String>,
    pub format: Option<String>,
    pub force_ytdlp: bool,
    pub stream_manifest: bool,
    pub target_path: PathBuf,
    pub resumable_hint: bool,
    pub total_bytes_hint: Option<u64>,
    pub expected_checksum: Option<String>,
    pub scheduled_at: Option<String>,
    pub bandwidth_limit_kbps: Option<u64>,
}

pub struct AppState {
    pub storage: Storage,
    pub download_service: DownloadService,
    pub app_data_dir: PathBuf,
    pub default_download_dir: PathBuf,
    max_concurrent_downloads: AtomicUsize,
    default_bandwidth_limit_kbps: AtomicU64,
    /// User preference for OS-level download notifications. Defaults to on so
    /// existing users keep today's behaviour; only `send_download_notification`
    /// reads it, so the in-app status text is never affected.
    desktop_notifications_enabled: AtomicBool,
    selected_download_id: Mutex<Option<i64>>,
    queue: Mutex<DownloadQueueState>,
}

impl AppState {
    pub fn bootstrap() -> Result<Self, String> {
        let app_data_dir = resolve_app_data_dir()?;
        fs::create_dir_all(&app_data_dir)
            .map_err(|error| format!("failed to create app data directory: {error}"))?;

        let db_path = app_data_dir.join("downloads.sqlite3");
        let storage = Storage::open(&db_path)?;

        let default_download_dir = resolve_default_download_dir()?;
        fs::create_dir_all(&default_download_dir)
            .map_err(|error| format!("failed to create default download directory: {error}"))?;

        // The library tree exists from startup, not only once a download is
        // queued, so the sidebar folders are always browsable on disk.
        crate::library::ensure_library(&default_download_dir)?;

        let max_concurrent = storage
            .get_setting("max_concurrent_downloads")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3);

        let default_bandwidth = storage
            .get_setting("default_bandwidth_limit_kbps")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        // Absent or unparsable means "on": notifications must stay enabled for
        // every existing user until they explicitly turn them off.
        let desktop_notifications_enabled = storage
            .get_setting(DESKTOP_NOTIFICATIONS_SETTING)
            .ok()
            .flatten()
            .map(|value| value != "0")
            .unwrap_or(true);

        let client = Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .map_err(|error| format!("failed to create HTTP client: {error}"))?;

        Ok(Self {
            storage,
            download_service: DownloadService::new(client),
            app_data_dir,
            default_download_dir,
            max_concurrent_downloads: AtomicUsize::new(max_concurrent),
            default_bandwidth_limit_kbps: AtomicU64::new(default_bandwidth),
            desktop_notifications_enabled: AtomicBool::new(desktop_notifications_enabled),
            selected_download_id: Mutex::new(None),
            queue: Mutex::new(DownloadQueueState {
                active: HashMap::new(),
                pending: VecDeque::new(),
            }),
        })
    }

    /// Base directory for new downloads.
    ///
    /// The library tree is created here as a side effect, but the caller gets
    /// the *base* back: `ensure_category_dir` appends `LDM/<category>`, so
    /// returning the library root here would produce `LDM/LDM/<category>`.
    pub fn resolve_target_dir(&self, requested: Option<&str>) -> Result<PathBuf, String> {
        let base = match requested {
            Some(path) if !path.is_empty() => {
                let path = PathBuf::from(path);
                if path.is_relative() {
                    self.default_download_dir.join(path)
                } else {
                    path
                }
            }
            _ => self.default_download_dir.clone(),
        };
        crate::library::ensure_library(&base)?;
        Ok(base)
    }

    pub fn max_concurrent_downloads(&self) -> usize {
        self.max_concurrent_downloads.load(Ordering::Relaxed)
    }

    pub fn set_max_concurrent_downloads(&self, value: usize) {
        self.max_concurrent_downloads.store(value, Ordering::Relaxed);
        let _ = self.storage.set_setting("max_concurrent_downloads", &value.to_string());
    }

    pub fn default_bandwidth_limit_kbps(&self) -> Option<u64> {
        let value = self.default_bandwidth_limit_kbps.load(Ordering::Relaxed);
        if value == 0 { None } else { Some(value) }
    }

    pub fn set_default_bandwidth_limit_kbps(&self, value: Option<u64>) {
        self.default_bandwidth_limit_kbps.store(value.unwrap_or(0), Ordering::Relaxed);
        let _ = self.storage.set_setting("default_bandwidth_limit_kbps", &value.unwrap_or(0).to_string());
    }

    /// Whether OS-level download notifications are allowed. Read by the
    /// central `send_download_notification` guard only.
    pub fn desktop_notifications_enabled(&self) -> bool {
        self.desktop_notifications_enabled.load(Ordering::Relaxed)
    }

    /// Updates the preference in memory *and* in the settings table, so the
    /// change takes effect immediately and survives a restart.
    pub fn set_desktop_notifications_enabled(&self, enabled: bool) {
        self.desktop_notifications_enabled
            .store(enabled, Ordering::Relaxed);
        let _ = self.storage.set_setting(
            DESKTOP_NOTIFICATIONS_SETTING,
            if enabled { "1" } else { "0" },
        );
    }

    pub fn select_download(&self, id: i64) {
        *self.selected_download_id.lock().unwrap() = Some(id);
    }

    pub fn selected_download_id(&self) -> Option<i64> {
        *self.selected_download_id.lock().unwrap()
    }

    pub fn restore_download_queue(&self, app_handle: &AppHandle) -> Result<(), String> {
        let resumable = self.storage.get_resumable_downloads()?;
        for record in resumable {
            if record.status == "in_progress" {
                self.storage.set_status(
                    record.id,
                    "queued",
                    record.downloaded_bytes,
                    record.total_bytes,
                    None,
                )?;
            }
            self.emit_download_event(
                app_handle,
                record.id,
                DownloadEventUpdate {
                    status: &record.status,
                    downloaded_bytes: record.downloaded_bytes,
                    total_bytes: record.total_bytes,
                    active_parts: Some(0),
                    error_message: record.error_message.as_deref(),
                    ..Default::default()
                },
            );
            self.enqueue_download(
                app_handle,
                QueuedDownload {
                    id: record.id,
                    url: record.url,
                    fallback_urls: Vec::new(),
                    target_path: PathBuf::from(record.save_path),
                    audio_url: None,
                    source_page_url: None,
                    http_headers: HashMap::new(),
                    format: None,
                    force_ytdlp: false,
                    stream_manifest: false,
                    resumable_hint: record.downloaded_bytes > 0,
                    total_bytes_hint: record.total_bytes,
                    expected_checksum: record.expected_checksum,
                    scheduled_at: record.scheduled_at,
                    bandwidth_limit_kbps: record.bandwidth_limit_kbps,
                },
            )?;
        }
        Ok(())
    }

    pub async fn poll_browser_inbox(&self, app_handle: &AppHandle) -> Result<(), String> {
        let staged_requests = match load_staged_browser_requests(&self.app_data_dir) {
            Ok(requests) => requests,
            Err(error) => {
                send_download_notification(
                    app_handle,
                    "Browser integration error",
                    format!("The browser download queue could not be read: {error}"),
                );
                return Err(error);
            }
        };

        for staged in staged_requests {
            let source_page_url = staged.request.source_page_url.clone();
            let source_title = staged.request.source_title.clone();
            let download_url = staged.request.url.clone();

            let result = queue_download_request(
                app_handle,
                self,
                DownloadJobRequest {
                    url: staged.request.url,
                    fallback_urls: staged.request.fallback_urls,
                    audio_url: staged.request.audio_url,
                    source_page_url: staged.request.source_page_url.clone(),
                    http_headers: staged.request.http_headers,
                    save_dir: staged.request.save_dir,
                    file_name: None,
                    enqueue_only: false,
                    expected_checksum: staged.request.expected_checksum,
                    scheduled_at: staged.request.scheduled_at,
                    bandwidth_limit_kbps: staged.request.bandwidth_limit_kbps,
                    format: staged.request.format.clone(),
                    source_title: staged.request.source_title.clone(),
                    force_ytdlp: staged.request.force_ytdlp,
                    stream_manifest: staged.request.stream_manifest,
                },
            )
            .await;

            match result {
                Ok(record) => {
                    acknowledge_staged_browser_request(&staged.path)?;
                    let source_details = source_title
                        .or(source_page_url)
                        .map(|value| format!("Source: {value}"))
                        .unwrap_or_else(|| "Came from the browser.".to_string());
                    send_download_notification(
                        app_handle,
                        "Browser download queued",
                        format!("{} was added to the queue. {source_details}", record.file_name),
                    );
                }
                Err(error) => {
                    let _ = quarantine_staged_browser_request(&staged.path);
                    send_download_notification(
                        app_handle,
                        "Browser download rejected",
                        format!("{download_url} could not be imported: {error}"),
                    );
                }
            }
        }

        Ok(())
    }

    pub fn emit_download_event(
        &self,
        app_handle: &AppHandle,
        id: i64,
        update: DownloadEventUpdate<'_>,
    ) {
        let _ = app_handle.emit(
            DOWNLOAD_STATE_EVENT,
            DownloadStateEvent {
                id,
                status: update.status.to_string(),
                downloaded_bytes: update.downloaded_bytes,
                total_bytes: update.total_bytes,
                speed_bytes_per_second: update.speed_bytes_per_second,
                eta_seconds: update.eta_seconds,
                active_parts: update.active_parts,
                error_message: update.error_message.map(String::from),
            },
        );
    }

    pub fn enqueue_download(
        &self,
        app_handle: &AppHandle,
        job: QueuedDownload,
    ) -> Result<(), String> {
        let mut queue = self.queue.lock().unwrap();
        queue.pending.push_back(job);
        drop(queue);
        self.try_start_next(app_handle);
        Ok(())
    }

    pub fn schedule_pending(&self, app_handle: &AppHandle) -> Result<(), String> {
        let mut queue = self.queue.lock().unwrap();
        let now = Utc::now();

        queue.active.retain(|_, handle| !handle.inner().is_finished());

        let ready: Vec<_> = queue
            .pending
            .iter()
            .filter(|job| {
                job.scheduled_at
                    .as_ref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt <= now)
                    .unwrap_or(true)
            })
            .map(|job| job.id)
            .collect();

        drop(queue);

        if !ready.is_empty() {
            self.try_start_next(app_handle);
        }

        Ok(())
    }

    fn try_start_next(&self, app_handle: &AppHandle) {
        let mut queue = self.queue.lock().unwrap();
        queue.active.retain(|_, handle| !handle.inner().is_finished());

        let max = self.max_concurrent_downloads();
        let now = Utc::now();

        while queue.active.len() < max {
            let next = queue.pending.iter().position(|job| {
                job.scheduled_at
                    .as_ref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt <= now)
                    .unwrap_or(true)
            });

            let Some(index) = next else { break };
            let job = queue.pending.remove(index).unwrap();
            let id = job.id;
            let handle = start_download_task(app_handle.clone(), job, self);
            queue.active.insert(id, handle);
        }
    }

    pub fn pause_download(&self, id: i64) -> Result<(), String> {
        let mut queue = self.queue.lock().unwrap();
        if let Some(handle) = queue.active.remove(&id) {
            handle.abort();
        }
        queue.pending.retain(|job| job.id != id);
        drop(queue);
        let record = self.storage.get_download(id)?;
        self.storage.set_status(
            id,
            "paused",
            record.downloaded_bytes,
            record.total_bytes,
            None,
        )?;
        Ok(())
    }

    pub fn cancel_download(&self, id: i64) -> Result<(), String> {
        let mut queue = self.queue.lock().unwrap();
        if let Some(handle) = queue.active.remove(&id) {
            handle.abort();
        }
        queue.pending.retain(|job| job.id != id);
        drop(queue);
        let record = self.storage.get_download(id)?;
        self.storage.set_status(
            id,
            "cancelled",
            record.downloaded_bytes,
            record.total_bytes,
            None,
        )?;
        Ok(())
    }

    /// Clears one history entry. An in-flight transfer is stopped first so the
    /// queue cannot resurrect the row, then only the database record is
    /// removed. The file already written to disk is intentionally kept.
    pub fn clear_download(&self, id: i64) -> Result<bool, String> {
        {
            let mut queue = self.queue.lock().unwrap();
            if let Some(handle) = queue.active.remove(&id) {
                handle.abort();
            }
            queue.pending.retain(|job| job.id != id);
        }
        self.storage.delete_download(id)
    }

    pub fn active_download_count(&self) -> usize {
        let queue = self.queue.lock().unwrap();
        queue.active.len()
    }

    pub fn pending_download_count(&self) -> usize {
        let queue = self.queue.lock().unwrap();
        queue.pending.len()
    }
}

fn start_download_task(
    app_handle: AppHandle,
    job: QueuedDownload,
    state: &AppState,
) -> JoinHandle<()> {
    let download_service = state.download_service.clone();
    let storage = state.storage.clone_for_task();

    tauri::async_runtime::spawn(async move {
        let result = run_download(&app_handle, &download_service, &storage, job).await;
        if let Err(error) = result {
            eprintln!("download task failed: {error}");
        }
        let app_state = app_handle.state::<AppState>();
        app_state.try_start_next(&app_handle);
    })
}

async fn run_download(
    app_handle: &AppHandle,
    download_service: &DownloadService,
    storage: &Storage,
    job: QueuedDownload,
) -> Result<(), String> {
    let total_hint = job.total_bytes_hint;
    let requested_resume_from = download_service
        .current_downloaded_bytes(&job.target_path)
        .await;

    let speed_tracker = Arc::new(Mutex::new(SpeedTracker::new()));

    let mut on_started = {
        let storage = storage.clone_for_task();
        let app_handle = app_handle.clone();
        let speed_tracker = speed_tracker.clone();
        move |downloaded_bytes: u64, total_bytes: Option<u64>, active_parts: usize| -> Result<(), String> {
            speed_tracker.lock().unwrap().reset(downloaded_bytes);
            storage.set_status(
                job.id,
                "in_progress",
                downloaded_bytes,
                total_bytes.or(total_hint),
                None,
            )?;
            let state = app_handle.state::<AppState>();
            state.emit_download_event(
                &app_handle,
                job.id,
                DownloadEventUpdate {
                    status: "in_progress",
                    downloaded_bytes,
                    total_bytes: total_bytes.or(total_hint),
                    active_parts: Some(active_parts),
                    ..Default::default()
                },
            );
            Ok(())
        }
    };

    let mut on_progress = {
        let storage = storage.clone_for_task();
        let app_handle = app_handle.clone();
        let speed_tracker = speed_tracker.clone();
        move |downloaded_bytes: u64, total_bytes: Option<u64>, active_parts: usize| -> Result<(), String> {
            let (speed_bytes_per_second, eta_seconds) = {
                let mut tracker = speed_tracker.lock().unwrap();
                tracker.update(downloaded_bytes);
                let speed = tracker.speed();
                let eta = total_bytes.and_then(|total| {
                    if speed > 0 && downloaded_bytes < total {
                        Some((total - downloaded_bytes) / speed)
                    } else {
                        None
                    }
                });
                (Some(speed), eta)
            };
            storage.set_status(
                job.id,
                "in_progress",
                downloaded_bytes,
                total_bytes.or(total_hint),
                None,
            )?;
            let state = app_handle.state::<AppState>();
            state.emit_download_event(
                &app_handle,
                job.id,
                DownloadEventUpdate {
                    status: "in_progress",
                    downloaded_bytes,
                    total_bytes: total_bytes.or(total_hint),
                    speed_bytes_per_second,
                    eta_seconds,
                    active_parts: Some(active_parts),
                    error_message: None,
                },
            );
            Ok(())
        }
    };

    let download_result = if let Some(audio_url) = job.audio_url.as_deref() {
        download_service
            .download_media_bundle_to_path(
                &job.url,
                audio_url,
                &job.target_path,
                &mut on_started,
                &mut on_progress,
            )
            .await
    } else {
        download_service
            .download_to_path(
                &job.url,
                &job.target_path,
                DownloadOptions {
                    requested_resume_from,
                    resumable_hint: job.resumable_hint,
                    total_bytes_hint: job.total_bytes_hint,
                    bandwidth_limit_kbps: job.bandwidth_limit_kbps,
                    source_page_url: job.source_page_url.as_deref(),
                    http_headers: &job.http_headers,
                    format: job.format.as_deref(),
                    force_ytdlp: job.force_ytdlp,
                    stream_manifest: job.stream_manifest,
                    fallback_urls: &job.fallback_urls,
                },
                &mut on_started,
                &mut on_progress,
            )
            .await
    };

    match download_result {
        Ok((downloaded_bytes, total_bytes, _active_parts)) => {
            storage.set_status(
                job.id,
                "completed",
                downloaded_bytes,
                total_bytes.or(total_hint),
                None,
            )?;
            let mut checksum_error = None;
            if let Some(expected_checksum) = job.expected_checksum.as_deref() {
                let actual_checksum = download_service.compute_sha256(&job.target_path).await?;
                let checksum_status = if actual_checksum.eq_ignore_ascii_case(expected_checksum) {
                    "verified"
                } else {
                    checksum_error = Some("SHA-256 checksum mismatch".to_string());
                    "mismatch"
                };
                storage.set_checksum_verification(
                    job.id,
                    Some(&actual_checksum),
                    Some(checksum_status),
                    checksum_error.as_deref(),
                )?;
            }

            let state = app_handle.state::<AppState>();
            state.emit_download_event(
                app_handle,
                job.id,
                DownloadEventUpdate {
                    status: "completed",
                    downloaded_bytes,
                    total_bytes: total_bytes.or(total_hint),
                    eta_seconds: Some(0),
                    active_parts: Some(0),
                    error_message: checksum_error.as_deref(),
                    ..Default::default()
                },
            );
            send_download_notification(
                app_handle,
                "Download completed",
                format!("{} has finished downloading.", job.target_path.file_name().unwrap_or_default().to_string_lossy()),
            );
        }
        Err(error) => {
            storage.set_status(job.id, "failed", 0, total_hint, Some(&error))?;
            let state = app_handle.state::<AppState>();
            state.emit_download_event(
                app_handle,
                job.id,
                DownloadEventUpdate {
                    status: "failed",
                    downloaded_bytes: 0,
                    total_bytes: total_hint,
                    active_parts: Some(0),
                    error_message: Some(&error),
                    ..Default::default()
                },
            );
            send_download_notification(
                app_handle,
                "Download failed",
                format!("Download failed: {error}"),
            );
        }
    }

    Ok(())
}

pub fn send_download_notification(app_handle: &AppHandle, title: &str, body: String) {
    // Single central guard: every OS-level download notification goes through
    // this function, so the preference suppresses all of them at once. The
    // check lives here — in the backend, before the notification is actually
    // emitted — rather than hiding anything in the UI. Download behaviour,
    // queueing and in-app status text are untouched by it.
    let enabled = app_handle
        .try_state::<AppState>()
        .map(|state| state.desktop_notifications_enabled())
        .unwrap_or(true);
    if !enabled {
        return;
    }

    let _ = app_handle
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

struct SpeedTracker {
    last_bytes: u64,
    last_time: Instant,
    speed: u64,
}

impl SpeedTracker {
    fn new() -> Self {
        Self {
            last_bytes: 0,
            last_time: Instant::now(),
            speed: 0,
        }
    }

    fn reset(&mut self, bytes: u64) {
        self.last_bytes = bytes;
        self.last_time = Instant::now();
        self.speed = 0;
    }

    fn update(&mut self, bytes: u64) {
        let elapsed = self.last_time.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let delta = bytes.saturating_sub(self.last_bytes);
            self.speed = (delta as f64 / elapsed.as_secs_f64()) as u64;
            self.last_bytes = bytes;
            self.last_time = Instant::now();
        }
    }

    fn speed(&self) -> u64 {
        self.speed
    }
}

#[cfg(test)]
mod pause_tests {
    use super::*;
    use crate::library;
    use crate::storage::NewDownloadRecord;
    // Only the tests use Path; importing it at module scope would warn as an
    // unused import on a normal build.
    use std::path::Path;

    fn test_dir(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".temp_files");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "app-tests-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            name
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn state_at(dir: &Path) -> AppState {
        AppState {
            storage: Storage::open(&dir.join("downloads.sqlite3")).unwrap(),
            download_service: DownloadService::new(Client::new()),
            app_data_dir: dir.to_path_buf(),
            default_download_dir: dir.join("downloads"),
            max_concurrent_downloads: AtomicUsize::new(3),
            default_bandwidth_limit_kbps: AtomicU64::new(0),
            desktop_notifications_enabled: AtomicBool::new(true),
            selected_download_id: Mutex::new(None),
            queue: Mutex::new(DownloadQueueState {
                active: HashMap::new(),
                pending: VecDeque::new(),
            }),
        }
    }

    #[test]
    fn stopping_keeps_progress_partial_file_and_resume_offset() {
        let dir = test_dir("stop");
        let state = state_at(&dir);
        let target = dir.join("big.iso");

        let created = state
            .storage
            .insert_download(NewDownloadRecord {
                url: "https://example.com/big.iso".to_string(),
                file_name: "big.iso".to_string(),
                save_path: target.clone(),
                total_bytes: Some(1000),
                expected_checksum: None,
                scheduled_at: None,
                bandwidth_limit_kbps: None,
                category: library::classify(&target.display().to_string()).to_string(),
            })
            .unwrap();
        state
            .storage
            .set_status(created.id, "in_progress", 260, Some(1000), None)
            .unwrap();

        // Mirrors DownloadService::partial_path_for: the engine resumes from it.
        let partial = target.with_extension("part");
        std::fs::write(&partial, [7_u8; 260]).unwrap();

        state.pause_download(created.id).unwrap();

        let record = state.storage.get_download(created.id).unwrap();
        assert_eq!(record.status, "paused");
        assert_eq!(
            record.downloaded_bytes, 260,
            "Stop must not reset the downloaded byte count"
        );
        assert_eq!(
            record.total_bytes,
            Some(1000),
            "Stop must not drop the total size"
        );
        assert_eq!(
            std::fs::metadata(&partial).unwrap().len(),
            260,
            "Stop must not delete the partial download data"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The preference must default to ON so existing users keep today's
    /// behaviour, and must survive closing and reopening the settings table.
    #[test]
    fn desktop_notifications_default_on_and_persist_across_restart() {
        let dir = test_dir("notify-default");
        let state = state_at(&dir);
        assert!(
            state.desktop_notifications_enabled(),
            "notifications must be enabled by default"
        );

        // Nothing is written until the user changes it.
        assert!(state
            .storage
            .get_setting(DESKTOP_NOTIFICATIONS_SETTING)
            .unwrap()
            .is_none());

        state.set_desktop_notifications_enabled(false);
        assert!(!state.desktop_notifications_enabled());
        assert_eq!(
            state
                .storage
                .get_setting(DESKTOP_NOTIFICATIONS_SETTING)
                .unwrap()
                .as_deref(),
            Some("0")
        );

        // Reopen the same database: this is what a restart does.
        let reopened = Storage::open(&dir.join("downloads.sqlite3")).unwrap();
        assert_eq!(
            reopened
                .get_setting(DESKTOP_NOTIFICATIONS_SETTING)
                .unwrap()
                .as_deref(),
            Some("0"),
            "a disabled preference must survive a restart"
        );

        state.set_desktop_notifications_enabled(true);
        assert_eq!(
            state
                .storage
                .get_setting(DESKTOP_NOTIFICATIONS_SETTING)
                .unwrap()
                .as_deref(),
            Some("1"),
            "re-enabling must take effect immediately and persist"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end guard for the *combined* path that `queue_download_request`
    /// uses. `resolve_target_dir` already appends `LDM/`, and
    /// `ensure_category_dir` appends it again — when the first one returned the
    /// library root instead of the base, real downloads landed in
    /// `Downloads/LDM/LDM/<Category>`. Testing `classify()` and
    /// `category_dir()` in isolation cannot catch that, so this walks the real
    /// chain and asserts the `LDM` component appears exactly once.
    #[test]
    fn the_resolve_and_ensure_chain_never_nests_ldm_inside_ldm() {
        let dir = test_dir("chain");
        let state = state_at(&dir);

        let base = state.resolve_target_dir(None).unwrap();
        assert_eq!(
            base,
            dir.join("downloads"),
            "resolve_target_dir must hand back the base, not the library root"
        );

        for (name, folder) in [
            ("test.jpg", "Images"),
            ("song.mp3", "Music"),
            ("movie.mkv", "Videos"),
            ("program.AppImage", "Applications"),
            ("document.pdf", "Documents"),
            ("archive.7z", "Archives"),
            ("ubuntu.iso", "ISO"),
            ("unknown.xyz", "Other"),
            ("IMAGE.JPG", "Images"),
            ("VIDEO.MKV", "Videos"),
            ("LINUX.ISO", "ISO"),
        ] {
            let category = library::classify(name);
            let target_dir = library::ensure_category_dir(&base, category).unwrap();
            let target = state
                .download_service
                .reserve_target_path(&target_dir, name);

            let relative = target
                .strip_prefix(&base)
                .unwrap_or_else(|_| panic!("{target:?} escaped the download base {base:?}"));

            let ldm_count = relative
                .components()
                .filter(|component| component.as_os_str() == library::LIBRARY_DIR)
                .count();
            assert_eq!(
                ldm_count,
                1,
                "{name} produced a nested library: {relative:?}"
            );
            assert_eq!(
                relative.components().next().unwrap().as_os_str(),
                library::LIBRARY_DIR,
                "{name} must start at the library root: {relative:?}"
            );
            assert_eq!(
                relative.parent().unwrap().file_name().unwrap(),
                folder,
                "{name} landed in the wrong category folder: {relative:?}"
            );
            assert_eq!(relative.file_name().unwrap(), name);
            assert!(
                target_dir.is_dir(),
                "{name} folder must exist before the engine writes: {target_dir:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A custom base directory must keep working: the library is nested under
    /// it exactly once, and a base that already points at `LDM/` is reused
    /// rather than doubled.
    #[test]
    fn a_custom_base_directory_is_nested_once_too() {
        let dir = test_dir("chain-custom");
        let state = state_at(&dir);

        let custom = dir.join("my-files");
        let base = state
            .resolve_target_dir(Some(custom.to_str().unwrap()))
            .unwrap();
        assert_eq!(base, custom);

        let target_dir =
            library::ensure_category_dir(&base, library::classify("ubuntu.iso")).unwrap();
        assert_eq!(target_dir, custom.join("LDM").join("ISO"));
        assert!(target_dir.is_dir());
        assert!(!custom.join("LDM").join("LDM").exists());

        // Pointing the base straight at the library must not double it either.
        let library_base = state
            .resolve_target_dir(Some(target_dir.to_str().unwrap()))
            .unwrap();
        assert_eq!(library_base, target_dir);
        assert!(library::ensure_library(&library_base).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

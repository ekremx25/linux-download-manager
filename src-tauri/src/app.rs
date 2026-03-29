use crate::browser::{
    acknowledge_staged_browser_request, load_staged_browser_requests,
    quarantine_staged_browser_request,
};
use crate::download::DownloadService;
use crate::jobs::{DownloadJobRequest, queue_download_request};
use crate::storage::Storage;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::platform::{resolve_app_data_dir, resolve_default_download_dir};

pub const DOWNLOAD_STATE_EVENT: &str = "download://state";

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

struct DownloadQueueState {
    active: HashMap<i64, JoinHandle<()>>,
    pending: VecDeque<QueuedDownload>,
}

#[derive(Clone)]
pub struct QueuedDownload {
    pub id: i64,
    pub url: String,
    pub audio_url: Option<String>,
    pub source_page_url: Option<String>,
    pub format: Option<String>,
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
            queue: Mutex::new(DownloadQueueState {
                active: HashMap::new(),
                pending: VecDeque::new(),
            }),
        })
    }

    pub fn resolve_target_dir(&self, requested: Option<&str>) -> Result<PathBuf, String> {
        match requested {
            Some(path) if !path.is_empty() => {
                let path = PathBuf::from(path);
                if path.is_relative() {
                    Ok(self.default_download_dir.join(path))
                } else {
                    Ok(path)
                }
            }
            _ => Ok(self.default_download_dir.clone()),
        }
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
                &record.status,
                record.downloaded_bytes,
                record.total_bytes,
                None,
                None,
                Some(0),
                record.error_message.as_deref(),
            );
            self.enqueue_download(
                app_handle,
                QueuedDownload {
                    id: record.id,
                    url: record.url,
                    target_path: PathBuf::from(record.save_path),
                    audio_url: None,
                    source_page_url: None,
                    format: None,
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
                    format!("Tarayici inbox kuyruğu okunamadi: {error}"),
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
                    audio_url: staged.request.audio_url,
                    source_page_url: staged.request.source_page_url.clone(),
                    save_dir: staged.request.save_dir,
                    expected_checksum: staged.request.expected_checksum,
                    scheduled_at: staged.request.scheduled_at,
                    bandwidth_limit_kbps: staged.request.bandwidth_limit_kbps,
                    format: staged.request.format.clone(),
                    source_title: staged.request.source_title.clone(),
                },
            )
            .await;

            match result {
                Ok(record) => {
                    acknowledge_staged_browser_request(&staged.path)?;
                    let source_details = source_title
                        .or(source_page_url)
                        .map(|value| format!("Kaynak: {value}"))
                        .unwrap_or_else(|| "Tarayicidan geldi.".to_string());
                    send_download_notification(
                        app_handle,
                        "Browser download queued",
                        format!("{} kuyruga eklendi. {source_details}", record.file_name),
                    );
                }
                Err(error) => {
                    let _ = quarantine_staged_browser_request(&staged.path);
                    send_download_notification(
                        app_handle,
                        "Browser download rejected",
                        format!("{download_url} iceri aktarılamadi: {error}"),
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
        status: &str,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        speed_bytes_per_second: Option<u64>,
        eta_seconds: Option<u64>,
        active_parts: Option<usize>,
        error_message: Option<&str>,
    ) {
        let _ = app_handle.emit(
            DOWNLOAD_STATE_EVENT,
            DownloadStateEvent {
                id,
                status: status.to_string(),
                downloaded_bytes,
                total_bytes,
                speed_bytes_per_second,
                eta_seconds,
                active_parts,
                error_message: error_message.map(String::from),
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
        self.storage.set_status(id, "paused", 0, None, None)?;
        Ok(())
    }

    pub fn cancel_download(&self, id: i64) -> Result<(), String> {
        let mut queue = self.queue.lock().unwrap();
        if let Some(handle) = queue.active.remove(&id) {
            handle.abort();
        }
        queue.pending.retain(|job| job.id != id);
        drop(queue);
        self.storage.set_status(id, "cancelled", 0, None, None)?;
        Ok(())
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
                "in_progress",
                downloaded_bytes,
                total_bytes.or(total_hint),
                None,
                None,
                Some(active_parts),
                None,
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
                "in_progress",
                downloaded_bytes,
                total_bytes.or(total_hint),
                speed_bytes_per_second,
                eta_seconds,
                Some(active_parts),
                None,
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
                requested_resume_from,
                job.resumable_hint,
                job.total_bytes_hint,
                job.bandwidth_limit_kbps,
                job.source_page_url.as_deref(),
                job.format.as_deref(),
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
                "completed",
                downloaded_bytes,
                total_bytes.or(total_hint),
                None,
                Some(0),
                Some(0),
                checksum_error.as_deref(),
            );
            send_download_notification(
                app_handle,
                "Download completed",
                format!("{} indirme tamamlandi.", job.target_path.file_name().unwrap_or_default().to_string_lossy()),
            );
        }
        Err(error) => {
            storage.set_status(job.id, "failed", 0, total_hint, Some(&error))?;
            let state = app_handle.state::<AppState>();
            state.emit_download_event(
                app_handle,
                job.id,
                "failed",
                0,
                total_hint,
                None,
                None,
                Some(0),
                Some(&error),
            );
            send_download_notification(
                app_handle,
                "Download failed",
                format!("Indirme basarisiz: {error}"),
            );
        }
    }

    Ok(())
}

pub fn send_download_notification(app_handle: &AppHandle, title: &str, body: String) {
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

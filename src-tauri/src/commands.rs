use crate::app::{AppState, QueuedDownload};
use crate::download::{DownloadMetadata, DownloadRecord};
use crate::jobs::{DownloadJobRequest, queue_download_request};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_download_dir: String,
    pub max_concurrent_downloads: usize,
    pub default_bandwidth_limit_kbps: u64,
}

#[tauri::command]
pub async fn inspect_url(
    state: State<'_, AppState>,
    url: String,
) -> Result<DownloadMetadata, String> {
    state.download_service.inspect_url(&url).await
}

#[tauri::command]
pub async fn list_downloads(state: State<'_, AppState>) -> Result<Vec<DownloadRecord>, String> {
    let mut list = state.storage.list_downloads()?;
    let live = state.live_metrics.lock().unwrap();
    for item in &mut list {
        if item.status == "in_progress" {
            if let Some(&(speed, eta)) = live.get(&item.id) {
                item.speed_bytes_per_second = Some(speed);
                item.eta_seconds = eta;
            }
        }
    }
    Ok(list)
}

#[tauri::command]
pub async fn start_download(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    url: String,
    save_dir: Option<String>,
    expected_checksum: Option<String>,
    scheduled_at: Option<String>,
    bandwidth_limit_kbps: Option<u64>,
    custom_file_name: Option<String>,
) -> Result<DownloadRecord, String> {
    queue_download_request(
        &app_handle,
        &state,
        DownloadJobRequest {
            url,
            fallback_urls: Vec::new(),
            audio_url: None,
            source_page_url: None,
            http_headers: Default::default(),
            save_dir,
            expected_checksum,
            scheduled_at,
            bandwidth_limit_kbps,
            format: None,
            source_title: custom_file_name,
            force_ytdlp: false,
            stream_manifest: false,
        },
    )
    .await
}

#[tauri::command]
pub async fn pick_save_directory(
    app_handle: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let dialog = tauri_plugin_dialog::DialogExt::dialog(&app_handle);
    let result = dialog.file().blocking_pick_folder();
    Ok(result.map(|path| path.to_string()))
}

#[tauri::command]
pub async fn app_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    Ok(AppSettings {
        default_download_dir: state.default_download_dir.display().to_string(),
        max_concurrent_downloads: state.max_concurrent_downloads(),
        default_bandwidth_limit_kbps: state.default_bandwidth_limit_kbps().unwrap_or(0),
    })
}

#[tauri::command]
pub async fn update_app_settings(
    state: State<'_, AppState>,
    max_concurrent_downloads: Option<usize>,
    default_bandwidth_limit_kbps: Option<u64>,
) -> Result<AppSettings, String> {
    if let Some(value) = max_concurrent_downloads {
        if value == 0 || value > 10 {
            return Err("max concurrent downloads must be between 1 and 10".to_string());
        }
        state.set_max_concurrent_downloads(value);
    }

    if let Some(value) = default_bandwidth_limit_kbps {
        state.set_default_bandwidth_limit_kbps(if value == 0 { None } else { Some(value) });
    }

    Ok(AppSettings {
        default_download_dir: state.default_download_dir.display().to_string(),
        max_concurrent_downloads: state.max_concurrent_downloads(),
        default_bandwidth_limit_kbps: state.default_bandwidth_limit_kbps().unwrap_or(0),
    })
}

#[tauri::command]
pub async fn pause_download(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    state.pause_download(id)?;
    state.emit_download_event(&app_handle, id, "paused", 0, None, None, None, Some(0), None);
    Ok(())
}

#[tauri::command]
pub async fn resume_download(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<DownloadRecord, String> {
    let record = state.storage.get_download(id)?;
    let metadata = state.download_service.inspect_url(&record.url).await?;

    state.storage.set_status(
        id,
        "queued",
        record.downloaded_bytes,
        record.total_bytes.or(metadata.content_length),
        None,
    )?;
    state.enqueue_download(
        &app_handle,
        QueuedDownload {
            id,
            url: record.url,
            fallback_urls: Vec::new(),
            audio_url: None,
            source_page_url: None,
            http_headers: Default::default(),
            format: None,
            force_ytdlp: false,
            stream_manifest: false,
            target_path: std::path::Path::new(&record.save_path).to_path_buf(),
            resumable_hint: metadata.resumable,
            total_bytes_hint: record.total_bytes.or(metadata.content_length),
            expected_checksum: record.expected_checksum,
            scheduled_at: record.scheduled_at,
            bandwidth_limit_kbps: record.bandwidth_limit_kbps,
        },
    )?;

    state.storage.get_download(id)
}

#[tauri::command]
pub async fn cancel_download(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    state.cancel_download(id)?;
    let record = state.storage.get_download(id)?;
    let _ = state
        .download_service
        .remove_temp_artifacts(std::path::Path::new(&record.save_path))
        .await;
    state.emit_download_event(&app_handle, id, "cancelled", 0, None, None, None, Some(0), None);
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub active_downloads: usize,
    pub pending_downloads: usize,
    pub default_download_dir: String,
}

#[tauri::command]
pub async fn clear_completed(state: State<'_, AppState>) -> Result<u64, String> {
    state.storage.delete_completed()
}

#[tauri::command]
pub async fn system_status(state: State<'_, AppState>) -> Result<SystemStatus, String> {
    Ok(SystemStatus {
        active_downloads: state.active_download_count(),
        pending_downloads: state.pending_download_count(),
        default_download_dir: state.default_download_dir.display().to_string(),
    })
}

#[tauri::command]
pub async fn delete_download(
    state: State<'_, AppState>,
    id: i64,
    delete_file: bool,
) -> Result<(), String> {
    let _ = state.cancel_download(id);
    if let Ok(record) = state.storage.get_download(id) {
        let path = std::path::PathBuf::from(&record.save_path);
        let _ = state.download_service.remove_temp_artifacts(&path).await;
        if delete_file && path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
    state.storage.delete_download(id)?;
    Ok(())
}

#[tauri::command]
pub async fn open_file(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("File does not exist: {path}"));
    }
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Failed to open file: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn open_folder(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    let target = if p.is_dir() {
        p.to_path_buf()
    } else {
        p.parent().map(|parent| parent.to_path_buf()).unwrap_or_else(|| p.to_path_buf())
    };
    if !target.exists() {
        return Err(format!("Folder does not exist: {}", target.display()));
    }
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map_err(|e| format!("Failed to open folder: {e}"))?;
    Ok(())
}


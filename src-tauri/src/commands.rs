use crate::app::{AppState, DownloadEventUpdate};
use crate::download::{DownloadMetadata, DownloadRecord, DownloadSegmentDetail};
use crate::jobs::{DownloadJobRequest, queue_download_request};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_download_dir: String,
    pub max_concurrent_downloads: usize,
    pub default_bandwidth_limit_kbps: u64,
    pub desktop_notifications_enabled: bool,
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
    state.storage.list_downloads()
}

// The parameter list below IS the IPC contract: Tauri deserializes each
// argument by name from the object `ui/add-download.js` passes to
// `invoke('start_download', {...})`. Grouping these into a struct would break
// every existing frontend call, so the wide signature is deliberate. The body
// immediately packs them into the `DownloadJobRequest` struct, which is the
// form the rest of the codebase passes around.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_download(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    url: String,
    save_dir: Option<String>,
    file_name: Option<String>,
    enqueue_only: Option<bool>,
    expected_checksum: Option<String>,
    scheduled_at: Option<String>,
    bandwidth_limit_kbps: Option<u64>,
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
            file_name,
            enqueue_only: enqueue_only.unwrap_or(false),
            expected_checksum,
            scheduled_at,
            bandwidth_limit_kbps,
            format: None,
            source_title: None,
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
        desktop_notifications_enabled: state.desktop_notifications_enabled(),
    })
}

#[tauri::command]
pub async fn update_app_settings(
    state: State<'_, AppState>,
    max_concurrent_downloads: Option<usize>,
    default_bandwidth_limit_kbps: Option<u64>,
    desktop_notifications_enabled: Option<bool>,
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

    // Applied immediately and persisted, so no restart is needed.
    if let Some(value) = desktop_notifications_enabled {
        state.set_desktop_notifications_enabled(value);
    }

    Ok(AppSettings {
        default_download_dir: state.default_download_dir.display().to_string(),
        max_concurrent_downloads: state.max_concurrent_downloads(),
        default_bandwidth_limit_kbps: state.default_bandwidth_limit_kbps().unwrap_or(0),
        desktop_notifications_enabled: state.desktop_notifications_enabled(),
    })
}

#[tauri::command]
pub async fn pause_download(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    state.pause_download(id).await?;
    // The list merges this event over its stored row, so it has to carry the
    // progress `pause_download` just persisted. Emitting zeros here wiped the
    // visible percentage back to 0% and made a resume look like a fresh start.
    let record = state.storage.get_download(id)?;
    state.emit_download_event(
        &app_handle,
        id,
        DownloadEventUpdate {
            status: "paused",
            downloaded_bytes: record.downloaded_bytes,
            total_bytes: record.total_bytes,
            active_parts: Some(0),
            ..Default::default()
        },
    );
    Ok(())
}

#[tauri::command]
pub async fn resume_download(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<DownloadRecord, String> {
    let record = state.storage.get_download(id)?;
    if !matches!(record.status.as_str(), "paused" | "failed" | "cancelled") {
        return Err("Only paused or failed downloads can be resumed".to_string());
    }
    let mut job = state.resume_job(&record)?;
    // Explicit Resume should start now, not wait for an old schedule.
    job.scheduled_at = None;
    state.storage.set_status(
        id,
        "queued",
        record.downloaded_bytes,
        job.total_bytes_hint,
        None,
    )?;
    state.enqueue_download(&app_handle, job)?;

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
    state.emit_download_event(
        &app_handle,
        id,
        DownloadEventUpdate {
            status: "cancelled",
            downloaded_bytes: record.downloaded_bytes,
            total_bytes: record.total_bytes,
            active_parts: Some(0),
            ..Default::default()
        },
    );
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadDetail {
    pub download: DownloadRecord,
    pub resume_supported: bool,
    pub segments: Vec<DownloadSegmentDetail>,
}

fn show_window(app_handle: &tauri::AppHandle, label: &str) -> Result<(), String> {
    let window = app_handle
        .get_webview_window(label)
        .ok_or_else(|| format!("window not found: {label}"))?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.center().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn show_add_download_window(app_handle: tauri::AppHandle) -> Result<(), String> {
    show_window(&app_handle, "add-download")
}

#[tauri::command]
pub fn show_download_detail_window(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    state.storage.get_download(id)?;
    state.select_download(id);
    show_window(&app_handle, "download-detail")?;
    app_handle
        .emit_to("download-detail", "download://selected", id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn hide_app_window(app_handle: tauri::AppHandle, label: String) -> Result<(), String> {
    if !matches!(label.as_str(), "add-download" | "download-detail") {
        return Err("window cannot be hidden by this command".to_string());
    }
    let window = app_handle
        .get_webview_window(&label)
        .ok_or_else(|| format!("window not found: {label}"))?;
    window.hide().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn download_detail(
    state: State<'_, AppState>,
    id: Option<i64>,
) -> Result<DownloadDetail, String> {
    let id = id.or_else(|| state.selected_download_id())
        .ok_or_else(|| "no download selected".to_string())?;
    let download = state.storage.get_download(id)?;
    let segments = state.download_service.segment_details(
        std::path::Path::new(&download.save_path),
        &download.status,
        download.downloaded_bytes,
        download.total_bytes,
    ).await?;
    let resume_supported = download.downloaded_bytes > 0
        || segments.len() > 1
        || matches!(download.status.as_str(), "paused" | "in_progress" | "queued");
    Ok(DownloadDetail { download, resume_supported, segments })
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

/// Clears a single download from the list/history. The file on disk is kept.
#[tauri::command]
pub async fn clear_download(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    state.clear_download(id)
}

/// Explicit destructive action, called only after the UI confirmation.
#[tauri::command]
pub async fn delete_download_files(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    let record = state.storage.get_download(id)?;
    if matches!(
        record.status.as_str(),
        "in_progress" | "queued" | "scheduled"
    ) {
        return Err("Pause the download before deleting its files".to_string());
    }
    state
        .download_service
        .delete_download_files(std::path::Path::new(&record.save_path))
        .await?;
    state.clear_download(id)
}

#[tauri::command]
pub async fn system_status(state: State<'_, AppState>) -> Result<SystemStatus, String> {
    Ok(SystemStatus {
        active_downloads: state.active_download_count(),
        pending_downloads: state.pending_download_count(),
        default_download_dir: state.default_download_dir.display().to_string(),
    })
}

/// Reveal a record's containing folder; never execute the downloaded file.
#[tauri::command]
pub fn open_download_folder(state: State<'_, AppState>, id: Option<i64>) -> Result<(), String> {
    let folder = if let Some(id) = id {
        let record = state.storage.get_download(id)?;
        std::path::Path::new(&record.save_path).parent()
            .ok_or("The save folder could not be found")?.to_path_buf()
    } else { state.default_download_dir.clone() };
    if !folder.is_dir() { return Err("The save folder does not exist or has been moved.".into()); }
    std::process::Command::new("xdg-open").arg(&folder)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().map_err(|e| format!("Could not open folder: {e}"))?;
    Ok(())
}

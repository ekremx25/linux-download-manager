use crate::app::{AppState, QueuedDownload};
use crate::download::DownloadRecord;
use crate::storage::NewDownloadRecord;
use chrono::{Local, LocalResult, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJobRequest {
    pub url: String,
    pub audio_url: Option<String>,
    pub source_page_url: Option<String>,
    pub save_dir: Option<String>,
    pub expected_checksum: Option<String>,
    pub scheduled_at: Option<String>,
    pub bandwidth_limit_kbps: Option<u64>,
    pub format: Option<String>,
    pub source_title: Option<String>,
}

pub async fn queue_download_request(
    app_handle: &tauri::AppHandle,
    state: &AppState,
    request: DownloadJobRequest,
) -> Result<DownloadRecord, String> {
    let metadata = state.download_service.inspect_url(&request.url).await?;
    let target_dir = state.resolve_target_dir(request.save_dir.as_deref())?;
    let expected_checksum = normalize_checksum(request.expected_checksum)?;
    let scheduled_at = normalize_schedule_input(request.scheduled_at)?;
    let bandwidth_limit_kbps = normalize_bandwidth_limit(
        request
            .bandwidth_limit_kbps
            .or(state.default_bandwidth_limit_kbps()),
    )?;
    let file_name = match request.source_title.as_deref() {
        Some(title) if !title.is_empty() && title.len() > 3 => {
            let clean: String = title
                .chars()
                .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
                .collect();
            let ext = if metadata.suggested_file_name.contains('.') {
                metadata.suggested_file_name.rsplit('.').next().unwrap_or("mp4")
            } else {
                "mp4"
            };
            format!("{clean}.{ext}")
        }
        _ => metadata.suggested_file_name.clone(),
    };
    let target_path = state
        .download_service
        .reserve_target_path(&target_dir, &file_name);

    let created = state.storage.insert_download(NewDownloadRecord {
        url: request.url.clone(),
        file_name: target_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| metadata.suggested_file_name.clone()),
        save_path: target_path.clone(),
        total_bytes: metadata.content_length,
        expected_checksum: expected_checksum.clone(),
        scheduled_at: scheduled_at.clone(),
        bandwidth_limit_kbps,
    })?;

    let initial_status = if scheduled_at.is_some() {
        "scheduled"
    } else {
        "queued"
    };
    state
        .storage
        .set_status(created.id, initial_status, 0, metadata.content_length, None)?;
    state.emit_download_event(
        app_handle,
        created.id,
        initial_status,
        0,
        metadata.content_length,
        None,
        None,
        Some(0),
        None,
    );
    state.enqueue_download(
        app_handle,
        QueuedDownload {
            id: created.id,
            url: request.url,
            audio_url: request.audio_url,
            source_page_url: request.source_page_url,
            format: request.format,
            target_path,
            resumable_hint: metadata.resumable,
            total_bytes_hint: metadata.content_length,
            expected_checksum,
            scheduled_at,
            bandwidth_limit_kbps,
        },
    )?;

    state.storage.get_download(created.id)
}

pub fn normalize_checksum(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };

    let cleaned = value.trim().to_ascii_lowercase();
    if cleaned.is_empty() {
        return Ok(None);
    }

    if cleaned.len() != 64
        || !cleaned
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("SHA-256 checksum must be a 64-character hexadecimal string".to_string());
    }

    Ok(Some(cleaned))
}

pub fn normalize_schedule_input(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };

    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Ok(None);
    }

    if let Ok(datetime) = chrono::DateTime::parse_from_rfc3339(cleaned) {
        return Ok(Some(datetime.with_timezone(&Utc).to_rfc3339()));
    }

    let naive = NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|_| "scheduled time must be a valid local date/time".to_string())?;

    let local = match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, _) => first,
        LocalResult::None => {
            return Err("scheduled time is invalid in the current local timezone".to_string());
        }
    };

    Ok(Some(local.with_timezone(&Utc).to_rfc3339()))
}

pub fn normalize_bandwidth_limit(value: Option<u64>) -> Result<Option<u64>, String> {
    match value {
        Some(0) => Ok(None),
        Some(value) if value > 50_000 => Err("bandwidth limit is unrealistically high".to_string()),
        other => Ok(other),
    }
}

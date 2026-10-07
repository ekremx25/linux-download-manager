use crate::app::{AppState, DownloadEventUpdate, QueuedDownload};
use crate::download::DownloadRecord;
use crate::library;
use crate::storage::NewDownloadRecord;
use chrono::{Local, LocalResult, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJobRequest {
    pub url: String,
    #[serde(default)]
    pub fallback_urls: Vec<String>,
    pub audio_url: Option<String>,
    pub source_page_url: Option<String>,
    #[serde(default)]
    pub http_headers: HashMap<String, String>,
    pub save_dir: Option<String>,
    pub file_name: Option<String>,
    #[serde(default)]
    pub enqueue_only: bool,
    pub expected_checksum: Option<String>,
    pub scheduled_at: Option<String>,
    pub bandwidth_limit_kbps: Option<u64>,
    pub format: Option<String>,
    pub source_title: Option<String>,
    #[serde(default)]
    pub force_ytdlp: bool,
    #[serde(default)]
    pub stream_manifest: bool,
}

pub async fn queue_download_request(
    app_handle: &tauri::AppHandle,
    state: &AppState,
    request: DownloadJobRequest,
) -> Result<DownloadRecord, String> {
    let metadata = if request.force_ytdlp || request.stream_manifest {
        crate::download::DownloadMetadata {
            source_url: request.url.clone(),
            suggested_file_name: "video.mp4".to_string(),
            content_length: None,
            content_type: Some("video/mp4".to_string()),
            resumable: false,
        }
    } else {
        state.download_service.inspect_url(&request.url).await?
    };
    let target_dir = state.resolve_target_dir(request.save_dir.as_deref())?;
    let expected_checksum = normalize_checksum(request.expected_checksum)?;
    let scheduled_at = normalize_schedule_input(request.scheduled_at)?;
    let bandwidth_limit_kbps = normalize_bandwidth_limit(
        request
            .bandwidth_limit_kbps
            .or(state.default_bandwidth_limit_kbps()),
    )?;
    let file_name = derive_file_name(
        request.source_title.as_deref(),
        &metadata.suggested_file_name,
    );
    let file_name = request
        .file_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(|name| library::sanitize_file_name(name, "download.bin"))
        .unwrap_or(file_name);

    // The category comes from the real file name, never the query string, and
    // the folder is created before the engine opens a single file handle.
    let category = library::classify(&file_name);
    let category_dir = library::ensure_category_dir(&target_dir, category)?;
    let target_path = state
        .download_service
        .reserve_target_path(&category_dir, &file_name);

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
        category: category.to_string(),
    })?;

    let job = QueuedDownload {
        id: created.id,
        url: request.url,
        fallback_urls: request.fallback_urls,
        audio_url: request.audio_url,
        source_page_url: request.source_page_url,
        http_headers: request.http_headers,
        format: request.format,
        force_ytdlp: request.force_ytdlp,
        stream_manifest: request.stream_manifest,
        target_path,
        resumable_hint: metadata.resumable,
        total_bytes_hint: metadata.content_length,
        expected_checksum,
        scheduled_at,
        bandwidth_limit_kbps,
    };
    state.storage.save_resume_job(&job)?;
    // Held jobs are persisted as paused so the scheduler and restart cannot start them.
    let initial_status = if request.enqueue_only {
        "paused"
    } else if job.scheduled_at.is_some() {
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
        DownloadEventUpdate {
            status: initial_status,
            total_bytes: metadata.content_length,
            active_parts: Some(0),
            ..Default::default()
        },
    );
    if request.enqueue_only {
        return state.storage.get_download(created.id);
    }
    state.enqueue_download(app_handle, job)?;

    state.storage.get_download(created.id)
}

/// Returns the trailing extension when it looks like a real file extension:
/// an alphanumeric suffix of 1..=8 characters after the final dot.
fn file_extension(value: &str) -> Option<&str> {
    let (stem, suffix) = value.rsplit_once('.')?;
    if stem.is_empty() || suffix.is_empty() || suffix.len() > 8 {
        return None;
    }
    suffix
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
        .then_some(suffix)
}

fn detect_extension(suggested_file_name: &str) -> &str {
    file_extension(suggested_file_name).unwrap_or("mp4")
}

/// Derives the on-disk name from the source page title and the server-suggested
/// name. A title that already carries an extension keeps it: appending the
/// detected extension whenever the two differed produced double extensions
/// such as `speed-observation.bin.dat`.
fn derive_file_name(source_title: Option<&str>, suggested_file_name: &str) -> String {
    match source_title
        .map(str::trim)
        .filter(|title| !title.is_empty() && title.len() > 3)
    {
        Some(title) => {
            let clean = library::sanitize_file_name(title, "video");
            if file_extension(&clean).is_some() {
                clean
            } else {
                format!("{clean}.{}", detect_extension(suggested_file_name))
            }
        }
        None => library::sanitize_file_name(suggested_file_name, "download.bin"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_requests_keep_automatic_start_and_detected_name() {
        let request: DownloadJobRequest = serde_json::from_str(r#"{"url":"https://example.com/file"}"#).unwrap();
        assert!(!request.enqueue_only);
        assert!(request.file_name.is_none());
        assert!(request.fallback_urls.is_empty());
        assert!(request.http_headers.is_empty());
    }

    #[test]
    fn explicit_add_options_deserialize() {
        let request: DownloadJobRequest = serde_json::from_str(r#"{"url":"https://example.com/file","fileName":"chosen.zip","enqueueOnly":true}"#).unwrap();
        assert!(request.enqueue_only);
        assert_eq!(request.file_name.as_deref(), Some("chosen.zip"));
    }

    #[test]
    fn filenames_cannot_escape_target_directory() {
        assert_eq!(library::sanitize_file_name("../escape\\name", "download.bin"), "_escape_name");
        assert_eq!(library::sanitize_file_name("..", "download.bin"), "download.bin");
        assert_eq!(library::sanitize_file_name("report\n.txt", "download.bin"), "report_.txt");
        assert_eq!(library::sanitize_file_name(" chosen.zip ", "download.bin"), "chosen.zip");
    }

    #[test]
    fn only_a_real_extension_suffix_counts() {
        assert_eq!(file_extension("archive.tar.gz"), Some("gz"));
        assert_eq!(file_extension("story 1.5"), Some("5"));
        assert_eq!(file_extension("name.toolongextension"), None);
        assert_eq!(file_extension(".hidden"), None);
        assert_eq!(file_extension("trailing."), None);
        assert_eq!(file_extension("no-extension"), None);
    }

    #[test]
    fn a_title_extension_is_kept_instead_of_gaining_a_second_one() {
        // Regression: the title only suppressed a second extension when it
        // matched the server-suggested one, so any other pairing doubled up.
        assert_eq!(
            derive_file_name(Some("speed-observation.bin"), "1Mb.dat"),
            "speed-observation.bin"
        );
        assert_eq!(
            derive_file_name(Some("mergetest.bin"), "mergetest.bin.dat"),
            "mergetest.bin"
        );
    }

    #[test]
    fn a_title_without_an_extension_still_receives_the_detected_one() {
        assert_eq!(
            derive_file_name(Some("Ubuntu 26.04 LTS"), "ubuntu.iso"),
            "Ubuntu 26.04 LTS.iso"
        );
        assert_eq!(derive_file_name(Some("Ep 1"), "video.mp4"), "Ep 1.mp4");
        assert_eq!(derive_file_name(Some("Some Clip"), "no-extension"), "Some Clip.mp4");
    }

    #[test]
    fn without_a_title_the_suggested_name_is_used_verbatim() {
        assert_eq!(derive_file_name(None, "ubuntu.iso"), "ubuntu.iso");
        assert_eq!(derive_file_name(Some(""), "ubuntu.iso"), "ubuntu.iso");
        assert_eq!(derive_file_name(Some("ab"), "ubuntu.iso"), "ubuntu.iso");
    }
}

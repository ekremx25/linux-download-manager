use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const INBOX_DIR_NAME: &str = "browser-inbox";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDownloadRequest {
    pub url: String,
    pub audio_url: Option<String>,
    pub save_dir: Option<String>,
    pub expected_checksum: Option<String>,
    pub scheduled_at: Option<String>,
    pub bandwidth_limit_kbps: Option<u64>,
    pub source_page_url: Option<String>,
    pub source_title: Option<String>,
    pub format: Option<String>,
    pub created_at: String,
}

pub struct StagedBrowserRequest {
    pub path: PathBuf,
    pub request: BrowserDownloadRequest,
}

pub fn new_browser_download_request(
    url: String,
    audio_url: Option<String>,
    save_dir: Option<String>,
    expected_checksum: Option<String>,
    scheduled_at: Option<String>,
    bandwidth_limit_kbps: Option<u64>,
    source_page_url: Option<String>,
    source_title: Option<String>,
    format: Option<String>,
) -> BrowserDownloadRequest {
    BrowserDownloadRequest {
        url,
        audio_url,
        save_dir,
        expected_checksum,
        scheduled_at,
        bandwidth_limit_kbps,
        source_page_url,
        source_title,
        format,
        created_at: Utc::now().to_rfc3339(),
    }
}

pub fn stage_browser_request(
    app_data_dir: &Path,
    request: &BrowserDownloadRequest,
) -> Result<PathBuf, String> {
    let inbox_dir = browser_inbox_dir(app_data_dir);
    fs::create_dir_all(&inbox_dir)
        .map_err(|error| format!("failed to create browser inbox directory: {error}"))?;

    let file_stem = format!(
        "{}-{}",
        Utc::now().format("%Y%m%d%H%M%S%.6f"),
        std::process::id()
    );
    let temp_path = inbox_dir.join(format!("{file_stem}.tmp"));
    let final_path = inbox_dir.join(format!("{file_stem}.json"));
    let payload = serde_json::to_vec_pretty(request)
        .map_err(|error| format!("invalid inbox payload: {error}"))?;

    fs::write(&temp_path, payload)
        .map_err(|error| format!("failed to write temporary browser inbox file: {error}"))?;
    fs::rename(&temp_path, &final_path)
        .map_err(|error| format!("failed to finalize browser inbox file: {error}"))?;

    Ok(final_path)
}

pub fn load_staged_browser_requests(
    app_data_dir: &Path,
) -> Result<Vec<StagedBrowserRequest>, String> {
    let inbox_dir = browser_inbox_dir(app_data_dir);
    if !inbox_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(&inbox_dir)
        .map_err(|error| format!("failed to read browser inbox directory: {error}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    entries.sort();

    let mut staged = Vec::with_capacity(entries.len());
    for path in entries {
        let payload = fs::read_to_string(&path).map_err(|error| {
            format!(
                "failed to read browser inbox file {}: {error}",
                path.display()
            )
        })?;
        let request =
            serde_json::from_str::<BrowserDownloadRequest>(&payload).map_err(|error| {
                format!(
                    "failed to parse browser inbox file {}: {error}",
                    path.display()
                )
            })?;
        staged.push(StagedBrowserRequest { path, request });
    }

    Ok(staged)
}

pub fn acknowledge_staged_browser_request(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to remove processed browser inbox file {}: {error}",
            path.display()
        )),
    }
}

pub fn quarantine_staged_browser_request(path: &Path) -> Result<PathBuf, String> {
    let quarantined_path = path.with_extension("invalid");
    fs::rename(path, &quarantined_path).map_err(|error| {
        format!(
            "failed to quarantine invalid browser inbox file {}: {error}",
            path.display()
        )
    })?;
    Ok(quarantined_path)
}

pub fn browser_inbox_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(INBOX_DIR_NAME)
}

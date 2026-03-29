use linux_download_manager::{
    BrowserDownloadRequest, new_browser_download_request, resolve_app_data_dir,
    stage_browser_request,
};
use serde::{Deserialize, Serialize};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeHostInput {
    url: String,
    audio_url: Option<String>,
    save_dir: Option<String>,
    expected_checksum: Option<String>,
    scheduled_at: Option<String>,
    bandwidth_limit_kbps: Option<u64>,
    source_page_url: Option<String>,
    source_title: Option<String>,
    format: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeHostOutput {
    ok: bool,
    staged: bool,
    inbox_file: Option<String>,
    error: Option<String>,
}

fn main() {
    if let Err(error) = run() {
        let _ = write_message(&NativeHostOutput {
            ok: false,
            staged: false,
            inbox_file: None,
            error: Some(error),
        });
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    while let Some(input) = read_message::<NativeHostInput>()? {
        let request: BrowserDownloadRequest = new_browser_download_request(
            input.url,
            input.audio_url,
            input.save_dir,
            input.expected_checksum,
            input.scheduled_at,
            input.bandwidth_limit_kbps,
            input.source_page_url,
            input.source_title,
            input.format,
        );

        let app_data_dir = resolve_app_data_dir()?;
        let staged_file = stage_browser_request(&app_data_dir, &request)?;
        let _ = ensure_desktop_app_running();

        write_message(&NativeHostOutput {
            ok: true,
            staged: true,
            inbox_file: Some(staged_file.display().to_string()),
            error: None,
        })?;
    }

    Ok(())
}

fn read_message<T>() -> Result<Option<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    let mut stdin = io::stdin().lock();
    let mut header = [0_u8; 4];
    match stdin.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read native host message header: {error}"
            ));
        }
    }

    let message_length = u32::from_ne_bytes(header) as usize;
    if message_length == 0 || message_length > MAX_MESSAGE_BYTES {
        return Err(format!(
            "native host message size is invalid: {message_length} bytes"
        ));
    }

    let mut payload = vec![0_u8; message_length];
    stdin
        .read_exact(&mut payload)
        .map_err(|error| format!("failed to read native host message payload: {error}"))?;

    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| format!("failed to decode native host message payload: {error}"))
}

fn write_message<T>(message: &T) -> Result<(), String>
where
    T: Serialize,
{
    let payload = serde_json::to_vec(message)
        .map_err(|error| format!("failed to encode host response: {error}"))?;
    let header = (payload.len() as u32).to_ne_bytes();
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&header)
        .map_err(|error| format!("failed to write native host response header: {error}"))?;
    stdout
        .write_all(&payload)
        .map_err(|error| format!("failed to write native host response body: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush native host response: {error}"))?;
    Ok(())
}

fn ensure_desktop_app_running() -> Result<(), String> {
    if is_desktop_app_running()? {
        return Ok(());
    }

    let desktop_binary = desktop_binary_path()?;
    Command::new(&desktop_binary).spawn().map_err(|error| {
        format!(
            "failed to launch desktop application {}: {error}",
            desktop_binary.display()
        )
    })?;
    Ok(())
}

fn desktop_binary_path() -> Result<PathBuf, String> {
    let current_exe =
        std::env::current_exe().map_err(|error| format!("failed to resolve host path: {error}"))?;
    let Some(parent) = current_exe.parent() else {
        return Err("native host executable parent directory is missing".to_string());
    };

    let candidate = parent.join("linux-download-manager");
    if candidate.exists() {
        return Ok(candidate);
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let search_dirs = [
        format!("{home}/Downloads"),
        format!("{home}/Applications"),
        format!("{home}/Desktop"),
        format!("{home}/.local/bin"),
        "/usr/local/bin".to_string(),
        format!("{home}"),
    ];

    for dir in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("Linux_Download_Manager") && name.ends_with(".AppImage") {
                    return Ok(entry.path());
                }
                if name == "linux-download-manager" {
                    return Ok(entry.path());
                }
            }
        }
    }

    Err(format!(
        "desktop application binary was not found: searched near {} and common directories",
        candidate.display()
    ))
}

fn is_desktop_app_running() -> Result<bool, String> {
    let proc_dir = Path::new("/proc");
    let entries =
        std::fs::read_dir(proc_dir).map_err(|error| format!("failed to inspect /proc: {error}"))?;

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid) = file_name.to_str() else {
            continue;
        };
        if !pid.chars().all(|character| character.is_ascii_digit()) {
            continue;
        }

        let cmdline_path = entry.path().join("cmdline");
        let Ok(contents) = std::fs::read(&cmdline_path) else {
            continue;
        };

        if contents.is_empty() {
            continue;
        }

        if cmdline_contains_desktop_binary(&contents) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn cmdline_contains_desktop_binary(contents: &[u8]) -> bool {
    contents
        .split(|byte| *byte == 0)
        .filter(|segment| !segment.is_empty())
        .filter_map(|segment| std::str::from_utf8(segment).ok())
        .any(|segment| {
            segment.ends_with("/linux-download-manager") || segment == "linux-download-manager"
                || segment.contains("Linux_Download_Manager") && segment.contains(".AppImage")
        })
}

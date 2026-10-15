use linux_download_manager::resolve_browser_socket_path;
use serde::{Deserialize, Serialize};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

fn main() {
    if let Err(error) = run() {
        let _ = write_message(&serde_json::json!({"ok":false,"error":error}));
    }
}
fn run() -> Result<(), String> {
    while let Some(input) = read_message::<serde_json::Value>()? {
        let result =
            forward(&input).unwrap_or_else(|error| serde_json::json!({"ok":false,"error":error}));
        write_message(&result)?;
    }
    Ok(())
}
fn forward(input: &serde_json::Value) -> Result<serde_json::Value, String> {
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};
    if input["action"].as_str() == Some("capture_settings") {
        return linux_download_manager::capture::request(input);
    }
    let path = resolve_browser_socket_path()?;
    // Connect before launching so an existing process is never disturbed.
    let socket = UnixStream::connect(&path);
    let mut socket = match socket {
        Ok(s) => s,
        Err(_) => {
            ensure_desktop_app_running()?;
            let started = Instant::now();
            loop {
                match UnixStream::connect(&path) {
                    Ok(s)=>break s,
                    Err(_) if started.elapsed()<Duration::from_secs(10)=>std::thread::sleep(Duration::from_millis(200)),
                    Err(_)=>return Err("The application did not respond. Restart Linux Download Manager after updating.".into()),
                }
            }
        }
    };
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    socket
        .write_all(&serde_json::to_vec(input).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    socket
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    socket
        .take(MAX_MESSAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("Response too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
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
    Command::new(&desktop_binary)
        .arg("--background")
        .spawn()
        .map_err(|error| {
            format!(
                "failed to launch desktop application {}: {error}",
                desktop_binary.display()
            )
        })?;
    Ok(())
}

fn desktop_binary_path() -> Result<PathBuf, String> {
    // A package-managed bridge must never select a previously saved AppImage.
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    if executable.parent() == Some(std::path::Path::new("/usr/lib/linux-download-manager")) {
        let app = executable.with_file_name("linux-download-manager-custom");
        return app.is_file().then_some(app)
            .ok_or_else(|| "Package-managed LDM executable is missing; reinstall the package".into());
    }
    if let Ok(data) = std::env::var("XDG_DATA_HOME").map(PathBuf::from).or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share"))).map(|p| p.join("linux-download-manager-custom")) {
        if let Ok(saved) = std::fs::read_to_string(data.join(".appimage_path")) {
            let path = PathBuf::from(saved);
            if path.is_absolute() && path.is_file() { return Ok(path); }
        }
    }
    let current_exe =
        std::env::current_exe().map_err(|error| format!("failed to resolve host path: {error}"))?;
    let Some(parent) = current_exe.parent() else {
        return Err("native host executable parent directory is missing".to_string());
    };

    let candidate = parent.join("linux-download-manager-custom");
    if candidate.exists() {
        return Ok(candidate);
    }
    let candidate_orig = parent.join("linux-download-manager");
    if candidate_orig.exists() {
        return Ok(candidate_orig);
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let search_dirs = [
        format!("{home}/Downloads"),
        format!("{home}/Applications"),
        format!("{home}/Desktop"),
        format!("{home}/.local/bin"),
        "/usr/local/bin".to_string(),
        home.to_string(),
    ];

    for dir in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("Linux_Download_Manager") && name.ends_with(".AppImage") {
                    return Ok(entry.path());
                }
                if name == "linux-download-manager-custom" || name == "linux-download-manager" {
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
            segment.ends_with("/linux-download-manager")
                || segment == "linux-download-manager"
                || segment.ends_with("/linux-download-manager-custom")
                || segment == "linux-download-manager-custom"
                || segment.contains("Linux_Download_Manager") && segment.contains(".AppImage")
        })
}

use linux_download_manager::resolve_browser_socket_path;
use serde::{Deserialize, Serialize};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
#[path = "../trusted_executable.rs"]
mod trusted_executable;

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
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let home = std::env::var_os("HOME").map(PathBuf::from)
        .ok_or("HOME environment variable is missing")?;
    let data = linux_download_manager::resolve_app_data_dir()?;
    let package_sibling = (executable.parent() == Some(Path::new("/usr/lib/linux-download-manager")))
        .then(|| executable.with_file_name("linux-download-manager"));
    select_desktop_binary(package_sibling.as_deref(), &data, &fixed_install_paths(&home), trusted_executable::current_uid()?)
}

fn fixed_install_paths(home: &Path) -> Vec<PathBuf> {
    [home.join(".local/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")]
        .into_iter().map(|directory| directory.join("linux-download-manager")).collect()
}

fn select_desktop_binary(
    package_sibling: Option<&Path>,
    data: &Path,
    fixed_paths: &[PathBuf],
    uid: u32,
) -> Result<PathBuf, String> {
    use trusted_executable::trusted_file;
    // A package-managed bridge must never fall back to a saved user executable.
    if let Some(app) = package_sibling {
        return trusted_file(app, uid, true).then(|| app.to_path_buf())
            .ok_or_else(|| "Package-managed LDM executable is missing or unsafe; reinstall the package".into());
    }
    let registration = data.join(".appimage_path");
    if trusted_file(&registration, uid, false)
        && std::fs::metadata(&registration).is_ok_and(|metadata| metadata.len() <= 4096)
    {
        if let Ok(saved) = std::fs::read_to_string(&registration) {
            let app = PathBuf::from(saved.trim_end_matches(['\r', '\n']));
            if trusted_file(&app, uid, true) {
                return Ok(app);
            }
        }
    }
    fixed_paths.iter().find(|candidate| trusted_file(candidate, uid, true)).cloned()
        .ok_or_else(|| "LDM executable is missing or unsafe. Reinstall LDM or launch your AppImage once to register its exact path.".into())
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
                || segment.contains("Linux_Download_Manager") && segment.contains(".AppImage")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use trusted_executable::{current_uid, test_support::Fixture};

    #[test]
    fn downloaded_desktop_and_arbitrary_sibling_names_are_never_discovered() {
        let fixture = Fixture::new();
        for location in ["Downloads", "Desktop", "Applications", "host-directory", ""] {
            fixture.script(format!("{location}/Linux_Download_Manager_evil.AppImage").trim_start_matches('/'), "exit 0");
            fixture.script(format!("{location}/linux-download-manager-custom").trim_start_matches('/'), "exit 0");
        }
        let fixed: Vec<_> = fixed_install_paths(&fixture.path).into_iter()
            .filter(|path| path.starts_with(&fixture.path)).collect();
        assert!(select_desktop_binary(None, &fixture.path, &fixed, current_uid().unwrap()).is_err());
        let installed = fixture.script(".local/bin/linux-download-manager", "exit 0");
        assert_eq!(select_desktop_binary(None, &fixture.path, &fixed, current_uid().unwrap()).unwrap(), installed);
    }

    #[test]
    fn legacy_custom_binary_is_never_launched() {
        let fixture = Fixture::new();
        fixture.script(".local/bin/linux-download-manager-custom", "exit 0");
        let fixed: Vec<_> = fixed_install_paths(&fixture.path).into_iter()
            .filter(|path| path.starts_with(&fixture.path)).collect();
        assert!(select_desktop_binary(None, &fixture.path, &fixed, current_uid().unwrap()).is_err());
        let installed = fixture.script(".local/bin/linux-download-manager", "exit 0");
        assert_eq!(select_desktop_binary(None, &fixture.path, &fixed, current_uid().unwrap()).unwrap(), installed);
    }

    #[test]
    fn registered_appimage_and_package_precedence_are_preserved() {
        let fixture = Fixture::new();
        let uid = current_uid().unwrap();
        let image = fixture.script("Downloads/Explicit image.AppImage", "exit 0");
        let registration = fixture.path.join(".appimage_path");
        fs::write(&registration, format!("{}\n", image.display())).unwrap();
        fs::set_permissions(&registration, fs::Permissions::from_mode(0o600)).unwrap();
        let fixed = fixture.script(".local/bin/linux-download-manager", "exit 0");
        assert_eq!(select_desktop_binary(None, &fixture.path, std::slice::from_ref(&fixed), uid).unwrap(), image);
        let package = fixture.script("package/linux-download-manager", "exit 0");
        assert_eq!(select_desktop_binary(Some(&package), &fixture.path, std::slice::from_ref(&fixed), uid).unwrap(), package);
        fs::remove_file(&package).unwrap();
        assert!(select_desktop_binary(Some(&package), &fixture.path, &[fixed], uid).is_err());
    }

    #[test]
    fn registration_and_candidates_require_safe_executable_files() {
        let fixture = Fixture::new();
        let uid = current_uid().unwrap();
        let image = fixture.script("registered.AppImage", "exit 0");
        let registration = fixture.path.join(".appimage_path");
        fs::write(&registration, image.to_str().unwrap()).unwrap();
        fs::set_permissions(&registration, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(select_desktop_binary(None, &fixture.path, &[], uid).is_ok());
        fs::set_permissions(&image, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(select_desktop_binary(None, &fixture.path, &[], uid).is_err());
        fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&registration, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(select_desktop_binary(None, &fixture.path, &[], uid).is_err());
        fs::remove_file(&registration).unwrap();
        let real = fixture.path.join("real-registration");
        fs::write(&real, image.to_str().unwrap()).unwrap();
        symlink(&real, &registration).unwrap();
        assert!(select_desktop_binary(None, &fixture.path, &[], uid).is_err());
        let image_link = fixture.path.join("image-link");
        symlink(&image, &image_link).unwrap();
        assert!(select_desktop_binary(None, &fixture.path, &[image_link], uid).is_err());
        assert!(select_desktop_binary(None, &fixture.path, std::slice::from_ref(&fixture.path), uid).is_err());
    }

    #[test]
    fn relative_partial_and_oversized_registrations_are_not_accepted() {
        let fixture = Fixture::new();
        let uid = current_uid().unwrap();
        let registration = fixture.path.join(".appimage_path");
        for text in ["relative.AppImage".to_string(), "/missing/partial.AppImage".into(), "x".repeat(4097)] {
            fs::write(&registration, text).unwrap();
            fs::set_permissions(&registration, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(select_desktop_binary(None, &fixture.path, &[], uid).is_err());
        }
    }
}

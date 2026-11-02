use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::os::unix::fs::DirBuilderExt;
use std::process::{Command, Stdio};
use std::io::Write;
use std::time::{Duration, Instant};
use crate::trusted_executable::{current_uid, trusted_directory, trusted_file};

const EXTENSION_FILES: [&str; 9] = [
    "manifest.json", "service-worker.js", "player-manifest-observer.js",
    "youtube-audio-observer.js", "content-script.js", "content-style.css",
    "popup.html", "popup.js", "popup.css",
];

pub fn package_firefox_extension(
    firefox_dir: impl AsRef<Path>,
    output_zip: impl AsRef<Path>,
) -> Result<(), String> {
    let source_dir = firefox_dir.as_ref();
    let destination = output_zip.as_ref();
    let parent = destination.parent().ok_or("Firefox ZIP needs a parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = destination.with_extension("zip.tmp");
    let file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for name in EXTENSION_FILES {
        let contents = fs::read(source_dir.join(name))
            .map_err(|error| format!("{}: {error}", source_dir.join(name).display()))?;
        archive.start_file(name, options).map_err(|error| error.to_string())?;
        archive.write_all(&contents).map_err(|error| error.to_string())?;
    }
    archive.finish().map_err(|error| error.to_string())?;
    fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn resolve_app_data_dir() -> Result<PathBuf, String> {
    let base = match env::var("XDG_DATA_HOME") {
        Ok(value) => PathBuf::from(value),
        Err(_) => {
            let home = env::var("HOME").map_err(|_| "HOME environment variable is missing".to_string())?;
            PathBuf::from(home).join(".local").join("share")
        }
    };
    let data = base.join("linux-download-manager");
    migrate_legacy_data_dir(&base.join("linux-download-manager-custom"), &data);
    Ok(data)
}

/// Earlier releases kept history under a "-custom" folder; adopt it once.
fn migrate_legacy_data_dir(legacy: &Path, data: &Path) {
    if !data.exists() && legacy.is_dir() && !legacy.is_symlink() {
        let _ = fs::rename(legacy, data);
    }
}

pub fn resolve_default_download_dir() -> Result<PathBuf, String> {
    let home = env::var("HOME").map_err(|_| "HOME environment variable is missing".to_string())?;
    let home_path = PathBuf::from(&home);

    // XDG user directories. `xdg-user-dir DOWNLOAD` is the canonical answer,
    // but the file is read directly so startup never depends on that binary
    // being installed.
    if let Ok(contents) = fs::read_to_string(home_path.join(".config").join("user-dirs.dirs")) {
        for line in contents.lines() {
            let Some(raw) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") else {
                continue;
            };
            let raw = raw.trim().trim_matches('"');
            if raw.is_empty() {
                continue;
            }
            let expanded = if let Some(rest) = raw.strip_prefix("$HOME/") {
                home_path.join(rest)
            } else if raw == "$HOME" {
                home_path.clone()
            } else {
                PathBuf::from(raw)
            };
            if expanded.is_absolute() {
                return Ok(expanded);
            }
        }
    }

    // `~/Downloads` is the XDG fallback, not a hard-coded username path.
    Ok(home_path.join("Downloads"))
}

pub fn is_first_run() -> bool {
    let app_data = match resolve_app_data_dir() {
        Ok(d) => d,
        Err(_) => return true,
    };
    !app_data.join(".setup_done").exists()
}

/// No network access or shared yt-dlp configuration: tools are installed by the
/// package/archive installer. Every external probe has a finite deadline.
pub fn run_first_time_setup() -> Result<(), String> {
    let home = env::var("HOME").map_err(|_| "HOME environment variable is missing")?;
    let app_data = resolve_app_data_dir()?;
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let exe_dir = executable.parent().ok_or("Executable directory is missing")?;
    let path = env::var_os("PATH").unwrap_or_default();
    let uid = current_uid()?;
    create_private_dir(&app_data)?;
    if !trusted_directory(&app_data, uid) {
        return Err("Application data directory has unsafe ownership or permissions".into());
    }
    if let Some(image) = env::var_os("APPIMAGE").map(PathBuf::from) {
        if !trusted_file(&image, uid, true) {
            return Err("Cannot register an unsafe or non-executable AppImage".into());
        }
        let registration = app_data.join(".appimage_path");
        let bytes = image.as_os_str().as_encoded_bytes();
        if fs::read(&registration).ok().as_deref() != Some(bytes) {
            write_atomic(&registration, bytes, false)?;
        }
    }
    setup_at(Path::new(&home), &app_data, exe_dir, &path, uid)
}

fn setup_at(
    home: &Path,
    app_data: &Path,
    exe_dir: &Path,
    path: &std::ffi::OsStr,
    uid: u32,
) -> Result<(), String> {
    let system_package = exe_dir == Path::new("/usr/lib/linux-download-manager");
    let source_native_host = exe_dir.join("browser_native_host");
    let installed_native_host = if system_package {
        source_native_host.clone()
    } else {
        app_data.join("bin/browser_native_host")
    };
    // Per-user installs keep the host in app-data, not beside the launcher.
    let source_native_host = if !system_package && !source_native_host.exists() {
        installed_native_host.clone()
    } else {
        source_native_host
    };
    use sha2::{Digest, Sha256};
    if !trusted_file(&source_native_host, uid, true) {
        return Err("Native messaging host is missing or unsafe; reinstall LDM".into());
    }
    let host_bytes = fs::read(&source_native_host).map_err(|error| error.to_string())?;
    let marker = app_data.join(".setup_done");
    let expected = format!("startup-v2:{}\n{}\n{:x}", env!("CARGO_PKG_VERSION"), installed_native_host.display(), Sha256::digest(&host_bytes));
    if trusted_file(&marker, uid, false)
        && fs::read_to_string(&marker).is_ok_and(|stored| stored == expected)
        && trusted_file(&installed_native_host, uid, true)
        && fs::read(&installed_native_host).is_ok_and(|bytes| bytes == host_bytes)
    {
        return Ok(());
    }

    match fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    if !system_package {
        let parent = installed_native_host.parent().ok_or("Host directory is missing")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        write_atomic(&installed_native_host, &host_bytes, true)?;
    }
    install_browser_extension(exe_dir, app_data, home)?;
    install_firefox_native_host(home, &installed_native_host)?;

    let extension_id = detect_extension_id(&home.to_string_lossy());
    let manifest = serde_json::json!({
        "name": "com.eko.linuxdownloadmanager",
        "description": "Native messaging bridge for Linux Download Manager",
        "path": installed_native_host.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{extension_id}/")]
    });
    let manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    for browser in [
        ".config/google-chrome", ".config/chromium", ".config/BraveSoftware/Brave-Browser",
        ".config/microsoft-edge-dev", ".config/microsoft-edge", ".config/vivaldi",
    ] {
        let profile = home.join(browser);
        if profile.is_dir() {
            let directory = profile.join("NativeMessagingHosts");
            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            write_atomic(&directory.join("com.eko.linuxdownloadmanager.json"), &manifest_json, false)?;
        }
    }
    // Preserve HTTP/browser integration even when optional media tools need
    // installation, but never record the whole setup as successful in that case.
    validate_media_tools(home, path, uid, Duration::from_secs(2))?;
    fs::create_dir_all(app_data).map_err(|error| error.to_string())?;
    write_atomic(&marker, expected.as_bytes(), false)
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
        .map_err(|error| error.to_string())
}

fn write_atomic(destination: &Path, contents: &[u8], executable: bool) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let parent = destination.parent().ok_or("Destination directory is missing")?;
    if !trusted_directory(parent, current_uid()?) {
        return Err(format!("Unsafe destination directory: {}", parent.display()));
    }
    let name = destination.file_name().ok_or("Destination filename is missing")?;
    let temporary = parent.join(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(&temporary).map_err(|error| error.to_string())?;
    let result = (|| {
        file.write_all(contents).map_err(|error| error.to_string())?;
        file.set_permissions(fs::Permissions::from_mode(if executable { 0o755 } else { 0o600 }))
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, destination).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn install_firefox_native_host(home: &Path, native_host: &Path) -> Result<(), String> {
    // Register locally without probing or launching a browser process.
    let firefox_dir = home.join(".mozilla/native-messaging-hosts");
    fs::create_dir_all(&firefox_dir).map_err(|error| error.to_string())?;
    let manifest = serde_json::json!({
        "name": "com.eko.linuxdownloadmanager",
        "description": "Native messaging bridge for Linux Download Manager",
        "path": native_host.to_string_lossy(),
        "type": "stdio",
        "allowed_extensions": ["ldm-bridge@ekremx25.github.io"]
    });
    write_atomic(&firefox_dir.join("com.eko.linuxdownloadmanager.json"), manifest.to_string().as_bytes(), false)
}

fn detect_extension_id(home: &str) -> String {
    let browser_dirs = [
        format!("{home}/.config/google-chrome/NativeMessagingHosts"),
        format!("{home}/.config/chromium/NativeMessagingHosts"),
        format!("{home}/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts"),
        format!("{home}/.config/microsoft-edge-dev/NativeMessagingHosts"),
        format!("{home}/.config/microsoft-edge/NativeMessagingHosts"),
        format!("{home}/.config/vivaldi/NativeMessagingHosts"),
    ];

    for dir in &browser_dirs {
        let manifest_path = format!("{dir}/com.eko.linuxdownloadmanager.json");
        if let Ok(content) = fs::read_to_string(&manifest_path)
            && let Some(start) = content.find("chrome-extension://")
        {
            let rest = &content[start + 19..];
            if let Some(end) = rest.find('/') {
                let id = &rest[..end];
                if !id.is_empty() && id != "unknown" && id != "__EXTENSION_ORIGIN__" {
                    return id.to_string();
                }
            }
        }
    }

    "dhbkcopeagecbkoncdjefnjlcienhlpg".to_string()
}

fn install_browser_extension(exe_dir: &Path, app_data: &Path, home: &Path) -> Result<(), String> {
    // Put the extension under ~/Documents so the user can find it visually
    // ("Load unpacked" in chrome://extensions), not buried under ~/.local/share.
    let ext_dest = PathBuf::from(home)
        .join("Documents")
        .join("Linux Download Manager Extension");
    let ext_marker = app_data.join(".extension_installed");

    // Tauri's AppImage bundler puts resource files under
    // `usr/lib/<ProductName>/_up_/browser/...`, so from `usr/bin/<binary>`
    // the correct relative path is `../lib/<ProductName>/_up_/browser/chromium`.
    // Older Tauri builds and `cargo tauri dev` put resources next to the
    // binary at `_up_/browser/chromium`, so we check several candidates.
    let mut candidates: Vec<PathBuf> = vec![
        exe_dir.join("_up_/browser/chromium"),
        exe_dir.join("../resources/browser/chromium"),
    ];
    if let Some(parent) = exe_dir.parent() {
        // Walk every subdirectory of `../lib/` — we don't hard-code the
        // product name so a rename of the app doesn't break this path.
        let lib_dir = parent.join("lib");
        if let Ok(entries) = fs::read_dir(&lib_dir) {
            for entry in entries.flatten() {
                let c = entry.path().join("_up_/browser/chromium");
                if c.is_dir() {
                    candidates.push(c);
                }
            }
        }
    }

    let source_dir = match candidates.into_iter().find(|path| path.is_dir()) {
        Some(source) => source,
        None => {
            // The per-user installer has already published these payloads.
            let firefox_dest = home.join("Documents/Linux Download Manager Firefox Extension");
            let uid = current_uid()?;
            if !EXTENSION_FILES.iter().all(|name| {
                trusted_file(&ext_dest.join(name), uid, false)
                    && trusted_file(&firefox_dest.join(name), uid, false)
            }) {
                return Err("Bundled browser extension is missing; reinstall LDM".into());
            }
            package_firefox_extension(
                &firefox_dest,
                home.join("Documents/Linux Download Manager Firefox Extension.zip"),
            )?;
            return write_atomic(&ext_marker, ext_dest.as_os_str().as_encoded_bytes(), false);
        }
    };
    fs::create_dir_all(&ext_dest).map_err(|error| error.to_string())?;
    for name in EXTENSION_FILES {
        let contents = fs::read(source_dir.join(name)).map_err(|error| format!("{name}: {error}"))?;
        write_atomic(&ext_dest.join(name), &contents, false)?;
    }

    let manifest_source = source_dir.parent().ok_or("Browser resource directory is missing")?
        .join("firefox/manifest.json");
    let firefox_dest = home.join("Documents/Linux Download Manager Firefox Extension");
    fs::create_dir_all(&firefox_dest).map_err(|error| error.to_string())?;
    for name in EXTENSION_FILES {
        let source = if name == "manifest.json" { manifest_source.clone() } else { source_dir.join(name) };
        let contents = fs::read(&source).map_err(|error| format!("{}: {error}", source.display()))?;
        write_atomic(&firefox_dest.join(name), &contents, false)?;
    }
    package_firefox_extension(
        &firefox_dest,
        home.join("Documents/Linux Download Manager Firefox Extension.zip"),
    )?;
    // Do not launch browsers or modal helper processes from startup.
    eprintln!("Browser extension files: {}", ext_dest.display());
    write_atomic(&ext_marker, ext_dest.as_os_str().as_encoded_bytes(), false)
}

fn tool_on_path(name: &str, path: &std::ffi::OsStr, uid: u32) -> Option<PathBuf> {
    env::split_paths(path).map(|directory| directory.join(name))
        .find(|candidate| trusted_file(candidate, uid, true))
}

/// Variables an AppImage points into its own bundle. A host tool that inherits
/// them can break: system python3 under the bundle's PYTHONHOME cannot start,
/// so a working yt-dlp looked "invalid" to the startup check.
pub(crate) const BUNDLE_ENV: &[&str] = &[
    "LD_LIBRARY_PATH", "LD_PRELOAD", "PYTHONPATH", "PYTHONHOME", "PERLLIB",
    "QT_PLUGIN_PATH", "GST_PLUGIN_SYSTEM_PATH", "APPDIR", "APPIMAGE", "OWD",
];

/// Run host tools (yt-dlp, FFmpeg, runtimes) without the AppImage's bundle paths.
pub(crate) fn without_bundle_env(command: &mut Command) -> &mut Command {
    for name in BUNDLE_ENV {
        command.env_remove(name);
    }
    command
}

fn probe_tool(program: &Path, flag: &str, timeout: Duration, home: &Path) -> Result<(), String> {
    let mut child = without_bundle_env(&mut Command::new(program)).arg(flag).env("HOME", home).stdin(Stdio::null())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|error| format!("{} could not start: {error}", program.display()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{} is invalid or incomplete ({status}); reinstall it", program.display())),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match outcome {
                    Err(error) => format!("{} probe failed: {error}", program.display()),
                    _ => format!("{} version check timed out", program.display()),
                });
            }
        }
    }
}

fn validate_media_tools(home: &Path, path: &std::ffi::OsStr, uid: u32, timeout: Duration) -> Result<(), String> {
    for (name, flag) in [("yt-dlp", "--version"), ("ffmpeg", "-version")] {
        let tool = tool_on_path(name, path, uid).ok_or_else(|| format!(
            "Missing or unsafe {name}. Install yt-dlp, FFmpeg and Node or Deno with your package manager or the LDM installer; startup never downloads executables."
        ))?;
        probe_tool(&tool, flag, timeout, home)?;
    }
    for runtime in ["deno", "node"] {
        if let Some(tool) = tool_on_path(runtime, path, uid) {
            if probe_tool(&tool, "--version", timeout, home).is_ok() {
                return Ok(());
            }
        }
    }
    Err("Missing or invalid JavaScript runtime. Install Node or Deno with your package manager or the LDM installer.".into())
}

/// Pass these arguments on each app-owned yt-dlp invocation, never persist
/// options into the user's shared ~/.config/yt-dlp/config.
pub(crate) fn ytdlp_runtime_args() -> Vec<String> {
    let path = env::var_os("PATH").unwrap_or_default();
    let Ok(uid) = current_uid() else { return vec!["--no-mtime".into()]; };
    runtime_args_for_path(&path, uid)
}

fn runtime_args_for_path(path: &std::ffi::OsStr, uid: u32) -> Vec<String> {
    let mut args = vec!["--no-mtime".to_string()];
    for runtime in ["deno", "node"] {
        if let Some(tool) = tool_on_path(runtime, path, uid) {
            args.extend(["--js-runtimes".into(), format!("{runtime}:{}", tool.display())]);
        }
    }
    args
}

/// Linux Unix socket addresses have a small byte limit. Long XDG paths use a
/// private, owner-checked directory with a stable hash instead of truncating.
pub fn resolve_browser_socket_path() -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let data = resolve_app_data_dir()?;
    let direct = data.join("browser-control.sock");
    if direct.as_os_str().len() < 100 {
        return Ok(direct);
    }
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let uid = fs::metadata(&data).map_err(|e| e.to_string())?.uid();
    let hash = format!("{:x}", Sha256::digest(data.as_os_str().as_encoded_bytes()));
    let dir = PathBuf::from(format!("/tmp/ldm-ipc-{uid}-{}", &hash[..16]));
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let meta = fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
    if !meta.file_type().is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(
            "Browser IPC directory must be private and owned by the application user".into(),
        );
    }
    Ok(dir.join("control.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trusted_executable::test_support::Fixture;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn legacy_data_dir_is_adopted_once_and_never_overwrites() {
        let fixture = Fixture::new();
        let legacy = fixture.path.join("linux-download-manager-custom");
        let data = fixture.path.join("linux-download-manager");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("downloads.sqlite3"), "history").unwrap();
        migrate_legacy_data_dir(&legacy, &data);
        assert!(!legacy.exists());
        assert_eq!(fs::read_to_string(data.join("downloads.sqlite3")).unwrap(), "history");

        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("downloads.sqlite3"), "stale").unwrap();
        migrate_legacy_data_dir(&legacy, &data);
        assert_eq!(fs::read_to_string(data.join("downloads.sqlite3")).unwrap(), "history");
    }

    fn ready_fixture() -> Fixture {
        let fixture = Fixture::new();
        fixture.script("bundle/browser_native_host", "exit 0");
        for name in ["yt-dlp", "ffmpeg", "node"] {
            fixture.script(&format!("tools/{name}"), "printf '%s\\n' \"$HOME\" >> \"$0.probes\"\nexit 0");
        }
        for name in EXTENSION_FILES {
            let path = fixture.path.join("bundle/_up_/browser/chromium").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "{}").unwrap();
        }
        let firefox = fixture.path.join("bundle/_up_/browser/firefox");
        fs::create_dir_all(&firefox).unwrap();
        fs::write(firefox.join("manifest.json"), "{}").unwrap();
        fs::create_dir_all(fixture.path.join("home/.config/yt-dlp")).unwrap();
        fs::write(fixture.path.join("home/.config/yt-dlp/config"), "--js-runtimes custom\n--mtime\n# preserve\n").unwrap();
        fs::create_dir_all(fixture.path.join("data")).unwrap();
        fixture
    }

    fn setup(fixture: &Fixture) -> Result<(), String> {
        setup_at(
            &fixture.path.join("home"),
            &fixture.path.join("data"),
            &fixture.path.join("bundle"),
            fixture.path.join("tools").as_os_str(),
            current_uid().unwrap(),
        )
    }

    #[test]
    fn setup_is_once_only_and_preserves_shared_config_and_browser_payloads() {
        let fixture = ready_fixture();
        setup(&fixture).unwrap();
        let home = fixture.path.join("home");
        assert_eq!(fs::read_to_string(home.join(".config/yt-dlp/config")).unwrap(),
            "--js-runtimes custom\n--mtime\n# preserve\n");
        assert_eq!(fs::read_to_string(fixture.path.join("tools/yt-dlp.probes")).unwrap().trim(), home.to_str().unwrap());
        assert!(home.join("Documents/Linux Download Manager Extension/popup.js").is_file());
        assert!(home.join("Documents/Linux Download Manager Firefox Extension.zip").is_file());
        let manifest: serde_json::Value = serde_json::from_slice(&fs::read(home.join(
            ".mozilla/native-messaging-hosts/com.eko.linuxdownloadmanager.json"
        )).unwrap()).unwrap();
        assert_eq!(manifest["allowed_extensions"][0], "ldm-bridge@ekremx25.github.io");
        let installed = fixture.path.join("data/bin/browser_native_host");
        assert!(trusted_file(&installed, current_uid().unwrap(), true));
        fs::remove_file(fixture.path.join("bundle/_up_/browser/chromium/popup.js")).unwrap();
        fixture.script("tools/yt-dlp", "exit 7");
        setup(&fixture).unwrap();
        assert_eq!(fs::read_to_string(fixture.path.join("tools/yt-dlp.probes")).unwrap().lines().count(), 1);
    }

    #[test]
    fn missing_failed_and_partial_helpers_never_mark_setup_successful() {
        let fixture = ready_fixture();
        let tool = fixture.path.join("tools/yt-dlp");
        fs::remove_file(&tool).unwrap();
        assert!(setup(&fixture).unwrap_err().contains("Missing or unsafe yt-dlp"));
        assert!(!fixture.path.join("data/.setup_done").exists());
        fixture.script("tools/yt-dlp", "exit 7");
        assert!(setup(&fixture).unwrap_err().contains("invalid or incomplete"));
        assert!(!fixture.path.join("data/.setup_done").exists());
        fs::write(&tool, "truncated executable").unwrap();
        assert!(setup(&fixture).is_err());
        assert!(!fixture.path.join("data/.setup_done").exists());
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(setup(&fixture).unwrap_err().contains("Missing or unsafe yt-dlp"));
        fixture.script("tools/yt-dlp", "exit 0");
        fs::remove_file(fixture.path.join("tools/ffmpeg")).unwrap();
        assert!(setup(&fixture).unwrap_err().contains("Missing or unsafe ffmpeg"));
        assert!(!fixture.path.join("data/.setup_done").exists());
    }

    #[test]
    fn per_user_install_uses_native_host_in_app_data() {
        let fixture = ready_fixture();
        let packaged = fixture.path.join("bundle/browser_native_host");
        let installed = fixture.path.join("data/bin/browser_native_host");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::rename(packaged, &installed).unwrap();
        install_browser_extension(
            &fixture.path.join("bundle"),
            &fixture.path.join("data"),
            &fixture.path.join("home"),
        ).unwrap();
        fs::remove_dir_all(fixture.path.join("bundle/_up_")).unwrap();
        let previous = fs::read(&installed).unwrap();
        setup(&fixture).unwrap();
        assert_eq!(fs::read(&installed).unwrap(), previous);
        assert!(fixture.path.join("data/.setup_done").is_file());
        assert!(fixture.path.join("home/Documents/Linux Download Manager Firefox Extension.zip").is_file());
        fs::remove_file(fixture.path.join("data/.setup_done")).unwrap();
        fs::remove_file(fixture.path.join("home/Documents/Linux Download Manager Extension/popup.js")).unwrap();
        assert!(setup(&fixture).unwrap_err().contains("Bundled browser extension is missing"));
        assert!(!fixture.path.join("data/.setup_done").exists());
    }

    #[test]
    fn successful_setup_repairs_partial_host_and_refreshes_changed_package_host() {
        let fixture = ready_fixture();
        setup(&fixture).unwrap();
        let installed = fixture.path.join("data/bin/browser_native_host");
        fs::write(&installed, "partial").unwrap();
        setup(&fixture).unwrap();
        assert_eq!(fs::read(&installed).unwrap(), fs::read(fixture.path.join("bundle/browser_native_host")).unwrap());
        fixture.script("bundle/browser_native_host", "# updated package\nexit 0");
        setup(&fixture).unwrap();
        assert_eq!(fs::read(&installed).unwrap(), fs::read(fixture.path.join("bundle/browser_native_host")).unwrap());
    }

    #[test]
    fn missing_shared_config_stays_absent_and_no_download_command_is_started() {
        let fixture = ready_fixture();
        let config = fixture.path.join("home/.config/yt-dlp/config");
        fs::remove_file(&config).unwrap();
        fixture.script("tools/curl", "printf invoked > \"$0.invoked\"\nexit 99");
        setup(&fixture).unwrap();
        assert!(!config.exists());
        assert!(!fixture.path.join("tools/curl.invoked").exists());
    }

    #[test]
    fn resource_failure_does_not_mark_success_and_can_be_retried() {
        let fixture = ready_fixture();
        let resource = fixture.path.join("bundle/_up_/browser/chromium/popup.js");
        fs::remove_file(&resource).unwrap();
        assert!(setup(&fixture).is_err());
        assert!(!fixture.path.join("data/.setup_done").exists());
        fs::write(resource, "{}").unwrap();
        setup(&fixture).unwrap();
        assert!(fixture.path.join("data/.setup_done").is_file());
    }

    #[test]
    fn probe_ignores_appimage_bundle_environment() {
        let fixture = ready_fixture();
        // Mimics system python3 started under an AppImage's PYTHONHOME.
        let tool = fixture.script("python-tool", "[ -z \"$PYTHONHOME$PYTHONPATH$LD_LIBRARY_PATH\" ] || exit 1");
        let mut leaked = Command::new(&tool);
        leaked.env("PYTHONHOME", "/tmp/.mount_x/usr/").env("PYTHONPATH", "/tmp/.mount_x/usr/share/pyshared/")
            .env("LD_LIBRARY_PATH", "/tmp/.mount_x/usr/lib/");
        assert!(!leaked.status().unwrap().success(), "fixture must fail with the bundle environment");
        assert!(without_bundle_env(&mut leaked).status().unwrap().success());
        assert!(probe_tool(&tool, "--version", Duration::from_secs(2), &fixture.path).is_ok());
    }

    #[test]
    fn hung_tool_is_killed_and_runtime_fallback_requires_success() {
        let fixture = ready_fixture();
        let hung = fixture.script("hung", "while :; do :; done");
        let start = Instant::now();
        assert!(probe_tool(&hung, "--version", Duration::from_millis(30), &fixture.path)
            .unwrap_err().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
        fixture.script("tools/deno", "exit 1");
        assert!(validate_media_tools(&fixture.path, fixture.path.join("tools").as_os_str(), current_uid().unwrap(), Duration::from_millis(100)).is_ok());
        fixture.script("tools/node", "exit 1");
        assert!(setup(&fixture).unwrap_err().contains("JavaScript runtime"));
        assert!(!fixture.path.join("data/.setup_done").exists());
    }

    #[test]
    fn runtime_configuration_is_per_invocation_without_commands_or_config_writes() {
        let fixture = Fixture::new();
        let uid = current_uid().unwrap();
        assert_eq!(runtime_args_for_path(fixture.path.as_os_str(), uid), ["--no-mtime"]);
        let node = fixture.script("node", "exit 1");
        assert_eq!(runtime_args_for_path(fixture.path.as_os_str(), uid), [
            "--no-mtime".to_string(), "--js-runtimes".to_string(), format!("node:{}", node.display()),
        ]);
        let deno = fixture.script("deno", "exit 1");
        let args = runtime_args_for_path(fixture.path.as_os_str(), uid);
        assert!(args.contains(&format!("deno:{}", deno.display())));
        assert!(args.contains(&format!("node:{}", node.display())));
        assert!(!fixture.path.join(".config").exists());
    }

    #[test]
    fn atomic_write_rejects_preexisting_temporary_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let target = fixture.path.join("target");
        fs::write(&target, "untouched").unwrap();
        let destination = fixture.path.join("destination");
        let temporary = fixture.path.join(format!(".destination.{}.tmp", std::process::id()));
        symlink(&target, &temporary).unwrap();
        assert!(write_atomic(&destination, b"replacement", true).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
        assert!(!destination.exists());
        fs::remove_file(&temporary).unwrap();
        write_atomic(&destination, b"complete", true).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"complete");
        assert!(!temporary.exists());
    }
}

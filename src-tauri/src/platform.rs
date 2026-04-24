use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn resolve_app_data_dir() -> Result<PathBuf, String> {
    if let Ok(value) = env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(value).join("linux-download-manager"));
    }

    let home = env::var("HOME").map_err(|_| "HOME environment variable is missing".to_string())?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("linux-download-manager"))
}

pub fn resolve_default_download_dir() -> Result<PathBuf, String> {
    let home = env::var("HOME").map_err(|_| "HOME environment variable is missing".to_string())?;
    Ok(PathBuf::from(home).join("Downloads"))
}

pub fn is_first_run() -> bool {
    let app_data = match resolve_app_data_dir() {
        Ok(d) => d,
        Err(_) => return true,
    };
    !app_data.join(".setup_done").exists()
}

pub fn run_first_time_setup() {
    let home = match env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };

    let app_data = match resolve_app_data_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    let setup_marker = app_data.join(".setup_done");
    let current_exe = env::current_exe().unwrap_or_default();
    let exe_dir = current_exe.parent().unwrap_or(Path::new("."));

    let source_native_host = exe_dir.join("browser_native_host");
    if !source_native_host.exists() {
        return;
    }

    let bin_dir = app_data.join("bin");
    let _ = fs::create_dir_all(&bin_dir);
    let installed_native_host = bin_dir.join("browser_native_host");
    let _ = fs::copy(&source_native_host, &installed_native_host);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&installed_native_host, fs::Permissions::from_mode(0o755));
    }

    let needs_setup = if setup_marker.exists() {
        let stored = fs::read_to_string(&setup_marker).unwrap_or_default();
        stored.trim() != installed_native_host.to_string_lossy().trim()
    } else {
        true
    };

    if !needs_setup {
        return;
    }

    let extension_id = detect_extension_id(&home);

    let manifest = serde_json::json!({
        "name": "com.eko.linuxdownloadmanager",
        "description": "Native messaging bridge for Linux Download Manager",
        "path": installed_native_host.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{extension_id}/")]
    });

    let manifest_json = serde_json::to_string_pretty(&manifest).unwrap_or_default();

    let browser_dirs = [
        format!("{home}/.config/google-chrome/NativeMessagingHosts"),
        format!("{home}/.config/chromium/NativeMessagingHosts"),
        format!("{home}/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts"),
    ];

    for dir in &browser_dirs {
        let dir_path = Path::new(dir);
        if dir_path.parent().map(|p| p.exists()).unwrap_or(false) {
            let _ = fs::create_dir_all(dir_path);
            let manifest_path = dir_path.join("com.eko.linuxdownloadmanager.json");
            let _ = fs::write(&manifest_path, &manifest_json);
        }
    }

    install_ytdlp(&home);
    install_browser_extension(exe_dir, &app_data, &home);

    let _ = fs::create_dir_all(&app_data);
    let _ = fs::write(&setup_marker, installed_native_host.to_string_lossy().as_ref());
}

fn detect_extension_id(home: &str) -> String {
    let browser_dirs = [
        format!("{home}/.config/google-chrome/NativeMessagingHosts"),
        format!("{home}/.config/chromium/NativeMessagingHosts"),
        format!("{home}/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts"),
    ];

    for dir in &browser_dirs {
        let manifest_path = format!("{dir}/com.eko.linuxdownloadmanager.json");
        if let Ok(content) = fs::read_to_string(&manifest_path) {
            if let Some(start) = content.find("chrome-extension://") {
                let rest = &content[start + 19..];
                if let Some(end) = rest.find('/') {
                    let id = &rest[..end];
                    if !id.is_empty() && id != "__EXTENSION_ORIGIN__" {
                        return id.to_string();
                    }
                }
            }
        }
    }

    "unknown".to_string()
}

fn install_browser_extension(exe_dir: &Path, app_data: &Path, home: &str) {
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

    let source_dir = match candidates.into_iter().find(|p| p.is_dir()) {
        Some(p) => p,
        None => return,
    };

    let _ = fs::create_dir_all(&ext_dest);

    let files = ["manifest.json", "service-worker.js", "content-script.js", "content-style.css"];
    for file in &files {
        let src = source_dir.join(file);
        let dst = ext_dest.join(file);
        if src.exists() {
            let _ = fs::copy(&src, &dst);
        }
    }

    if ext_marker.exists() {
        return;
    }

    let browsers = [
        ("google-chrome", format!("{home}/.config/google-chrome")),
        ("chromium-browser", format!("{home}/.config/chromium")),
        ("brave-browser", format!("{home}/.config/BraveSoftware/Brave-Browser")),
        ("brave", format!("{home}/.config/BraveSoftware/Brave-Browser")),
    ];

    // Open the extensions page on whichever Chromium-based browser the user
    // actually has installed. First match wins.
    for (browser_cmd, config_dir) in &browsers {
        if Path::new(config_dir).exists() && Command::new("which").arg(browser_cmd).output()
            .map(|o| o.status.success()).unwrap_or(false)
        {
            let _ = Command::new(browser_cmd)
                .arg("chrome://extensions")
                .spawn();
            break;
        }
    }

    // Show a proper modal dialog (zenity → kdialog → notify-send fallback)
    // with the exact path the user has to pick in 'Load unpacked'.
    let message = format!(
        "Linux Download Manager kuruldu.\n\n\
        Tarayıcı eklentisini yüklemek için:\n\n\
        1. Açılan chrome://extensions sayfasında sağ üstten \
        \"Geliştirici modu\" / \"Developer mode\"'u açın\n\n\
        2. \"Paketlenmemiş öğe yükle\" / \"Load unpacked\" butonuna tıklayın\n\n\
        3. Şu klasörü seçin:\n   {}\n\n\
        (Bu klasör Documents altındadır; silmeyin, uygulama bu konuma bağlı.)",
        ext_dest.display()
    );

    let zenity_ok = Command::new("zenity")
        .args([
            "--info",
            "--title=Linux Download Manager",
            "--width=520",
            &format!("--text={message}"),
        ])
        .spawn()
        .is_ok();

    if !zenity_ok {
        let kdialog_ok = Command::new("kdialog")
            .args(["--title", "Linux Download Manager", "--msgbox", &message])
            .spawn()
            .is_ok();

        if !kdialog_ok {
            // Final fallback: notify-send (no interactive modal).
            let _ = Command::new("notify-send")
                .args([
                    "Linux Download Manager",
                    &message,
                    "-t",
                    "30000",
                ])
                .spawn();
        }
    }

    let _ = fs::write(&ext_marker, ext_dest.to_string_lossy().as_ref());
}

fn install_ytdlp(home: &str) {
    let local_bin = format!("{home}/.local/bin");
    let ytdlp_path = format!("{local_bin}/yt-dlp");

    let already_present = Path::new(&ytdlp_path).exists()
        || Command::new("yt-dlp").arg("--version").output().is_ok();

    if !already_present {
        let _ = fs::create_dir_all(&local_bin);
        let curl_status = Command::new("curl")
            .args([
                "-fL",
                "-o",
                &ytdlp_path,
                "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp",
            ])
            .status();

        match curl_status {
            Ok(status) if status.success() => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(
                        &ytdlp_path,
                        fs::Permissions::from_mode(0o755),
                    );
                }
            }
            _ => {
                eprintln!(
                    "WARNING: failed to download yt-dlp. YouTube/social media \
                     downloads will be disabled until you run:\n\
                     \tcurl -fL -o ~/.local/bin/yt-dlp \\\n\
                     \t\thttps://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp\n\
                     \tchmod +x ~/.local/bin/yt-dlp"
                );
                return;
            }
        }
    }

    configure_ytdlp_js_runtime(home);
}

/// yt-dlp needs a JavaScript runtime (deno or node) for YouTube signature
/// extraction since late 2024. Detect whichever is on PATH and pin it in
/// `~/.config/yt-dlp/config` so the app's `yt-dlp` calls don't need extra
/// flags. Preserve any user-authored lines already in the config.
fn configure_ytdlp_js_runtime(home: &str) {
    let runtime = if Command::new("deno").arg("--version").output().is_ok() {
        "deno"
    } else if Command::new("node").arg("--version").output().is_ok() {
        "node"
    } else {
        eprintln!(
            "WARNING: no JavaScript runtime (deno / node) found. YouTube \
             downloads may be limited. Install one with your package manager \
             and re-run the app."
        );
        return;
    };

    let config_dir = PathBuf::from(home).join(".config").join("yt-dlp");
    let config_file = config_dir.join("config");
    let _ = fs::create_dir_all(&config_dir);

    let existing: Vec<String> = match fs::read_to_string(&config_file) {
        Ok(s) => s
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("--js-runtimes") && !t.starts_with("--no-mtime")
            })
            .map(String::from)
            .collect(),
        Err(_) => Vec::new(),
    };

    let mut lines = vec![
        format!("--js-runtimes {runtime}"),
        "--no-mtime".to_string(),
    ];
    lines.extend(existing.into_iter().filter(|l| !l.is_empty()));

    let _ = fs::write(&config_file, lines.join("\n") + "\n");
}

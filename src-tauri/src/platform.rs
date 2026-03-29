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
    let ext_dest = app_data.join("extension");
    let ext_marker = app_data.join(".extension_installed");

    let resource_dir = exe_dir.join("_up_/browser/chromium");
    let alt_resource_dir = exe_dir.join("../resources/browser/chromium");
    let source_dir = if resource_dir.is_dir() {
        resource_dir
    } else if alt_resource_dir.is_dir() {
        alt_resource_dir
    } else {
        return;
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
    ];

    for (browser_cmd, config_dir) in &browsers {
        if Path::new(config_dir).exists() {
            let _ = Command::new(browser_cmd)
                .arg("chrome://extensions")
                .spawn();
            break;
        }
    }

    let _ = Command::new("notify-send")
        .arg("Linux Download Manager")
        .arg(format!(
            "Extension kurulumu:\n\
            1. Açılan chrome://extensions sayfasında 'Geliştirici modu'nu açın\n\
            2. 'Paketlenmemiş öğe yükle' tıklayın\n\
            3. Bu klasörü seçin:\n{}",
            ext_dest.display()
        ))
        .arg("-t")
        .arg("15000")
        .spawn();

    let _ = fs::write(&ext_marker, ext_dest.to_string_lossy().as_ref());
}

fn install_ytdlp(home: &str) {
    let local_bin = format!("{home}/.local/bin");
    let ytdlp_path = format!("{local_bin}/yt-dlp");

    if Path::new(&ytdlp_path).exists() {
        return;
    }

    if Command::new("yt-dlp").arg("--version").output().is_ok() {
        return;
    }

    let _ = fs::create_dir_all(&local_bin);
    let _ = Command::new("curl")
        .arg("-L")
        .arg("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp")
        .arg("-o")
        .arg(&ytdlp_path)
        .output();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&ytdlp_path, fs::Permissions::from_mode(0o755));
    }
}

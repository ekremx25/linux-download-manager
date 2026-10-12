use std::{env, fs, path::{Path, PathBuf}};

fn entry_path() -> Result<PathBuf, String> {
    let base = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .ok_or("Cannot find the user configuration directory")?;
    Ok(base.join("autostart/linux-download-manager.desktop"))
}

fn enabled_at(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|s| !s.lines().any(|line|
        matches!(line.trim(), "Hidden=true" | "X-GNOME-Autostart-enabled=false")))
}

pub fn enabled() -> bool {
    entry_path().is_ok_and(|p| enabled_at(&p))
}

fn write_entry(path: &Path, executable: &Path, enabled: bool) -> Result<(), String> {
    if !enabled {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Could not disable login startup: {e}")),
        };
    }
    let raw = executable.to_str().ok_or("Executable path is not valid UTF-8")?;
    if raw.contains(['\n', '\r']) { return Err("Unsupported executable path".into()); }
    // Escape Exec arguments first, then desktop-entry string backslashes.
    let mut quoted = String::new();
    for c in raw.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') { quoted.push('\\'); }
        if c == '%' { quoted.push('%'); }
        quoted.push(c);
    }
    let exec = quoted.replace('\\', "\\\\");
    let contents = format!("[Desktop Entry]\nType=Application\nName=Linux Download Manager\nExec=\"{exec}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n");
    let parent = path.parent().ok_or("Invalid autostart path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension("desktop.tmp");
    fs::write(&temp, contents).map_err(|e| format!("Could not save login startup: {e}"))?;
    fs::rename(temp, path).map_err(|e| format!("Could not enable login startup: {e}"))
}

pub fn set_enabled(value: bool) -> Result<(), String> {
    let executable = env::var_os("APPIMAGE").map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_file())
        .map(Ok).unwrap_or_else(env::current_exe).map_err(|e| e.to_string())?;
    write_entry(&entry_path()?, &executable, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_startup_can_be_enabled_disabled_and_reenabled() {
        let root = env::temp_dir().join(format!("ldm-autostart-{}", std::process::id()));
        let path = root.join("autostart/linux-download-manager.desktop");
        assert!(!enabled_at(&path));
        write_entry(&path, Path::new("/home/test user/LDM"), true).unwrap();
        assert!(enabled_at(&path));
        assert!(fs::read_to_string(&path).unwrap().contains("Exec=\"/home/test user/LDM\""));
        fs::write(&path, "[Desktop Entry]\nHidden=true\n").unwrap();
        assert!(!enabled_at(&path));
        write_entry(&path, Path::new("/home/test user/LDM"), true).unwrap();
        assert!(enabled_at(&path));
        write_entry(&path, Path::new("/unused"), false).unwrap();
        write_entry(&path, Path::new("/unused"), false).unwrap();
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }
}

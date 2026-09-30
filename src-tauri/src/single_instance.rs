use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use tauri::{Emitter, Manager};

pub fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("linux-download-manager.sock")
    } else {
        crate::platform::resolve_app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("linux-download-manager.sock")
    }
}

pub fn try_activate_existing_instance(args: &[String]) -> bool {
    let socket_path = get_socket_path();
    if socket_path.exists() {
        if let Ok(mut stream) = UnixStream::connect(&socket_path) {
            let msg = if args.len() > 1 && !args[1].is_empty() {
                format!("open:{}\n", args[1])
            } else {
                "show\n".to_string()
            };
            let _ = stream.write_all(msg.as_bytes());
            return true;
        } else {
            // Stale socket from dead process
            let _ = std::fs::remove_file(&socket_path);
        }
    }
    false
}

pub fn start_single_instance_listener(app_handle: tauri::AppHandle) {
    let socket_path = get_socket_path();
    let _ = std::fs::remove_file(&socket_path);

    if let Ok(listener) = UnixListener::bind(&socket_path) {
        let _ = listener.set_nonblocking(true);
        std::thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 1024];
                        if let Ok(n) = stream.read(&mut buf) {
                            if n > 0 {
                                let msg = String::from_utf8_lossy(&buf[..n]);
                                let trimmed = msg.trim();
                                if let Some(window) = app_handle.get_webview_window("main") {
                                    let _ = window.show();
                                    let _ = window.unminimize();
                                    let _ = window.set_focus();
                                    if let Some(url) = trimmed.strip_prefix("open:") {
                                        let url_clean = url.trim();
                                        if !url_clean.is_empty() {
                                            let _ = window.emit("open-url", url_clean);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(150));
                    }
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                }
            }
        });
    }
}

pub fn cleanup_socket() {
    let socket_path = get_socket_path();
    let _ = std::fs::remove_file(&socket_path);
}

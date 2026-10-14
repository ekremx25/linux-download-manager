//! Private, per-user IPC. No TCP listener and no browser-visible credentials.
use crate::{
    app::AppState,
    jobs::{DownloadJobRequest, queue_download_request},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use tauri::{Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub fn start(app: tauri::AppHandle) -> Result<(), String> {
    let path = crate::resolve_browser_socket_path()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    let listener = tokio::net::UnixListener::bind(&path).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn(async move {
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        while let Ok((mut socket, _)) = listener.accept().await {
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _permit = permit;
                let result = tokio::time::timeout(std::time::Duration::from_secs(25), async {
                    let mut bytes = Vec::new();
                    (&mut socket)
                        .take(1024 * 1024 + 1)
                        .read_to_end(&mut bytes)
                        .await
                        .map_err(|e| e.to_string())?;
                    if bytes.len() > 1024 * 1024 {
                        return Err("Message too large".into());
                    }
                    let request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                    handle(&app, request).await
                })
                .await
                .unwrap_or_else(|_| Err("Application request timed out".into()));
                let response = result.unwrap_or_else(|error| json!({"ok":false,"error":error}));
                if let Ok(bytes) = serde_json::to_vec(&response) {
                    let _ = socket.write_all(&bytes).await;
                }
            });
        }
    });
    Ok(())
}

async fn handle(app: &tauri::AppHandle, request: Value) -> Result<Value, String> {
    let state = app.state::<AppState>();
    match request
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("download")
    {
        "status" => {
            let live = state.live_downloads.lock().unwrap().clone();
            let rows=state.storage.list_downloads()?.into_iter().filter(|r| !matches!(r.status.as_str(),"completed"|"cancelled")).take(50)
                .map(|r| {
                    let event=live.get(&r.id);
                    json!({"id":r.id,"name":r.file_name,"status":r.status,"downloadedBytes":r.downloaded_bytes,
                        "totalBytes":r.total_bytes,"speedBytesPerSecond":event.and_then(|e|e.speed_bytes_per_second),
                        "etaSeconds":event.and_then(|e|e.eta_seconds)})
                }).collect::<Vec<_>>();
            Ok(json!({"ok":true,"protocol":1,"downloads":rows}))
        }
        "media_preferences" => {
            let mut settings = state.feature_settings();
            if let Some(value) = request.get("preferences") {
                let container = value
                    .get("container")
                    .and_then(Value::as_str)
                    .ok_or("Missing output container")?;
                let height = value
                    .get("maxHeight")
                    .and_then(Value::as_u64)
                    .filter(|v| *v <= u32::MAX as u64)
                    .ok_or("Missing resolution")?;
                settings.transfer.container = container.to_owned();
                settings.transfer.max_height = height as u32;
                state.save_feature_settings(&settings)?;
            }
            Ok(json!({"ok":true,"preferences":settings.transfer,"showVideoQualityMenu":settings.show_video_quality_menu}))
        }
        "show_details" => {
            let id = request.get("id").and_then(Value::as_i64).ok_or("Missing download id")?;
            state.storage.get_download(id)?;
            state.select_download(id);
            let window = app.get_webview_window("download-detail").ok_or("Detail window unavailable")?;
            crate::commands::size_download_detail_window(&window)?;
            window.show().map_err(|e| e.to_string())?;
            window.unminimize().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            window.emit("download://selected", id).map_err(|e| e.to_string())?;
            Ok(json!({"ok":true}))
        }
        "show" => {
            crate::focus_main_window(app);
            Ok(json!({"ok":true}))
        }
        "pause" | "resume" | "cancel" => {
            let id = request
                .get("id")
                .and_then(Value::as_i64)
                .ok_or("Missing download id")?;
            match request["action"].as_str().unwrap() {
                "pause" => crate::commands::pause_download(app.clone(), state, id).await?,
                "resume" => {
                    crate::commands::resume_download(app.clone(), state, id).await?;
                }
                _ => crate::commands::cancel_download(app.clone(), state, id).await?,
            }
            Ok(json!({"ok":true}))
        }
        "refresh_failed_media" => {
            let id = request.get("id").and_then(Value::as_i64).ok_or("Missing download id")?;
            let old_url = request.get("oldUrl").and_then(Value::as_str).ok_or("Missing old URL")?;
            let new_url = request.get("newUrl").and_then(Value::as_str).ok_or("Missing new URL")?;
            if !same_media_asset(old_url, new_url) {
                return Err("Replacement is not the same media asset".into());
            }
            let record = state.storage.get_download(id)?;
            if record.status != "failed" || record.url != old_url || !record.error_message.as_deref().is_some_and(is_expired_media_error) {
                return Err("Download is not a failed, expired media request".into());
            }
            let mut job = state.storage.load_resume_job(id)?.ok_or("Saved media request is unavailable")?;
            if !job.stream_manifest || job.audio_url.is_some() || job.url != old_url {
                return Err("This media request cannot be refreshed automatically".into());
            }
            let headers: HashMap<String, String> = serde_json::from_value(request.get("httpHeaders").cloned().unwrap_or_default())
                .map_err(|_| "Invalid replacement headers")?;
            if headers.len() > 32 || headers.iter().any(|(key, value)| key.len() > 80 || value.len() > 8192 || key.contains(['\r', '\n']) || value.contains(['\r', '\n'])) {
                return Err("Replacement headers are too large or invalid".into());
            }
            job.url = new_url.to_owned();
            job.fallback_urls.clear();
            job.http_headers = headers;
            job.scheduled_at = None;
            state.storage.save_resume_job(&job)?;
            state.storage.update_download_url(id, new_url)?;
            crate::commands::resume_download(app.clone(), state, id).await?;
            Ok(json!({"ok":true,"id":id}))
        }
        "download" => {
            let job: DownloadJobRequest =
                serde_json::from_value(request).map_err(|e| e.to_string())?;
            let record = queue_download_request(app, &state, job).await?;
            Ok(json!({"ok":true,"id":record.id}))
        }
        _ => Err("Unknown browser action".into()),
    }
}

fn is_expired_media_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    ["4xx", "403", "404", "410", "expired"].iter().any(|needle| error.contains(needle))
}

fn same_media_asset(old_url: &str, new_url: &str) -> bool {
    let (Ok(old), Ok(new)) = (reqwest::Url::parse(old_url), reqwest::Url::parse(new_url)) else { return false };
    matches!(old.scheme(), "http" | "https") && matches!(new.scheme(), "http" | "https")
        && old != new && old.path() == new.path()
        && (old.host_str() == new.host_str() || old.path().split('/').filter(|part| !part.is_empty()).count() >= 3)
}

#[cfg(test)]
mod refresh_tests {
    use super::*;

    #[test]
    fn only_a_new_url_for_the_same_media_is_accepted() {
        assert!(same_media_asset("https://cdn.example/videos/42/master.m3u8?token=old", "https://cdn.example/videos/42/master.m3u8?token=new"));
        assert!(same_media_asset("https://cdn-a.example/videos/42/master.m3u8?old", "https://cdn-b.example/videos/42/master.m3u8?new"));
        assert!(!same_media_asset("https://cdn.example/videos/42/master.m3u8", "https://cdn.example/videos/43/master.m3u8"));
        assert!(!same_media_asset("https://cdn.example/master.m3u8", "https://other.example/master.m3u8"));
        assert!(!same_media_asset("https://cdn.example/videos/42/master.m3u8", "file:///videos/42/master.m3u8"));
        assert!(is_expired_media_error("ffmpeg: Server returned 4XX Client Error"));
        assert!(!is_expired_media_error("Could not write output file"));
    }
}

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, HeaderMap,
    RANGE,
};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;

const SEGMENT_THRESHOLD_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SEGMENTS: usize = 4;
const HLS_CONTENT_TYPES: &[&str] = &[
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "audio/x-mpegurl",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadMetadata {
    pub source_url: String,
    pub suggested_file_name: String,
    pub content_length: Option<u64>,
    pub content_type: Option<String>,
    pub resumable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecord {
    pub id: i64,
    pub url: String,
    pub file_name: String,
    pub save_path: String,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub status: String,
    pub error_message: Option<String>,
    pub expected_checksum: Option<String>,
    pub actual_checksum: Option<String>,
    pub checksum_status: Option<String>,
    pub scheduled_at: Option<String>,
    pub bandwidth_limit_kbps: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SegmentedDownloadManifest {
    total_bytes: u64,
    segments: Vec<SegmentPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SegmentPart {
    index: usize,
    start: u64,
    end: u64,
    downloaded_bytes: u64,
}

struct BandwidthThrottle {
    bytes_per_second: u64,
    last_check: tokio::sync::Mutex<Instant>,
    bytes_since_check: tokio::sync::Mutex<u64>,
}

impl BandwidthThrottle {
    fn from_kbps(kbps: Option<u64>) -> Option<Arc<Self>> {
        kbps.filter(|&v| v > 0).map(|kbps| {
            Arc::new(Self {
                bytes_per_second: kbps * 1024,
                last_check: tokio::sync::Mutex::new(Instant::now()),
                bytes_since_check: tokio::sync::Mutex::new(0),
            })
        })
    }

    async fn acquire(&self, bytes: u64) {
        let mut since_check = self.bytes_since_check.lock().await;
        *since_check += bytes;

        if *since_check >= self.bytes_per_second {
            let mut last = self.last_check.lock().await;
            let elapsed = last.elapsed();
            if elapsed < Duration::from_secs(1) {
                tokio::time::sleep(Duration::from_secs(1) - elapsed).await;
            }
            *last = Instant::now();
            *since_check = 0;
        }
    }
}

#[derive(Clone)]
pub struct DownloadService {
    client: Client,
}

impl DownloadService {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn inspect_url(&self, raw_url: &str) -> Result<DownloadMetadata, String> {
        let url = validate_url(raw_url)?;

        if is_ytdlp_supported_page(url.as_str()) || is_ytdlp_supported_cdn(&url) {
            let file_name = derive_ytdlp_file_name(&url);
            return Ok(DownloadMetadata {
                source_url: url.as_str().to_string(),
                suggested_file_name: file_name,
                content_length: None,
                content_type: Some("video/mp4".to_string()),
                resumable: false,
            });
        }

        if looks_like_hls_url(&url) {
            return Ok(DownloadMetadata {
                source_url: url.as_str().to_string(),
                suggested_file_name: derive_hls_file_name(&url),
                content_length: None,
                content_type: Some("application/vnd.apple.mpegurl".to_string()),
                resumable: false,
            });
        }

        let head_response = self.client.head(url.clone()).send().await;
        let response = match head_response {
            Ok(response) if response.status().is_success() => response,
            _ => self
                .client
                .get(url.clone())
                .header(RANGE, "bytes=0-0")
                .send()
                .await
                .map_err(|error| format!("failed to inspect remote file: {error}"))?,
        };

        if !response.status().is_success() && response.status().as_u16() != 206 {
            return Err(format!(
                "remote server returned unexpected status while inspecting URL: {}",
                response.status()
            ));
        }

        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(is_hls_content_type)
            .unwrap_or(false)
        {
            return Ok(DownloadMetadata {
                source_url: url.as_str().to_string(),
                suggested_file_name: derive_hls_file_name(&url),
                content_length: None,
                content_type: Some("application/vnd.apple.mpegurl".to_string()),
                resumable: false,
            });
        }

        let headers = response.headers().clone();
        let content_length = parse_content_length(&headers);
        let resumable = headers
            .get(ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok())
            .map(|value| value != "none")
            .unwrap_or(false)
            || response.status().as_u16() == 206;

        Ok(DownloadMetadata {
            source_url: url.as_str().to_string(),
            suggested_file_name: derive_file_name(&url, &headers),
            content_length,
            content_type: headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned),
            resumable,
        })
    }

    pub fn reserve_target_path(&self, target_dir: &Path, suggested_name: &str) -> PathBuf {
        let base_path = target_dir.join(suggested_name);
        if !base_path.exists() {
            return base_path;
        }

        let stem = base_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "download".to_string());
        let extension = base_path
            .extension()
            .map(|s| format!(".{}", s.to_string_lossy()))
            .unwrap_or_default();

        for counter in 1..1000 {
            let candidate = target_dir.join(format!("{stem} ({counter}){extension}"));
            if !candidate.exists() {
                return candidate;
            }
        }

        base_path
    }

    pub async fn current_downloaded_bytes(&self, target_path: &Path) -> u64 {
        let partial = self.partial_path_for(target_path);
        fs::metadata(&partial)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    }

    pub fn partial_path_for(&self, target_path: &Path) -> PathBuf {
        target_path.with_extension("part")
    }

    pub fn manifest_path_for(&self, target_path: &Path) -> PathBuf {
        PathBuf::from(format!(
            "{}.segments.json",
            self.partial_path_for(target_path).display()
        ))
    }

    fn segment_path_for(&self, target_path: &Path, index: usize) -> PathBuf {
        PathBuf::from(format!(
            "{}.seg{}",
            self.partial_path_for(target_path).display(),
            index
        ))
    }

    pub async fn download_to_path(
        &self,
        raw_url: &str,
        target_path: &Path,
        requested_resume_from: u64,
        resumable_hint: bool,
        total_bytes_hint: Option<u64>,
        bandwidth_limit_kbps: Option<u64>,
        source_page_url: Option<&str>,
        format: Option<&str>,
        mut on_started: impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        mut on_progress: impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let url = validate_url(raw_url)?;
        let throttle = BandwidthThrottle::from_kbps(bandwidth_limit_kbps);

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("failed to create target directory: {error}"))?;
        }

        if let Some(ytdlp_url) = resolve_ytdlp_source(&url, source_page_url) {
            if let Ok(result) = self
                .download_with_ytdlp(&ytdlp_url, target_path, format, &mut on_started, &mut on_progress)
                .await
            {
                return Ok(result);
            }
        } else if is_ytdlp_supported_cdn(&url) {
            // CDN URL but no page URL available - try with source_page_url or CDN URL directly
            let fallback_url = source_page_url
                .and_then(|u| validate_url(u).ok())
                .unwrap_or_else(|| url.clone());
            if let Ok(result) = self
                .download_with_ytdlp(&fallback_url, target_path, format, &mut on_started, &mut on_progress)
                .await
            {
                return Ok(result);
            }
        }

        if looks_like_hls_url(&url) {
            return self
                .download_hls_to_path(&url, target_path, &mut on_started, &mut on_progress)
                .await;
        }

        if let Some(manifest) = self.load_segment_manifest(target_path).await? {
            return self
                .download_with_segments(
                    &url,
                    target_path,
                    manifest,
                    throttle,
                    &mut on_started,
                    &mut on_progress,
                )
                .await;
        }

        if resumable_hint
            && requested_resume_from == 0
            && total_bytes_hint.unwrap_or(0) >= SEGMENT_THRESHOLD_BYTES
        {
            let manifest = build_segment_manifest(total_bytes_hint.unwrap_or(0));
            return self
                .download_with_segments(
                    &url,
                    target_path,
                    manifest,
                    throttle,
                    &mut on_started,
                    &mut on_progress,
                )
                .await;
        }

        let mut request = self.client.get(url);
        if requested_resume_from > 0 {
            request = request.header(RANGE, format!("bytes={requested_resume_from}-"));
        }

        let response = request
            .send()
            .await
            .map_err(|error| format!("failed to start download: {error}"))?;

        if !response.status().is_success() && response.status().as_u16() != 206 {
            return Err(format!(
                "remote server returned unexpected status while downloading: {}",
                response.status()
            ));
        }

        let status_code = response.status().as_u16();
        let actual_resume_from = if requested_resume_from > 0 && status_code == 206 {
            requested_resume_from
        } else {
            0
        };
        let total_bytes = parse_content_length(response.headers());
        let temp_path = self.partial_path_for(target_path);
        let mut file = if actual_resume_from > 0 {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&temp_path)
                .await
                .map_err(|error| format!("failed to reopen partial download file: {error}"))?
        } else {
            fs::File::create(&temp_path)
                .await
                .map_err(|error| format!("failed to create temporary file: {error}"))?
        };
        let mut stream = response.bytes_stream();
        let mut downloaded_bytes = actual_resume_from;

        on_started(actual_resume_from, total_bytes.or(total_bytes_hint), 1)?;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("failed while streaming data: {error}"))?;
            if let Some(throttle) = throttle.as_ref() {
                throttle.acquire(chunk.len() as u64).await;
            }
            file.write_all(&chunk)
                .await
                .map_err(|error| format!("failed to write downloaded bytes: {error}"))?;
            downloaded_bytes += chunk.len() as u64;
            on_progress(downloaded_bytes, total_bytes.or(total_bytes_hint), 1)?;
        }

        file.flush()
            .await
            .map_err(|error| format!("failed to flush downloaded file: {error}"))?;

        fs::rename(&temp_path, target_path)
            .await
            .map_err(|error| format!("failed to finalize downloaded file: {error}"))?;

        Ok((downloaded_bytes, total_bytes.or(total_bytes_hint), 1))
    }

    async fn download_hls_to_path(
        &self,
        playlist_url: &Url,
        target_path: &Path,
        on_started: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        on_progress: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        ensure_ffmpeg_available()?;

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("failed to create target directory: {error}"))?;
        }

        let temp_path = self.partial_path_for(target_path);
        if fs::try_exists(&temp_path).await.unwrap_or(false) {
            let _ = fs::remove_file(&temp_path).await;
        }

        on_started(0, None, 1)?;

        let mut child = Command::new("ffmpeg")
            .arg("-y")
            .arg("-nostdin")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(playlist_url.as_str())
            .arg("-map")
            .arg("0:v?")
            .arg("-map")
            .arg("0:a?")
            .arg("-c")
            .arg("copy")
            .arg("-movflags")
            .arg("+faststart")
            .arg("-f")
            .arg("mp4")
            .arg(&temp_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start ffmpeg for HLS download: {error}"))?;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        let output = child.wait_with_output().map_err(|error| {
                            format!("failed to collect ffmpeg error output: {error}")
                        })?;
                        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                        let message = if stderr.is_empty() {
                            "ffmpeg could not finalize the HLS stream".to_string()
                        } else {
                            format!("ffmpeg failed: {stderr}")
                        };
                        return Err(message);
                    }
                    break;
                }
                Ok(None) => {
                    let downloaded_bytes = fs::metadata(&temp_path)
                        .await
                        .map(|metadata| metadata.len())
                        .unwrap_or(0);
                    on_progress(downloaded_bytes, None, 1)?;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Err(error) => {
                    return Err(format!("failed while polling ffmpeg process: {error}"));
                }
            }
        }

        let downloaded_bytes = fs::metadata(&temp_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| format!("failed to inspect downloaded HLS output: {error}"))?;

        fs::rename(&temp_path, target_path)
            .await
            .map_err(|error| format!("failed to finalize downloaded HLS file: {error}"))?;

        Ok((downloaded_bytes, None, 1))
    }

    pub async fn download_media_bundle_to_path(
        &self,
        raw_video_url: &str,
        raw_audio_url: &str,
        target_path: &Path,
        mut on_started: impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        mut on_progress: impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        ensure_ffmpeg_available()?;
        let video_url = validate_url(raw_video_url)?;
        let audio_url = validate_url(raw_audio_url)?;

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("failed to create target directory: {error}"))?;
        }

        let temp_path = self.partial_path_for(target_path);
        if fs::try_exists(&temp_path).await.unwrap_or(false) {
            let _ = fs::remove_file(&temp_path).await;
        }

        on_started(0, None, 2)?;

        let mut child = Command::new("ffmpeg")
            .arg("-y")
            .arg("-nostdin")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(video_url.as_str())
            .arg("-i")
            .arg(audio_url.as_str())
            .arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg("1:a:0")
            .arg("-c")
            .arg("copy")
            .arg("-movflags")
            .arg("+faststart")
            .arg("-f")
            .arg("mp4")
            .arg(&temp_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start ffmpeg for media merge: {error}"))?;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        let output = child.wait_with_output().map_err(|error| {
                            format!("failed to collect ffmpeg error output: {error}")
                        })?;
                        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                        let message = if stderr.is_empty() {
                            "ffmpeg could not merge the captured media streams".to_string()
                        } else {
                            format!("ffmpeg failed: {stderr}")
                        };
                        return Err(message);
                    }
                    break;
                }
                Ok(None) => {
                    let downloaded_bytes = fs::metadata(&temp_path)
                        .await
                        .map(|metadata| metadata.len())
                        .unwrap_or(0);
                    on_progress(downloaded_bytes, None, 2)?;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Err(error) => {
                    return Err(format!("failed while polling ffmpeg process: {error}"));
                }
            }
        }

        let downloaded_bytes = fs::metadata(&temp_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| format!("failed to inspect merged media output: {error}"))?;

        fs::rename(&temp_path, target_path)
            .await
            .map_err(|error| format!("failed to finalize merged media file: {error}"))?;

        Ok((downloaded_bytes, None, 2))
    }

    async fn download_with_ytdlp(
        &self,
        url: &Url,
        target_path: &Path,
        format: Option<&str>,
        on_started: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        on_progress: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let ytdlp_path = resolve_ytdlp_path().ok_or("yt-dlp is not available")?;

        let download_dir = target_path
            .parent()
            .ok_or("failed to resolve download directory")?;
        let ytdlp_template = download_dir.join("%(title).100s [%(id)s].%(ext)s");

        on_started(0, None, 1)?;

        let format_spec = format.unwrap_or("bv*+ba/b");
        let needs_cookies = url.host_str().map(|h|
            h.contains("facebook.com") || h.contains("fb.watch") || h.contains("instagram.com")
        ).unwrap_or(false);
        let mut cmd = clean_env_command(&ytdlp_path);
        cmd.arg("--no-warnings")
            .arg("--no-playlist")
            .arg("-f")
            .arg(format_spec)
            .arg("--merge-output-format")
            .arg("mp4");
        if needs_cookies {
            cmd.arg("--cookies-from-browser")
                .arg(detect_browser_for_cookies());
        }
        cmd.arg("-o")
            .arg(&ytdlp_template)
            .arg("--print")
            .arg("after_move:filepath")
            .arg(url.as_str())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|error| format!("failed to start yt-dlp: {error}"))?;

        let output = child
            .wait_with_output()
            .map_err(|error| format!("failed to run yt-dlp: {error}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                "yt-dlp could not download the video".to_string()
            } else {
                format!("yt-dlp failed: {stderr}")
            };
            return Err(message);
        }

        let actual_path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let actual_path = PathBuf::from(&actual_path_str);

        if actual_path_str.is_empty() {
            return Err("yt-dlp completed but did not report output file".to_string());
        }

        // yt-dlp may print multiple lines, take the last non-empty line
        let final_file = actual_path_str
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or(&actual_path_str);
        let actual_path = PathBuf::from(final_file.trim());

        if !actual_path.exists() {
            return Err(format!("yt-dlp output file not found: {}", actual_path.display()));
        }

        let downloaded_bytes = fs::metadata(&actual_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| format!("failed to inspect yt-dlp output: {error}"))?;

        // Rename target_path to match actual yt-dlp output name in DB
        let actual_name = actual_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if target_path.exists() && target_path != actual_path {
            let _ = fs::remove_file(target_path).await;
        }

        on_progress(downloaded_bytes, Some(downloaded_bytes), 1)?;
        Ok((downloaded_bytes, Some(downloaded_bytes), 1))
    }

    pub async fn remove_temp_artifacts(&self, target_path: &Path) -> Result<(), String> {
        let partial_path = self.partial_path_for(target_path);
        if fs::try_exists(&partial_path).await.unwrap_or(false) {
            let _ = fs::remove_file(&partial_path).await;
        }

        let ytdlp_temp = target_path.with_extension("ytdlp.mp4");
        if fs::try_exists(&ytdlp_temp).await.unwrap_or(false) {
            let _ = fs::remove_file(&ytdlp_temp).await;
        }

        if let Some(manifest) = self.load_segment_manifest(target_path).await? {
            for segment in manifest.segments {
                let segment_path = self.segment_path_for(target_path, segment.index);
                if fs::try_exists(&segment_path).await.unwrap_or(false) {
                    let _ = fs::remove_file(segment_path).await;
                }
            }
        }

        let manifest_path = self.manifest_path_for(target_path);
        if fs::try_exists(&manifest_path).await.unwrap_or(false) {
            let _ = fs::remove_file(manifest_path).await;
        }

        Ok(())
    }

    pub async fn compute_sha256(&self, path: &Path) -> Result<String, String> {
        let mut file = fs::File::open(path)
            .await
            .map_err(|error| format!("failed to open file for checksum: {error}"))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let bytes_read = file
                .read(&mut buffer)
                .await
                .map_err(|error| format!("failed to read file for checksum: {error}"))?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    async fn load_segment_manifest(
        &self,
        target_path: &Path,
    ) -> Result<Option<SegmentedDownloadManifest>, String> {
        let manifest_path = self.manifest_path_for(target_path);
        if !fs::try_exists(&manifest_path).await.unwrap_or(false) {
            return Ok(None);
        }
        let contents = fs::read_to_string(&manifest_path)
            .await
            .map_err(|error| format!("failed to read segment manifest: {error}"))?;
        let manifest: SegmentedDownloadManifest = serde_json::from_str(&contents)
            .map_err(|error| format!("failed to parse segment manifest: {error}"))?;
        Ok(Some(manifest))
    }

    async fn save_segment_manifest(
        &self,
        target_path: &Path,
        manifest: &SegmentedDownloadManifest,
    ) -> Result<(), String> {
        let manifest_path = self.manifest_path_for(target_path);
        let contents = serde_json::to_string_pretty(manifest)
            .map_err(|error| format!("failed to serialize segment manifest: {error}"))?;
        fs::write(&manifest_path, contents)
            .await
            .map_err(|error| format!("failed to write segment manifest: {error}"))?;
        Ok(())
    }

    async fn download_with_segments(
        &self,
        url: &Url,
        target_path: &Path,
        manifest: SegmentedDownloadManifest,
        throttle: Option<Arc<BandwidthThrottle>>,
        on_started: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        on_progress: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let total_bytes = manifest.total_bytes;
        let segment_count = manifest.segments.len();

        self.save_segment_manifest(target_path, &manifest).await?;

        let initial_bytes: u64 = manifest.segments.iter().map(|s| s.downloaded_bytes).sum();
        on_started(initial_bytes, Some(total_bytes), segment_count)?;

        let progress = Arc::new(AsyncMutex::new(initial_bytes));
        let (tx, mut rx) = mpsc::channel::<u64>(64);

        let mut futures = FuturesUnordered::new();
        for segment in &manifest.segments {
            let client = self.client.clone();
            let url = url.clone();
            let seg_path = self.segment_path_for(target_path, segment.index);
            let start = segment.start + segment.downloaded_bytes;
            let end = segment.end;
            let tx = tx.clone();
            let throttle = throttle.clone();

            if segment.downloaded_bytes >= (segment.end - segment.start + 1) {
                continue;
            }

            futures.push(tokio::spawn(async move {
                let mut request = client.get(url.as_str());
                request = request.header(RANGE, format!("bytes={start}-{end}"));

                let response = request
                    .send()
                    .await
                    .map_err(|error| format!("segment download failed: {error}"))?;

                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&seg_path)
                    .await
                    .map_err(|error| format!("failed to open segment file: {error}"))?;

                let mut stream = response.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk =
                        chunk.map_err(|error| format!("segment stream error: {error}"))?;
                    if let Some(ref throttle) = throttle {
                        throttle.acquire(chunk.len() as u64).await;
                    }
                    file.write_all(&chunk)
                        .await
                        .map_err(|error| format!("failed to write segment: {error}"))?;
                    let _ = tx.send(chunk.len() as u64).await;
                }
                file.flush()
                    .await
                    .map_err(|error| format!("failed to flush segment: {error}"))?;
                Ok::<(), String>(())
            }));
        }
        drop(tx);

        let progress_task = {
            let progress = progress.clone();
            tokio::spawn(async move {
                while let Some(bytes) = rx.recv().await {
                    let mut p = progress.lock().await;
                    *p += bytes;
                }
            })
        };

        while let Some(result) = futures.next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error),
                Err(error) => return Err(format!("segment task panicked: {error}")),
            }
            let current = *progress.lock().await;
            on_progress(current, Some(total_bytes), segment_count)?;
        }

        progress_task.abort();

        let temp_path = self.partial_path_for(target_path);
        let mut output = fs::File::create(&temp_path)
            .await
            .map_err(|error| format!("failed to create merged output: {error}"))?;

        for segment in &manifest.segments {
            let seg_path = self.segment_path_for(target_path, segment.index);
            let mut seg_file = fs::File::open(&seg_path)
                .await
                .map_err(|error| format!("failed to open segment for merging: {error}"))?;
            tokio::io::copy(&mut seg_file, &mut output)
                .await
                .map_err(|error| format!("failed to merge segment: {error}"))?;
            let _ = fs::remove_file(&seg_path).await;
        }

        output.flush().await.map_err(|error| format!("failed to flush merged output: {error}"))?;

        fs::rename(&temp_path, target_path)
            .await
            .map_err(|error| format!("failed to finalize merged download: {error}"))?;

        let manifest_path = self.manifest_path_for(target_path);
        let _ = fs::remove_file(&manifest_path).await;

        let final_bytes = *progress.lock().await;
        Ok((final_bytes, Some(total_bytes), segment_count))
    }
}

fn detect_browser_for_cookies() -> &'static str {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates: &[(&str, &str)] = &[
        ("chrome", ".config/google-chrome"),
        ("chromium", ".config/chromium"),
        ("brave", ".config/BraveSoftware/Brave-Browser"),
        ("vivaldi", ".config/vivaldi"),
        ("edge", ".config/microsoft-edge"),
        ("firefox", ".mozilla/firefox"),
    ];
    for (name, path) in candidates {
        let full_path = format!("{home}/{path}/Default/Cookies");
        let alt_path = format!("{home}/{path}");
        if std::path::Path::new(&full_path).exists()
            || (*name == "firefox" && std::path::Path::new(&alt_path).is_dir())
        {
            return name;
        }
    }
    "chrome"
}

fn clean_env_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_remove("LD_LIBRARY_PATH");
    cmd.env_remove("LD_PRELOAD");
    cmd.env_remove("PYTHONPATH");
    cmd.env_remove("PYTHONHOME");
    cmd.env_remove("QT_PLUGIN_PATH");
    cmd.env_remove("GST_PLUGIN_SYSTEM_PATH");
    cmd.env_remove("APPDIR");
    cmd.env_remove("APPIMAGE");
    cmd.env_remove("OWD");
    if let Ok(home) = std::env::var("HOME") {
        let clean_path = format!("{home}/.local/bin:/usr/local/bin:/usr/bin:/bin");
        cmd.env("PATH", clean_path);
    }
    cmd
}

fn resolve_ytdlp_path() -> Option<String> {
    // Check ~/.local/bin/yt-dlp first (most common for AppImage installs)
    let home = std::env::var("HOME").unwrap_or_default();
    let local_path = format!("{home}/.local/bin/yt-dlp");
    if std::path::Path::new(&local_path).exists() {
        return Some(local_path);
    }

    // Check system PATH
    let candidates = ["yt-dlp"];
    for candidate in &candidates {
        if clean_env_command(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Some(candidate.to_string());
        }
    }

    let local_path2 = format!("{home}/.local/bin/yt-dlp");
    if clean_env_command(&local_path2)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
    {
        return Some(local_path);
    }

    None
}

fn extract_youtube_page_url(cdn_url: &Url) -> Option<Url> {
    // Try to get video ID from the 'video_id' or 'id' parameter
    let video_id = cdn_url.query_pairs()
        .find(|(k, _)| k == "video_id")
        .map(|(_, v)| v.to_string())
        .or_else(|| {
            // YouTube CDN URLs sometimes have 'id' param like 'o-XXXX'
            // but the actual video ID comes from the page URL
            None
        });

    if let Some(id) = video_id {
        return Url::parse(&format!("https://www.youtube.com/watch?v={id}")).ok();
    }

    None
}

fn extract_facebook_page_url(cdn_url: &Url) -> Option<Url> {
    let efg_raw = cdn_url.query_pairs().find(|(key, _)| key == "efg")?.1;
    let normalized = efg_raw.replace('-', "+").replace('_', "/");
    let padding = "=".repeat((4 - (normalized.len() % 4)) % 4);
    let encoded = format!("{normalized}{padding}");
    let decoded = base64_decode(&encoded)?;
    let text = std::str::from_utf8(&decoded).ok()?;
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let video_id = json.get("video_id")?.as_u64()?;
    Url::parse(&format!("https://www.facebook.com/watch/?v={video_id}")).ok()
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = TABLE.iter().position(|&c| c == byte)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(output)
}

fn resolve_ytdlp_source(cdn_url: &Url, source_page_url: Option<&str>) -> Option<Url> {
    if resolve_ytdlp_path().is_none() {
        return None;
    }

    if let Some(page_url) = source_page_url {
        if is_ytdlp_supported_page(page_url) {
            return validate_url(page_url).ok();
        }
    }

    let host = cdn_url.host_str().unwrap_or("");

    if host.ends_with(".fbcdn.net")
        || host.ends_with(".facebook.com")
        || host.ends_with(".cdninstagram.com")
    {
        return extract_facebook_page_url(cdn_url).or_else(|| Some(cdn_url.clone()));
    }

    if host.ends_with(".twimg.com") {
        return Some(cdn_url.clone());
    }

    // Reddit: yt-dlp ile değil, doğrudan HLS/mp4 olarak indir

    if host.ends_with(".googlevideo.com")
        || host.ends_with(".youtube.com")
        || host.ends_with(".ytimg.com")
    {
        return extract_youtube_page_url(cdn_url).or_else(|| source_page_url.and_then(|u| validate_url(u).ok()));
    }

    None
}

fn derive_ytdlp_file_name(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    let prefix = if host.contains("youtube.com") || host.contains("youtu.be") {
        "youtube"
    } else if host.contains("x.com") || host.contains("twitter.com") {
        "twitter"
    } else if host.contains("reddit.com") {
        "reddit"
    } else if host.contains("facebook.com") || host.contains("fb.watch") {
        "facebook"
    } else if host.contains("instagram.com") {
        "instagram"
    } else {
        "video"
    };

    let id = url
        .query_pairs()
        .find(|(key, _)| key == "v")
        .map(|(_, value)| value.to_string())
        .or_else(|| {
            url.path_segments()
                .and_then(|segments| segments.last().map(String::from))
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "download".to_string());

    format!("{prefix}_{id}.mp4")
}

fn is_ytdlp_supported_cdn(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("");
    host.ends_with(".googlevideo.com")
        || host.ends_with(".ytimg.com")
        || host.ends_with(".fbcdn.net")
        || host.ends_with(".cdninstagram.com")
        || host.ends_with(".twimg.com")
}

fn is_ytdlp_supported_page(page_url: &str) -> bool {
    let dominated_by = |domain: &str| -> bool {
        page_url.contains(domain)
    };
    dominated_by("facebook.com/")
        || dominated_by("fb.watch/")
        || dominated_by("instagram.com/")
        || dominated_by("x.com/")
        || dominated_by("twitter.com/")
        || dominated_by("youtube.com/")
        || dominated_by("youtu.be/")
}

fn looks_like_hls_url(url: &Url) -> bool {
    url.path().to_ascii_lowercase().ends_with(".m3u8")
}

fn is_hls_content_type(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    HLS_CONTENT_TYPES
        .iter()
        .any(|candidate| normalized.contains(candidate))
}

fn parse_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            headers
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split('/').nth(1))
                .and_then(|value| value.parse::<u64>().ok())
        })
}

fn derive_file_name(url: &Url, headers: &HeaderMap) -> String {
    let inferred_extension = infer_extension(url, headers);

    headers
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| extract_filename_from_disposition(value))
        .or_else(|| {
            url.path_segments()
                .and_then(|segments| segments.last())
                .filter(|name| !name.is_empty())
                .map(|name| urlencoding::decode(name).unwrap_or_else(|_| name.into()).into_owned())
        })
        .map(|name| {
            if let Some(ref ext) = inferred_extension {
                if !name.contains('.') {
                    return format!("{name}.{ext}");
                }
            }
            name
        })
        .unwrap_or_else(|| {
            let ext = inferred_extension.unwrap_or_else(|| "bin".to_string());
            format!("download.{ext}")
        })
}

fn derive_hls_file_name(url: &Url) -> String {
    let base = url
        .path_segments()
        .and_then(|segments| segments.last())
        .filter(|name| !name.is_empty())
        .map(|name| {
            let decoded = urlencoding::decode(name).unwrap_or_else(|_| name.into());
            decoded
                .strip_suffix(".m3u8")
                .or_else(|| decoded.strip_suffix(".m3u"))
                .unwrap_or(&decoded)
                .to_string()
        })
        .unwrap_or_else(|| "stream".to_string());
    format!("{base}.mp4")
}

fn extract_filename_from_disposition(value: &str) -> Option<String> {
    if let Some(start) = value.find("filename*=") {
        let rest = &value[start + 10..];
        let encoded = rest
            .split(';')
            .next()?
            .trim()
            .trim_matches('"');
        if let Some(pos) = encoded.find("''") {
            let decoded = urlencoding::decode(&encoded[pos + 2..])
                .unwrap_or_else(|_| encoded[pos + 2..].into());
            return Some(decoded.into_owned());
        }
    }

    if let Some(start) = value.find("filename=") {
        let rest = &value[start + 9..];
        let name = rest
            .split(';')
            .next()?
            .trim()
            .trim_matches('"');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }

    None
}

fn infer_extension(url: &Url, headers: &HeaderMap) -> Option<String> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim());

    match content_type {
        Some("video/mp4") => Some("mp4".to_string()),
        Some("video/webm") => Some("webm".to_string()),
        Some("video/x-matroska") => Some("mkv".to_string()),
        Some("audio/mpeg") => Some("mp3".to_string()),
        Some("audio/mp4") | Some("audio/x-m4a") => Some("m4a".to_string()),
        Some("audio/aac") => Some("aac".to_string()),
        Some("audio/ogg") => Some("ogg".to_string()),
        Some("application/pdf") => Some("pdf".to_string()),
        Some("application/zip") => Some("zip".to_string()),
        Some("application/x-7z-compressed") => Some("7z".to_string()),
        Some("application/x-tar") => Some("tar".to_string()),
        Some("application/gzip") => Some("gz".to_string()),
        Some("application/x-xz") => Some("xz".to_string()),
        Some("application/x-bzip2") => Some("bz2".to_string()),
        Some("application/x-rar-compressed") => Some("rar".to_string()),
        Some("application/octet-stream") | None => {
            let path = url.path().to_lowercase();
            let ext = path.rsplit('.').next().unwrap_or("");
            match ext {
                "mp4" | "mkv" | "webm" | "mov" | "avi" | "mp3" | "m4a" | "aac" | "ogg"
                | "wav" | "pdf" | "zip" | "7z" | "tar" | "gz" | "xz" | "bz2" | "rar"
                | "exe" | "msi" | "deb" | "rpm" | "appimage" | "iso" | "img" | "dmg"
                | "pkg" | "apk" | "csv" | "epub" | "torrent" => Some(ext.to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn validate_url(raw_url: &str) -> Result<Url, String> {
    Url::parse(raw_url).map_err(|error| format!("invalid URL: {error}"))
}

fn ensure_ffmpeg_available() -> Result<(), String> {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            format!("ffmpeg is required for HLS downloads but could not be started: {error}")
        })
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err("ffmpeg is required for HLS downloads but is not available".to_string())
            }
        })
}

fn build_segment_manifest(total_bytes: u64) -> SegmentedDownloadManifest {
    let segment_count = determine_segment_count(total_bytes);
    let mut segments = Vec::with_capacity(segment_count);
    let base_size = total_bytes / segment_count as u64;
    let mut start = 0_u64;

    for index in 0..segment_count {
        let mut end = start + base_size.saturating_sub(1);
        if index == segment_count - 1 {
            end = total_bytes.saturating_sub(1);
        }

        segments.push(SegmentPart {
            index,
            start,
            end,
            downloaded_bytes: 0,
        });

        start = end.saturating_add(1);
    }

    SegmentedDownloadManifest {
        total_bytes,
        segments,
    }
}

fn determine_segment_count(total_bytes: u64) -> usize {
    if total_bytes >= 128 * 1024 * 1024 {
        4
    } else if total_bytes >= 48 * 1024 * 1024 {
        3
    } else {
        2
    }
}

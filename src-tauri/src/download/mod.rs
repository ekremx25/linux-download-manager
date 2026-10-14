mod hls;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, HeaderMap,
    RANGE,
};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, BufWriter};
use tokio::sync::watch;

/// Byte counts always refer to the current transfer (video/audio may be separate).
#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub active_parts: usize,
    pub speed_bytes_per_second: Option<u64>,
    pub eta_seconds: Option<u64>,
}

impl DownloadProgress {
    fn bytes(downloaded_bytes: u64, total_bytes: Option<u64>, active_parts: usize) -> Self {
        Self {
            downloaded_bytes,
            total_bytes,
            active_parts,
            speed_bytes_per_second: None,
            eta_seconds: None,
        }
    }
}

const SEGMENT_THRESHOLD_BYTES: u64 = 8 * 1024 * 1024;
/// A single flaky range request must not abandon the whole download: mirrors
/// routinely drop some of several parallel range connections.
const SEGMENT_MAX_ATTEMPTS: u32 = 3;
const SEGMENT_RETRY_BASE_DELAY: Duration = Duration::from_millis(500);
const DYNAMIC_SPLIT_MIN_REMAINING: u64 = 2 * 1024 * 1024;
/// Used when the browser extension captured no User-Agent for a media request.
const FALLBACK_MEDIA_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const STREAM_MANIFEST_CONTENT_TYPES: &[&str] = &[
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "audio/x-mpegurl",
    "application/dash+xml",
];

fn hidden_work_dir(target: &Path, kind: &str) -> PathBuf {
    let digest = format!("{:x}", Sha256::digest(target.as_os_str().as_encoded_bytes()));
    target
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".ldm-{kind}-{}", &digest[..20]))
}

fn legacy_ytdlp_sidecar(name: &str, target_name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(target_name) else {
        return false;
    };
    suffix == ".part"
        || suffix == ".ytdl"
        || suffix.strip_prefix(".part-Frag").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        || (suffix.starts_with(".f") && (suffix.ends_with(".part") || suffix.ends_with(".ytdl") || suffix.contains(".part-Frag")))
}

async fn legacy_ytdlp_sidecars(target: &Path) -> Result<Vec<PathBuf>, String> {
    let Some(parent) = target.parent() else { return Ok(Vec::new()); };
    let Some(target_name) = target.file_name().and_then(|name| name.to_str()) else { return Ok(Vec::new()); };
    let mut entries = match fs::read_dir(parent).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut sidecars = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|error| error.to_string())? {
        if legacy_ytdlp_sidecar(&entry.file_name().to_string_lossy(), target_name)
            && entry.file_type().await.map_err(|error| error.to_string())?.is_file()
        {
            sidecars.push(entry.path());
        }
    }
    Ok(sidecars)
}

struct KillOnDropChild {
    child: Option<Child>,
}

impl KillOnDropChild {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.as_mut().unwrap().try_wait()
    }

    fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.as_mut().and_then(|child| child.stderr.take())
    }
}

impl Drop for KillOnDropChild {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadMetadata {
    pub source_url: String,
    pub suggested_file_name: String,
    pub content_length: Option<u64>,
    pub content_type: Option<String>,
    pub resumable: bool,
}

/// Tunables for a single download. Grouped into one struct so
/// `download_to_path` stays inside the clippy argument budget, and so a call
/// site cannot swap two adjacent `Option<u64>` fields by accident.
#[derive(Debug)]
pub struct DownloadOptions<'a> {
    pub transfer: crate::preferences::TransferPreferences,
    pub requested_resume_from: u64,
    pub resumable_hint: bool,
    pub total_bytes_hint: Option<u64>,
    pub bandwidth_limit_kbps: Option<u64>,
    pub source_page_url: Option<&'a str>,
    pub http_headers: &'a HashMap<String, String>,
    pub format: Option<&'a str>,
    pub force_ytdlp: bool,
    pub stream_manifest: bool,
    pub fallback_urls: &'a [String],
}

struct SingleOptions {
    resume_from: u64,
    total_bytes: Option<u64>,
    throttle: Option<Arc<BandwidthThrottle>>,
}
struct StreamOptions<'a> {
    source_page_url: Option<&'a str>,
    http_headers: &'a HashMap<String, String>,
    transfer: &'a crate::preferences::TransferPreferences,
}
struct YtdlpOptions<'a> {
    format: Option<&'a str>,
    use_browser_cookies: bool,
    transfer: &'a crate::preferences::TransferPreferences,
    bandwidth_limit_kbps: Option<u64>,
    http_headers: Option<&'a HashMap<String, String>>,
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
    /// Library category decided by the backend; the UI only renders it.
    pub category: String,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSegmentDetail {
    pub index: usize,
    pub status: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub start: u64,
    pub end: u64,
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
        self.inspect_url_with_headers(raw_url, &HashMap::new())
            .await
    }
    pub async fn inspect_url_with_headers(
        &self,
        raw_url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<DownloadMetadata, String> {
        let service = self.with_headers(headers)?;
        service.inspect_inner(raw_url).await
    }
    fn with_headers(&self, headers: &HashMap<String, String>) -> Result<Self, String> {
        if headers.is_empty() {
            return Ok(self.clone());
        }
        let mut defaults = HeaderMap::new();
        for (k, v) in headers {
            if matches!(
                k.to_ascii_lowercase().as_str(),
                "cookie"
                    | "referer"
                    | "origin"
                    | "user-agent"
                    | "authorization"
                    | "accept"
                    | "accept-language"
            ) {
                defaults.insert(
                    reqwest::header::HeaderName::from_bytes(k.as_bytes())
                        .map_err(|e| e.to_string())?,
                    reqwest::header::HeaderValue::from_str(v).map_err(|e| e.to_string())?,
                );
            }
        }
        let client = Client::builder()
            .default_headers(defaults)
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self::new(client))
    }
    async fn inspect_inner(&self, raw_url: &str) -> Result<DownloadMetadata, String> {
        let url = validate_url(raw_url)?;

        if is_ytdlp_supported_page(url.as_str()) || is_ytdlp_supported_cdn(&url) || is_embedded_player_page(&url) {
            let file_name = derive_ytdlp_file_name(&url);
            return Ok(DownloadMetadata {
                source_url: url.as_str().to_string(),
                suggested_file_name: file_name,
                content_length: None,
                content_type: Some("video/mp4".to_string()),
                resumable: false,
            });
        }

        if looks_like_stream_manifest_url(&url) {
            return Ok(DownloadMetadata {
                source_url: url.as_str().to_string(),
                suggested_file_name: derive_stream_file_name(&url),
                content_length: None,
                content_type: Some("application/stream-manifest".to_string()),
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
            .map(is_stream_manifest_content_type)
            .unwrap_or(false)
        {
            return Ok(DownloadMetadata {
                source_url: url.as_str().to_string(),
                suggested_file_name: derive_stream_file_name(&url),
                content_length: None,
                content_type: Some("application/stream-manifest".to_string()),
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
        fs::metadata(&partial).await.map(|m| m.len()).unwrap_or(0)
    }

    pub fn partial_path_for(&self, target_path: &Path) -> PathBuf {
        let current = PathBuf::from(format!("{}.part", target_path.display()));
        let legacy = target_path.with_extension("part");
        let legacy_manifest = PathBuf::from(format!("{}.segments.json", legacy.display()));
        if !current.exists() && (legacy.exists() || legacy_manifest.exists()) {
            legacy
        } else {
            current
        }
    }

    pub fn manifest_path_for(&self, target_path: &Path) -> PathBuf {
        PathBuf::from(format!(
            "{}.segments.json",
            self.partial_path_for(target_path).display()
        ))
    }

    /// Per-segment progress read from the segment files that are actually on
    /// disk, returning `(bytes_on_disk, is_complete)`.
    ///
    /// The manifest is only a plan, never the source of truth for progress.
    /// `download_with_segments` deletes each segment file as it copies it into
    /// the merged output, so a merge that fails part way through leaves the
    /// manifest claiming a segment is complete while its file is gone. Trusting
    /// the manifest then skips that segment and the merge fails later with
    /// "failed to open segment for merging". The same trust also corrupts data
    /// in the other direction: resuming from a stale offset while appending to
    /// an existing part file duplicates bytes. File size on disk is the only
    /// truthful record of what has been fetched.
    async fn segment_progress_on_disk(
        &self,
        target_path: &Path,
        segment: &SegmentPart,
    ) -> (u64, bool) {
        let expected = segment.end.saturating_sub(segment.start).saturating_add(1);
        let on_disk = fs::metadata(self.segment_path_for(target_path, segment.index))
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0)
            .min(expected);
        (on_disk, on_disk >= expected)
    }

    fn segment_path_for(&self, target_path: &Path, index: usize) -> PathBuf {
        PathBuf::from(format!(
            "{}.seg{}",
            self.partial_path_for(target_path).display(),
            index
        ))
    }

    pub async fn segment_details(
        &self,
        target_path: &Path,
        status: &str,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    ) -> Result<Vec<DownloadSegmentDetail>, String> {
        if let Some(manifest) = self.load_segment_manifest(target_path).await? {
            let mut details = Vec::with_capacity(manifest.segments.len());
            for segment in manifest.segments {
                let total = segment.end.saturating_sub(segment.start).saturating_add(1);
                let on_disk = fs::metadata(self.segment_path_for(target_path, segment.index))
                    .await
                    .map(|metadata| metadata.len())
                    .unwrap_or(segment.downloaded_bytes);
                let downloaded = on_disk.min(total);
                let part_status = if downloaded >= total {
                    "completed"
                } else if status == "in_progress" {
                    "receiving"
                } else {
                    status
                };
                details.push(DownloadSegmentDetail {
                    index: segment.index,
                    status: part_status.to_string(),
                    downloaded_bytes: downloaded,
                    total_bytes: total,
                    start: segment.start,
                    end: segment.end,
                });
            }
            details.sort_by_key(|part| part.start);
            return Ok(details);
        }

        let total = total_bytes.unwrap_or(downloaded_bytes);
        if total == 0 && downloaded_bytes == 0 {
            return Ok(Vec::new());
        }
        Ok(vec![DownloadSegmentDetail {
            index: 0,
            status: status.to_string(),
            downloaded_bytes: downloaded_bytes.min(total.max(downloaded_bytes)),
            total_bytes: total.max(downloaded_bytes),
            start: 0,
            end: total.saturating_sub(1),
        }])
    }

    pub async fn download_to_path(
        &self,
        raw_url: &str,
        target_path: &Path,
        options: DownloadOptions<'_>,
        mut on_started: impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        mut on_progress: impl FnMut(DownloadProgress) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let DownloadOptions {
            transfer,
            requested_resume_from,
            resumable_hint,
            total_bytes_hint,
            bandwidth_limit_kbps,
            source_page_url,
            http_headers,
            format,
            force_ytdlp,
            stream_manifest,
            fallback_urls,
        } = options;

        let url = validate_url(raw_url)?;
        let throttle = BandwidthThrottle::from_kbps(bandwidth_limit_kbps);

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("failed to create target directory: {error}"))?;
        }

        if force_ytdlp || transfer.audio_language.is_some() {
            let media_url = resolve_ytdlp_source(&url, source_page_url).unwrap_or_else(|| url.clone());
            return self
                .download_with_ytdlp(
                    &media_url,
                    target_path,
                    YtdlpOptions {
                        format,
                        use_browser_cookies: false,
                        transfer: &transfer,
                        bandwidth_limit_kbps,
                        http_headers: Some(http_headers),
                    },
                    &mut on_started,
                    &mut on_progress,
                )
                .await;
        }

        // Most embed URLs are HTML pages. A response verified by the browser
        // as an HLS playlist is an exception, even when served from /embed/.
        if is_embedded_player_page(&url) && !stream_manifest {
            return self
                .download_with_ytdlp(
                    &url,
                    target_path,
                    YtdlpOptions {
                        format,
                        use_browser_cookies: false,
                        transfer: &transfer,
                        bandwidth_limit_kbps,
                        http_headers: Some(http_headers),
                    },
                    &mut on_started,
                    &mut on_progress,
                )
                .await;
        }

        if let Some(ytdlp_url) = resolve_ytdlp_source(&url, source_page_url) {
            if let Ok(result) = self
                .download_with_ytdlp(
                    &ytdlp_url,
                    target_path,
                    YtdlpOptions {
                        format,
                        use_browser_cookies: false,
                        transfer: &transfer,
                        bandwidth_limit_kbps,
                        http_headers: Some(http_headers),
                    },
                    &mut on_started,
                    &mut on_progress,
                )
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
                .download_with_ytdlp(
                    &fallback_url,
                    target_path,
                    YtdlpOptions {
                        format,
                        use_browser_cookies: false,
                        transfer: &transfer,
                        bandwidth_limit_kbps,
                        http_headers: Some(http_headers),
                    },
                    &mut on_started,
                    &mut on_progress,
                )
                .await
            {
                return Ok(result);
            }
        }

        if stream_manifest || looks_like_stream_manifest_url(&url) {
            if transfer.connections > 1 && !is_embedded_player_page(&url) {
                let options = YtdlpOptions {
                    format,
                    use_browser_cookies: false,
                    transfer: &transfer,
                    bandwidth_limit_kbps,
                    http_headers: Some(http_headers),
                };
                if let Ok(result) = self
                    .download_with_ytdlp(
                        &url,
                        target_path,
                        options,
                        &mut on_started,
                        &mut on_progress,
                    )
                    .await
                {
                    return Ok(result);
                }
                let _ = fs::remove_file(media_progress_path(target_path)).await;
            }
            let mut playlist_urls = vec![url.clone()];
            for candidate in fallback_urls {
                if let Ok(candidate_url) = validate_url(candidate)
                    && looks_like_stream_manifest_url(&candidate_url)
                    && !playlist_urls.iter().any(|known| known == &candidate_url)
                {
                    playlist_urls.push(candidate_url);
                }
            }
            return self
                .download_stream_manifest_to_path(
                    &playlist_urls,
                    target_path,
                    StreamOptions {
                        source_page_url,
                        http_headers,
                        transfer: &transfer,
                    },
                    &mut on_started,
                    &mut on_progress,
                )
                .await;
        }

        let http_service = self.with_headers(http_headers)?;
        if let Some(manifest) = self.load_segment_manifest(target_path).await? {
            return http_service
                .download_with_segments(
                    &url,
                    target_path,
                    manifest,
                    transfer.connections,
                    throttle,
                    &mut on_started,
                    &mut on_progress,
                )
                .await;
        }

        if resumable_hint
            && requested_resume_from == 0
            && transfer.connections != 1
            && total_bytes_hint.unwrap_or(0) >= SEGMENT_THRESHOLD_BYTES
        {
            let manifest = build_segment_manifest_with_count(
                total_bytes_hint.unwrap_or(0),
                transfer.connections,
            );
            return http_service
                .download_with_segments(
                    &url,
                    target_path,
                    manifest,
                    transfer.connections,
                    throttle,
                    &mut on_started,
                    &mut on_progress,
                )
                .await;
        }

        http_service
            .download_single(
                &url,
                target_path,
                SingleOptions {
                    resume_from: requested_resume_from,
                    total_bytes: total_bytes_hint,
                    throttle,
                },
                &mut on_started,
                &mut on_progress,
            )
            .await
    }

    async fn download_single(
        &self,
        url: &Url,
        target_path: &Path,
        options: SingleOptions,
        on_started: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        on_progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let SingleOptions {
            resume_from: requested_resume_from,
            total_bytes: total_bytes_hint,
            throttle,
        } = options;
        let mut request = self
            .client
            .get(url.clone())
            .header("Accept-Encoding", "identity");
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
        let total_bytes = if status_code == 206 {
            let (start, _, total) = parse_range(response.headers())?;
            if start != actual_resume_from {
                return Err("Server returned an incorrect resume range".into());
            }
            Some(total)
        } else {
            parse_content_length(response.headers())
        };
        let temp_path = self.partial_path_for(target_path);
        let file = if actual_resume_from > 0 {
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
        let mut file = BufWriter::with_capacity(256 * 1024, file);
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
            on_progress(DownloadProgress::bytes(
                downloaded_bytes,
                total_bytes.or(total_bytes_hint),
                1,
            ))?;
        }

        file.flush()
            .await
            .map_err(|error| format!("failed to flush downloaded file: {error}"))?;

        if let Some(total) = total_bytes {
            if downloaded_bytes != total {
                return Err("Downloaded size does not match the server response".into());
            }
        }
        fs::rename(&temp_path, target_path)
            .await
            .map_err(|error| format!("failed to finalize downloaded file: {error}"))?;

        Ok((downloaded_bytes, total_bytes.or(total_bytes_hint), 1))
    }

    async fn download_stream_manifest_to_path(
        &self,
        playlist_urls: &[Url],
        target_path: &Path,
        options: StreamOptions<'_>,
        on_started: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        on_progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let StreamOptions {
            source_page_url,
            http_headers,
            transfer,
        } = options;
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

        let referer = http_headers
            .get("referer")
            .and_then(|value| validate_url(value).ok())
            .or_else(|| source_page_url.and_then(|value| validate_url(value).ok()));
        // Some CDNs answer 404 to the bare "Mozilla/5.0" placeholder instead of a real
        // browser token, so fall back to a complete desktop User-Agent.
        let user_agent = http_headers
            .get("user-agent")
            .map(String::as_str)
            .unwrap_or(FALLBACK_MEDIA_USER_AGENT);
        let forwarded_headers = [
            "origin",
            "cookie",
            "accept",
            "accept-language",
            "sec-ch-ua",
            "sec-ch-ua-mobile",
            "sec-ch-ua-platform",
            "sec-fetch-dest",
            "sec-fetch-mode",
            "sec-fetch-site",
        ]
        .iter()
        .filter_map(|name| {
            http_headers
                .get(*name)
                .map(|value| format!("{name}: {value}\r\n"))
        })
        .collect::<String>();
        let mut last_error = None;

        // Some HLS providers publish the playlist a moment after playback starts.
        // A short retry turns their transient 404 into a normal download.
        'playlists: for playlist_url in playlist_urls {
            let prepared = hls::prepare_parallel(
                &self.client,
                playlist_url,
                target_path,
                http_headers,
                source_page_url,
                if transfer.connections == 0 {
                    4
                } else {
                    transfer.connections
                },
                on_progress,
            )
            .await?;
            if prepared.is_none() {
                // Reset any prefetch estimate before the network fallback starts.
                on_started(0, None, 1)?;
            }
            let mut attempt = 0;
            let mut repair_audio = false;
            loop {
                if attempt > 0 || repair_audio {
                    let _ = fs::remove_file(&temp_path).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }

                let mut command = Command::new("ffmpeg");
                command.args(["-y", "-nostdin", "-loglevel", "error"]);
                if prepared.is_none() {
                    command.args([
                        "-user_agent",
                        user_agent,
                        "-reconnect",
                        "1",
                        "-reconnect_streamed",
                        "1",
                        "-reconnect_delay_max",
                        "5",
                    ]);
                    if let Some(referer) = referer.as_ref() {
                        command.arg("-referer").arg(referer.as_str());
                    }
                    if !forwarded_headers.is_empty() {
                        command.arg("-headers").arg(&forwarded_headers);
                    }
                }
                if prepared.is_some() || !playlist_url.path().to_ascii_lowercase().ends_with(".mpd")
                {
                    command.args([
                        "-f",
                        "hls",
                        "-allowed_segment_extensions",
                        "ALL",
                        "-extension_picky",
                        "0",
                        "-seg_max_retry",
                        "5",
                    ]);
                }
                command.args([
                    "-allowed_extensions",
                    "ALL",
                    "-rw_timeout",
                    "15000000",
                    "-i",
                ]);
                if let Some(prepared) = &prepared {
                    command.arg(&prepared.playlist);
                    if let Some(audio_playlist) = prepared.audio_playlist.as_ref() {
                        // Both tracks are already on disk; a local second input keeps the
                        // per-input protocol options out of the muxer's way entirely.
                        command.args(["-allowed_extensions", "ALL", "-f", "hls", "-i"]);
                        command.arg(audio_playlist);
                        command.args(["-map", "0:v?", "-map", "1:a?"]);
                    } else {
                        command.args(["-map", "0:v?", "-map", "0:a?"]);
                    }
                } else {
                    command.arg(playlist_url.as_str());
                    command.args(["-map", "0:v?", "-map", "0:a?"]);
                }
                configure_stream_codecs(&mut command, repair_audio);
                configure_media_output(&mut command, transfer);
                let mut child = KillOnDropChild::new(
                    command
                        .arg("-f")
                        .arg(media_muxer(&transfer.container))
                        .arg(&temp_path)
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .spawn()
                        .map_err(|error| {
                            format!("failed to start ffmpeg for stream download: {error}")
                        })?,
                );

                // FFmpeg can emit many transient network errors while an HLS stream is
                // running. Drain stderr concurrently so a full OS pipe cannot block the
                // downloader indefinitely.
                let stderr_reader = child.take_stderr().map(|mut stderr| {
                    std::thread::spawn(move || {
                        let mut bytes = Vec::new();
                        let _ = stderr.read_to_end(&mut bytes);
                        bytes
                    })
                });

                let attempt_error = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            if status.success() {
                                break None;
                            }
                            let stderr = stderr_reader
                                .and_then(|reader| reader.join().ok())
                                .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
                                .unwrap_or_default();
                            break Some(if stderr.is_empty() {
                                "ffmpeg could not finalize the media stream".to_string()
                            } else {
                                format!("ffmpeg failed: {stderr}")
                            });
                        }
                        Ok(None) => {
                            let downloaded_bytes = fs::metadata(&temp_path)
                                .await
                                .map(|metadata| metadata.len())
                                .unwrap_or(0);
                            on_progress(DownloadProgress::bytes(downloaded_bytes, None, 1))?;
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        }
                        Err(error) => {
                            break Some(format!("failed while polling ffmpeg process: {error}"));
                        }
                    }
                };

                match attempt_error {
                    None => {
                        last_error = None;
                        if let Some(prepared) = &prepared {
                            prepared.cleanup().await;
                        }
                        break 'playlists;
                    }
                    Some(error) => {
                        if !repair_audio && needs_aac_repair(&error) {
                            repair_audio = true;
                            continue;
                        }
                        let retryable = error.contains("404 Not Found")
                            || error.contains("Server returned 5")
                            || error.contains("Connection timed out")
                            || error.contains("Connection reset");
                        last_error = Some(error);
                        attempt += 1;
                        if !retryable || attempt >= 4 {
                            break;
                        }
                    }
                }
            }
        }

        if let Some(error) = last_error {
            return Err(error);
        }

        let downloaded_bytes = fs::metadata(&temp_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| format!("failed to inspect downloaded stream output: {error}"))?;

        fs::rename(&temp_path, target_path)
            .await
            .map_err(|error| format!("failed to finalize downloaded stream file: {error}"))?;
        // Fallback success no longer needs a cache left by failed prefetch.
        let _ = fs::remove_dir_all(hls::cache_path(target_path)).await;

        Ok((downloaded_bytes, None, 1))
    }

    pub async fn download_media_bundle_to_path(
        &self,
        raw_video_url: &str,
        raw_audio_url: &str,
        target_path: &Path,
        transfer: &crate::preferences::TransferPreferences,
        mut on_started: impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        mut on_progress: impl FnMut(DownloadProgress) -> Result<(), String>,
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

        let mut command = Command::new("ffmpeg");
        command
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
            .arg("copy");
        configure_media_output(&mut command, transfer);
        let mut child = KillOnDropChild::new(
            command
                .arg("-f")
                .arg(media_muxer(&transfer.container))
                .arg(&temp_path)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| format!("failed to start ffmpeg for media merge: {error}"))?,
        );

        let stderr_reader = child.take_stderr().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = stderr.read_to_end(&mut bytes);
                bytes
            })
        });

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        let stderr = stderr_reader
                            .and_then(|reader| reader.join().ok())
                            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
                            .unwrap_or_default();
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
                    on_progress(DownloadProgress::bytes(downloaded_bytes, None, 2))?;
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
        options: YtdlpOptions<'_>,
        on_started: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        on_progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let YtdlpOptions {
            format,
            use_browser_cookies,
            transfer,
            bandwidth_limit_kbps,
            http_headers,
        } = options;
        let ytdlp_path = resolve_ytdlp_path().ok_or("yt-dlp is not available")?;

        on_started(0, None, 1)?;

        let needs_cookies = use_browser_cookies
            || url
                .host_str()
                .map(|h| {
                    h.contains("facebook.com")
                        || h.contains("fb.watch")
                        || h.contains("instagram.com")
                })
                .unwrap_or(false);
        let mut cmd = clean_env_command("python3");
        cmd.arg("-c")
            .arg(include_str!("ytdlp_parallel.py"))
            .arg(&ytdlp_path);
        cmd.arg("--no-warnings")
            .arg("--no-playlist")
            .args(transfer.ytdlp_args(format));
        if let Some(limit) = bandwidth_limit_kbps {
            cmd.arg("--limit-rate").arg(format!("{limit}K"));
        }
        if let Some(headers) = http_headers {
            for (name, value) in headers {
                reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|e| e.to_string())?;
                reqwest::header::HeaderValue::from_str(value).map_err(|e| e.to_string())?;
                cmd.arg("--add-headers").arg(format!("{name}:{value}"));
            }
        }
        if needs_cookies {
            cmd.arg("--cookies-from-browser")
                .arg(detect_browser_for_cookies());
        }
        // Keep yt-dlp's .part-Frag and .ytdl sidecars out of the user's video
        // directory. Existing sidecars stay at their old path so resume works.
        let legacy_sidecars = legacy_ytdlp_sidecars(target_path).await?;
        let workspace = hidden_work_dir(target_path, "ytdlp");
        let output_path = if legacy_sidecars.is_empty() {
            fs::create_dir_all(&workspace).await.map_err(|e| e.to_string())?;
            workspace.join(target_path.file_name().ok_or("download filename is missing")?)
        } else {
            target_path.to_path_buf()
        };
        cmd.arg("-o")
            .arg(&output_path)
            .arg("--print")
            .arg("after_move:LDM_FILE:%(filepath)j")
            .arg("--newline")
            .arg("--progress")
            .arg("--no-simulate")
            .arg("--no-color")
            .arg("--progress-delta")
            .arg("1")
            .arg("--progress-template")
            .arg("download:LDM_PROGRESS:%(progress)j")
            .arg(url.as_str())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped());
        let monitor = media_progress_path(target_path);
        let _ = fs::remove_file(&monitor).await;
        write_media_progress(&monitor, transfer.connections, None).await?;
        let actual_path =
            run_ytdlp(cmd, on_progress, Some((&monitor, transfer.connections))).await?;

        if !actual_path.exists() && !target_path.exists() {
            return Err(format!(
                "yt-dlp output file not found: {}",
                actual_path.display()
            ));
        }

        if actual_path.exists() && actual_path != target_path {
            fs::rename(&actual_path, target_path)
                .await
                .map_err(|error| format!("failed to finalize yt-dlp output: {error}"))?;
        }

        if legacy_sidecars.is_empty() {
            let _ = fs::remove_dir_all(&workspace).await;
        } else {
            for sidecar in legacy_sidecars {
                let _ = fs::remove_file(sidecar).await;
            }
        }

        let downloaded_bytes = fs::metadata(target_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| format!("failed to inspect yt-dlp output: {error}"))?;

        on_progress(DownloadProgress::bytes(
            downloaded_bytes,
            Some(downloaded_bytes),
            1,
        ))?;
        Ok((downloaded_bytes, Some(downloaded_bytes), 1))
    }

    pub async fn delete_download_files(&self, target_path: &Path) -> Result<(), String> {
        match fs::remove_file(target_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not delete downloaded file: {error}")),
        }
        self.remove_temp_artifacts(target_path).await
    }

    pub async fn remove_temp_artifacts(&self, target_path: &Path) -> Result<(), String> {
        async fn remove_file(path: &Path) -> Result<(), String> {
            match fs::remove_file(path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("Could not remove temporary file: {e}")),
            }
        }
        let cache = hls::cache_path(target_path);
        match fs::remove_dir_all(&cache).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Could not remove HLS cache: {e}")),
        }
        let legacy_cache = hls::legacy_cache_path(target_path);
        if legacy_cache != cache {
            match fs::remove_dir_all(&legacy_cache).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Could not remove HLS cache: {e}")),
            }
        }
        let workspace = hidden_work_dir(target_path, "ytdlp");
        match fs::remove_dir_all(&workspace).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Could not remove yt-dlp work directory: {e}")),
        }
        for sidecar in legacy_ytdlp_sidecars(target_path).await? {
            remove_file(&sidecar).await?;
        }
        // yt-dlp names split video/audio files "<stem>.f<format>.<ext>".
        if let (Some(parent), Some(name), Some(stem)) = (
            target_path.parent(),
            target_path.file_name().and_then(|s| s.to_str()),
            target_path.file_stem().and_then(|s| s.to_str()),
        ) {
            let exact = format!("{name}.ldm-ranges");
            let prefix = format!("{stem}.f");
            if let Ok(mut entries) = fs::read_dir(parent).await {
                while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                    let entry_name = entry.file_name();
                    let value = entry_name.to_string_lossy();
                    if (value == exact
                        || (value.starts_with(&prefix) && value.ends_with(".ldm-ranges")))
                        && entry.file_type().await.map_err(|e| e.to_string())?.is_dir()
                    {
                        fs::remove_dir_all(entry.path())
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        remove_file(&media_progress_path(target_path)).await?;
        remove_file(&media_progress_path(target_path).with_extension("tmp")).await?;
        remove_file(&self.partial_path_for(target_path)).await?;
        remove_file(&target_path.with_extension("ytdlp.mp4")).await?;
        if let Some(manifest) = self.load_segment_manifest(target_path).await? {
            for segment in manifest.segments {
                remove_file(&self.segment_path_for(target_path, segment.index)).await?;
            }
        }
        remove_file(&self.manifest_path_for(target_path)).await
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
        // A manifest can disappear during download cleanup; handle absence at the read.
        let contents = match fs::read_to_string(&manifest_path).await {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("failed to read segment manifest: {error}")),
        };
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
        let temp_path = PathBuf::from(format!("{}.tmp", manifest_path.to_string_lossy()));
        fs::write(&temp_path, contents)
            .await
            .map_err(|error| format!("failed to write temporary segment manifest: {error}"))?;
        fs::rename(&temp_path, &manifest_path)
            .await
            .map_err(|error| {
                let _ = std::fs::remove_file(&temp_path);
                format!("failed to publish segment manifest: {error}")
            })?;
        Ok(())
    }

    async fn fetch_segment(
        &self,
        url: &Url,
        target: &Path,
        segment: SegmentPart,
        total: u64,
        downloaded: Arc<AtomicU64>,
        throttle: Option<Arc<BandwidthThrottle>>,
        mut split_requested: watch::Receiver<bool>,
    ) -> (usize, Result<(), SegmentFailure>) {
        let result = async {
            let path = self.segment_path_for(target, segment.index);
            let already = fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
            let start = segment.start + already;
            if start > segment.end {
                return Ok(());
            }
            let response = self
                .client
                .get(url.clone())
                .header("Accept-Encoding", "identity")
                .header(RANGE, format!("bytes={start}-{}", segment.end))
                .timeout(Duration::from_secs(120))
                .send()
                .await
                .map_err(|e| SegmentFailure::Retry(e.to_string(), Duration::ZERO, false))?;
            let code = response.status().as_u16();
            if code == 200 {
                return Err(SegmentFailure::RangeIgnored);
            }
            if [408, 425, 429, 500, 502, 503, 504].contains(&code) {
                let delay = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1)
                    .min(60);
                return Err(SegmentFailure::Retry(
                    format!("Server returned HTTP {code}"),
                    Duration::from_secs(delay),
                    true,
                ));
            }
            if code != 206 {
                return Err(SegmentFailure::Fatal(format!(
                    "Expected HTTP 206, received {code}"
                )));
            }
            let range = parse_range(response.headers()).map_err(SegmentFailure::Fatal)?;
            if range != (start, segment.end, total) {
                return Err(SegmentFailure::Fatal(
                    "Server returned an incorrect Content-Range".into(),
                ));
            }
            let expected = segment.end - start + 1;
            let mut received = 0u64;
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
                .map_err(|e| SegmentFailure::Fatal(e.to_string()))?;
            let mut stream = response.bytes_stream();
            let mut split_channel_open = true;
            loop {
                let next = tokio::select! {
                    changed = split_requested.changed(), if split_channel_open => {
                        if changed.is_err() {
                            split_channel_open = false;
                            continue;
                        }
                        if changed.is_ok() && *split_requested.borrow_and_update() {
                            file.flush().await.map_err(|e| SegmentFailure::Fatal(e.to_string()))?;
                            return Err(SegmentFailure::Split);
                        }
                        continue;
                    }
                    next = stream.next() => next,
                };
                let Some(chunk) = next else { break };
                let chunk = chunk
                    .map_err(|e| SegmentFailure::Retry(e.to_string(), Duration::ZERO, false))?;
                if received + chunk.len() as u64 > expected {
                    return Err(SegmentFailure::Fatal(
                        "Segment exceeds requested range".into(),
                    ));
                }
                if let Some(t) = &throttle {
                    t.acquire(chunk.len() as u64).await;
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|e| SegmentFailure::Fatal(e.to_string()))?;
                received += chunk.len() as u64;
                downloaded.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            file.flush()
                .await
                .map_err(|e| SegmentFailure::Fatal(e.to_string()))?;
            if received != expected {
                return Err(SegmentFailure::Retry(
                    "Incomplete segment".into(),
                    Duration::ZERO,
                    false,
                ));
            }
            Ok(())
        }
        .await;
        (segment.index, result)
    }

    async fn download_with_segments(
        &self,
        url: &Url,
        target_path: &Path,
        manifest: SegmentedDownloadManifest,
        requested_connections: usize,
        throttle: Option<Arc<BandwidthThrottle>>,
        on_started: &mut impl FnMut(u64, Option<u64>, usize) -> Result<(), String>,
        on_progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
    ) -> Result<(u64, Option<u64>, usize), String> {
        let total_bytes = manifest.total_bytes;
        let max_parallel = if requested_connections == 0 {
            manifest.segments.len().clamp(1, 16)
        } else {
            requested_connections.clamp(1, 16)
        };

        // Real progress comes from the segment files on disk, never from the
        // manifest. A merge that failed part way through deletes the segments it
        // already copied but leaves the manifest untouched, so a manifest that
        // says "complete" can point at a file that no longer exists. Reading the
        // disk first also keeps the resume offset honest, which prevents
        // duplicated bytes when the part file is longer than the manifest says.
        let mut disk_progress = Vec::with_capacity(manifest.segments.len());
        let mut initial_bytes = 0_u64;
        for segment in &manifest.segments {
            let path = self.segment_path_for(target_path, segment.index);
            let expected = segment.end - segment.start + 1;
            if fs::metadata(&path)
                .await
                .map(|m| m.len() > expected)
                .unwrap_or(false)
            {
                fs::remove_file(&path).await.map_err(|e| e.to_string())?;
            }
            let (on_disk, complete) = self.segment_progress_on_disk(target_path, segment).await;
            initial_bytes = initial_bytes.saturating_add(on_disk);
            disk_progress.push((on_disk, complete));
        }

        // Keep the manifest aligned with reality so it never advertises more
        // progress than the files actually hold.
        let mut manifest = manifest;
        for (segment, (on_disk, _)) in manifest.segments.iter_mut().zip(&disk_progress) {
            segment.downloaded_bytes = *on_disk;
        }
        self.save_segment_manifest(target_path, &manifest).await?;

        on_started(initial_bytes, Some(total_bytes), manifest.segments.len())?;

        // Progress accumulates in a shared atomic rather than a bounded
        // channel. The channel's consumer only ran after the whole segment set
        // finished, so a full 64-slot queue blocked `send` mid-transfer and
        // nothing reached the UI until the first segment finished. On a large
        // segmented file that meant speed and ETA stayed blank for minutes.
        let downloaded = Arc::new(AtomicU64::new(initial_bytes));

        // Futures are owned by this download (not detached tokio tasks): pause/drop
        // immediately stops every writer before a resume can touch the same files.
        let mut futures = FuturesUnordered::new();
        let mut pending: std::collections::VecDeque<usize> = disk_progress
            .iter()
            .enumerate()
            .filter_map(|(i, (_, done))| if *done { None } else { Some(i) })
            .collect();
        let mut attempts = vec![0u32; manifest.segments.len()];
        let mut limit = max_parallel.min(manifest.segments.len());
        let mut split_signals: HashMap<usize, watch::Sender<bool>> = HashMap::new();
        let mut split_in_progress = false;
        let mut ready_at = Instant::now();
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        let mut failure = None;
        let mut fallback = false;
        loop {
            while futures.len() < limit && Instant::now() >= ready_at {
                let Some(i) = pending.pop_front() else { break };
                attempts[i] += 1;
                let (split_tx, split_rx) = watch::channel(false);
                split_signals.insert(i, split_tx);
                futures.push(self.fetch_segment(
                    url,
                    target_path,
                    manifest.segments[i].clone(),
                    total_bytes,
                    downloaded.clone(),
                    throttle.clone(),
                    split_rx,
                ));
            }
            if futures.is_empty() && pending.is_empty() {
                break;
            }
            tokio::select! {
                _ = ticker.tick() => {
                    on_progress(DownloadProgress::bytes(downloaded.load(Ordering::Relaxed), Some(total_bytes), futures.len()))?;
                }
                result = futures.next(), if !futures.is_empty() => {
                    let Some((i,result)) = result else {continue};
                    if split_signals.remove(&i).is_some_and(|signal| *signal.borrow()) {
                        split_in_progress = false;
                    }
                    match result {
                        Ok(()) => { limit = (limit+1).min(max_parallel); }
                        Err(SegmentFailure::Split) => {
                            split_in_progress = false;
                            let (on_disk, _) = self.segment_progress_on_disk(target_path, &manifest.segments[i]).await;
                            if let Some(new_indexes) = split_remaining_segment(&mut manifest, i, on_disk, limit.saturating_sub(futures.len())) {
                                self.save_segment_manifest(target_path, &manifest).await?;
                                attempts.resize(manifest.segments.len(), 0);
                                for new_index in new_indexes {
                                    pending.push_back(new_index);
                                }
                            } else {
                                pending.push_back(i);
                            }
                        }
                        Err(SegmentFailure::RangeIgnored) => {fallback=true; break;}
                        Err(SegmentFailure::Retry(message, delay, throttled)) => {
                            if attempts[i] >= SEGMENT_MAX_ATTEMPTS {failure=Some(message); break;}
                            if throttled { limit = (limit/2).max(1); }
                            let backoff = SEGMENT_RETRY_BASE_DELAY * (1u32 << (attempts[i]-1));
                            ready_at = Instant::now() + delay.max(backoff);
                            pending.push_back(i);
                        }
                        Err(SegmentFailure::Fatal(message)) => {failure=Some(message); break;}
                    }
                    on_progress(DownloadProgress::bytes(downloaded.load(Ordering::Relaxed), Some(total_bytes), futures.len()))?;
                }
            }
            if pending.is_empty() && futures.len() < limit && !split_in_progress {
                let mut largest = None;
                for (&active, signal) in &split_signals {
                    if *signal.borrow() { continue; }
                    let (on_disk, _) = self.segment_progress_on_disk(target_path, &manifest.segments[active]).await;
                    let remaining = manifest.segments[active].end.saturating_sub(manifest.segments[active].start.saturating_add(on_disk)).saturating_add(1);
                    if on_disk > 0 && remaining >= DYNAMIC_SPLIT_MIN_REMAINING && largest.map_or(true, |(_, best)| remaining > best) {
                        largest = Some((active, remaining));
                    }
                }
                if let Some((active, _)) = largest {
                    if split_signals.get(&active).is_some_and(|signal| signal.send(true).is_ok()) {
                        split_in_progress = true;
                    }
                }
            }
        }
        drop(futures);
        if fallback {
            self.remove_temp_artifacts(target_path).await?;
            return self
                .download_single(
                    url,
                    target_path,
                    SingleOptions {
                        resume_from: 0,
                        total_bytes: Some(total_bytes),
                        throttle,
                    },
                    on_started,
                    on_progress,
                )
                .await;
        }
        if let Some(error) = failure {
            // Preserve validated partial data for a later explicit retry.
            return Err(error);
        }

        let temp_path = self.partial_path_for(target_path);
        let mut output = fs::File::create(&temp_path)
            .await
            .map_err(|error| format!("failed to create merged output: {error}"))?;

        let mut ordered_segments: Vec<_> = manifest.segments.iter().collect();
        ordered_segments.sort_by_key(|segment| segment.start);
        for segment in ordered_segments {
            let seg_path = self.segment_path_for(target_path, segment.index);
            let (on_disk, complete) = self.segment_progress_on_disk(target_path, segment).await;
            if !complete {
                // Report which segment is short instead of a bare open error,
                // so a truncated or externally removed part file is actionable.
                let _ = fs::remove_file(&temp_path).await;
                return Err(format!(
                    "failed to open segment for merging: segment {} has {} of {} bytes on disk",
                    segment.index,
                    on_disk,
                    segment.end.saturating_sub(segment.start).saturating_add(1)
                ));
            }
            let mut seg_file = fs::File::open(&seg_path)
                .await
                .map_err(|error| format!("failed to open segment for merging: {error}"))?;
            tokio::io::copy(&mut seg_file, &mut output)
                .await
                .map_err(|error| format!("failed to merge segment: {error}"))?;
            let _ = fs::remove_file(&seg_path).await;
        }

        output
            .flush()
            .await
            .map_err(|error| format!("failed to flush merged output: {error}"))?;

        fs::rename(&temp_path, target_path)
            .await
            .map_err(|error| format!("failed to finalize merged download: {error}"))?;

        let manifest_path = self.manifest_path_for(target_path);
        let _ = fs::remove_file(&manifest_path).await;

        let final_bytes = downloaded.load(Ordering::Relaxed);
        Ok((final_bytes, Some(total_bytes), manifest.segments.len()))
    }
}

enum SegmentFailure {
    Split,
    RangeIgnored,
    Retry(String, Duration, bool),
    Fatal(String),
}
fn parse_range(headers: &HeaderMap) -> Result<(u64, u64, u64), String> {
    let invalid = || "Missing or invalid Content-Range".to_string();
    let text = headers
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;
    let (range, total) = text
        .strip_prefix("bytes ")
        .and_then(|v| v.split_once('/'))
        .ok_or_else(invalid)?;
    let (start, end) = range.split_once('-').ok_or_else(invalid)?;
    let result = (
        start.parse::<u64>().map_err(|_| invalid())?,
        end.parse::<u64>().map_err(|_| invalid())?,
        total.parse::<u64>().map_err(|_| invalid())?,
    );
    if result.0 > result.1 || result.1 >= result.2 {
        return Err(invalid());
    }
    Ok(result)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTransferDetail {
    pub parallel_limit: usize,
    pub completed_fragments: Option<u64>,
    pub total_fragments: Option<u64>,
    #[serde(default)]
    pub parts: Vec<MediaPartDetail>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaPartDetail {
    pub index: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub status: String,
    pub label: String,
    pub start: u64,
}
fn media_progress_path(target: &Path) -> PathBuf {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let key = format!("{:x}", Sha256::digest(target.as_os_str().as_encoded_bytes()));
    root.join("linux-download-manager-custom").join("media-progress").join(format!("{key}.json"))
}
pub async fn read_media_progress(target: &Path) -> Option<MediaTransferDetail> {
    serde_json::from_slice(&fs::read(media_progress_path(target)).await.ok()?).ok()
}
async fn write_media_progress(
    path: &Path,
    connections: usize,
    progress: Option<&str>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    let value: serde_json::Value = progress
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default();
    let total = value["fragment_count"].as_u64().filter(|v| *v > 0);
    let completed = value["fragment_index"]
        .as_u64()
        .map(|v| total.map_or(v, |t| v.min(t)));
    let detail = MediaTransferDetail {
        parallel_limit: connections.max(1),
        completed_fragments: completed,
        total_fragments: total,
        parts: serde_json::from_slice::<MediaTransferDetail>(
            &fs::read(path).await.unwrap_or_default(),
        )
        .map(|d| d.parts)
        .unwrap_or_default(),
    };
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(&detail).map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, path).await.map_err(|e| e.to_string())
}

// Read progress while the child is running. Blocking wait_with_output prevented
// all live updates and kept the async task from responding to pause/cancel.
async fn run_ytdlp(
    command: Command,
    on_progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
    monitor: Option<(&Path, usize)>,
) -> Result<PathBuf, String> {
    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to start yt-dlp: {e}"))?;
    let stdout = child.stdout.take().ok_or("yt-dlp stdout unavailable")?;
    let mut stderr = child.stderr.take().ok_or("yt-dlp stderr unavailable")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut errors = Vec::new();
    let mut buffer = [0u8; 4096];
    let (mut stdout_open, mut stderr_open) = (true, true);
    let mut path = None;
    while stdout_open || stderr_open {
        tokio::select! {
            line = lines.next_line(), if stdout_open => {
                match line.map_err(|e| format!("yt-dlp progress read failed: {e}"))? {
                    Some(line) => {
                        if let Some(progress) = parse_ytdlp_progress(&line) {
                            if let Some((path, connections)) = monitor {
                                write_media_progress(path, connections, line.strip_prefix("LDM_PROGRESS:")).await?;
                            }
                            on_progress(progress)?;
                        } else if let Some(json) = line.strip_prefix("LDM_PARTS:") {
                            if let Some((path, connections)) = monitor
                                && let Ok(parts) = serde_json::from_str::<Vec<MediaPartDetail>>(json) {
                                let mut detail = serde_json::from_slice::<MediaTransferDetail>(&fs::read(path).await.unwrap_or_default())
                                    .unwrap_or(MediaTransferDetail {parallel_limit: connections.max(1),completed_fragments:None,total_fragments:None,parts:Vec::new()});
                                detail.parts = parts.into_iter().take(64).collect();
                                let temp = path.with_extension("tmp");
                                fs::write(&temp,serde_json::to_vec(&detail).map_err(|e|e.to_string())?).await.map_err(|e|e.to_string())?;
                                fs::rename(temp,path).await.map_err(|e|e.to_string())?;
                            }
                        } else if let Some(json) = line.strip_prefix("LDM_FILE:") {
                            path = serde_json::from_str::<String>(json).ok().map(PathBuf::from);
                        }
                    }
                    None => stdout_open = false,
                }
            }
            count = stderr.read(&mut buffer), if stderr_open => {
                let count = count.map_err(|e| format!("yt-dlp stderr read failed: {e}"))?;
                stderr_open = count != 0;
                errors.extend_from_slice(&buffer[..count]);
                // Keep only the diagnostic tail, even on a very long download.
                if errors.len() > 16 * 1024 { errors.drain(..errors.len() - 16 * 1024); }
            }
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|e| format!("failed to wait for yt-dlp: {e}"))?;
    if !status.success() {
        return Err(format!(
            "yt-dlp failed ({status}): {}",
            String::from_utf8_lossy(&errors).trim()
        ));
    }
    path.ok_or_else(|| "yt-dlp completed but did not report output file".to_string())
}

fn parse_ytdlp_progress(line: &str) -> Option<DownloadProgress> {
    let value: serde_json::Value =
        serde_json::from_str(line.strip_prefix("LDM_PROGRESS:")?).ok()?;
    let number = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_f64())
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(|n| n as u64)
    };
    let downloaded_bytes = number("downloaded_bytes")?;
    Some(DownloadProgress {
        downloaded_bytes,
        total_bytes: number("total_bytes")
            .filter(|n| *n > 0)
            .or_else(|| number("total_bytes_estimate").filter(|n| *n > 0)),
        active_parts: 1,
        speed_bytes_per_second: number("speed"),
        eta_seconds: number("eta"),
    })
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

/// Read a social post title/description when the browser supplied a generic name.
/// Metadata-only: never downloads the media, and never blocks queueing indefinitely.
pub async fn social_media_caption(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let instagram = host == "instagram.com" || host.ends_with(".instagram.com");
    let facebook = host == "facebook.com" || host.ends_with(".facebook.com");
    if !(instagram || facebook) { return None; }
    let mut command = tokio::process::Command::from(clean_env_command(&resolve_ytdlp_path()?));
    command.args(["--ignore-config", "--no-playlist", "--skip-download", "--no-warnings",
        "--socket-timeout", "5", "--retries", "0", "--print", if facebook { "%(title,description)j" } else { "%(description)j" }, url]);
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(12), command.output()).await.ok()?.ok()?;
    if !output.status.success() { return None; }
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines().find_map(|line| serde_json::from_str::<String>(line).ok())
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
    let video_id = cdn_url
        .query_pairs()
        .find(|(k, _)| k == "video_id")
        .map(|(_, v)| v.to_string())
        .or({
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
    resolve_ytdlp_path()?;

    if let Some(page_url) = source_page_url
        && is_ytdlp_supported_page(page_url)
    {
        return validate_url(page_url).ok();
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
        return extract_youtube_page_url(cdn_url)
            .or_else(|| source_page_url.and_then(|u| validate_url(u).ok()));
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
                .and_then(|mut segments| segments.next_back().map(String::from))
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
        // Reddit adaptive video/audio tracks.
        || host.ends_with(".v.redd.it")
        || host.ends_with(".redditmedia.com")
        || host.ends_with(".redd.it")
}

fn is_embedded_player_page(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    (host == "molystream.org" || host.ends_with(".molystream.org"))
        && url.path().starts_with("/embed/")
}

pub(crate) fn is_media_source(raw: &str) -> bool {
    validate_url(raw)
        .map(|u| {
            is_ytdlp_supported_page(raw)
                || is_ytdlp_supported_cdn(&u)
                || is_embedded_player_page(&u)
                || looks_like_stream_manifest_url(&u)
        })
        .unwrap_or(false)
}
pub(crate) fn is_supported_media_page(page_url: &str) -> bool {
    is_ytdlp_supported_page(page_url)
}
fn is_ytdlp_supported_page(page_url: &str) -> bool {
    // Match on the parsed hostname with an exact-or-subdomain boundary.
    // A raw substring check would also accept lookalikes such as
    // "notreddit.com/" or "evil-youtube.com/", routing unrelated hosts to
    // yt-dlp. The URL must be absolute: a scheme-less input like
    // "reddit.com/r/x" does not parse (its host is read as the scheme) and is
    // rejected rather than matched.
    let Ok(parsed) = Url::parse(page_url) else {
        return false;
    };
    let Some(host) = parsed.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };

    let dominated_by =
        |domain: &str| -> bool { host == domain || host.ends_with(&format!(".{domain}")) };
    dominated_by("facebook.com")
        || dominated_by("fb.watch")
        || dominated_by("instagram.com")
        || dominated_by("x.com")
        || dominated_by("twitter.com")
        || dominated_by("youtube.com")
        || dominated_by("youtu.be")
        || dominated_by("reddit.com")
        || dominated_by("redd.it")
}

fn looks_like_stream_manifest_url(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path.ends_with(".m3u8")
        || path.ends_with(".m3u")
        || path.ends_with(".mpd")
        || (path.contains("/hls/")
            && (path.ends_with("/master.txt")
                || path.ends_with("/index.txt")
                || path.ends_with("/playlist.txt")))
}

fn is_stream_manifest_content_type(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    STREAM_MANIFEST_CONTENT_TYPES
        .iter()
        .any(|candidate| normalized.contains(candidate))
}

fn parse_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('/').nth(1))
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            headers
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
        })
}

fn derive_file_name(url: &Url, headers: &HeaderMap) -> String {
    let inferred_extension = infer_extension(url, headers);

    headers
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_filename_from_disposition)
        .or_else(|| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .filter(|name| !name.is_empty())
                .map(|name| {
                    urlencoding::decode(name)
                        .unwrap_or_else(|_| name.into())
                        .into_owned()
                })
        })
        .map(|name| {
            if let Some(ref ext) = inferred_extension
                && !name.contains('.')
            {
                return format!("{name}.{ext}");
            }
            name
        })
        .unwrap_or_else(|| {
            let ext = inferred_extension.unwrap_or_else(|| "bin".to_string());
            format!("download.{ext}")
        })
}

fn derive_stream_file_name(url: &Url) -> String {
    let base = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .map(|name| {
            let decoded = urlencoding::decode(name).unwrap_or_else(|_| name.into());
            decoded
                .strip_suffix(".m3u8")
                .or_else(|| decoded.strip_suffix(".m3u"))
                .or_else(|| decoded.strip_suffix(".mpd"))
                .unwrap_or(&decoded)
                .to_string()
        })
        .unwrap_or_else(|| "stream".to_string());
    format!("{base}.mp4")
}

fn extract_filename_from_disposition(value: &str) -> Option<String> {
    if let Some(start) = value.find("filename*=") {
        let rest = &value[start + 10..];
        let encoded = rest.split(';').next()?.trim().trim_matches('"');
        if let Some(pos) = encoded.find("''") {
            let decoded = urlencoding::decode(&encoded[pos + 2..])
                .unwrap_or_else(|_| encoded[pos + 2..].into());
            return Some(decoded.into_owned());
        }
    }

    if let Some(start) = value.find("filename=") {
        let rest = &value[start + 9..];
        let name = rest.split(';').next()?.trim().trim_matches('"');
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
                "mp4" | "mkv" | "webm" | "mov" | "avi" | "mp3" | "m4a" | "aac" | "ogg" | "wav"
                | "pdf" | "zip" | "7z" | "tar" | "gz" | "xz" | "bz2" | "rar" | "exe" | "msi"
                | "deb" | "rpm" | "appimage" | "iso" | "img" | "dmg" | "pkg" | "apk" | "csv"
                | "epub" | "torrent" => Some(ext.to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn validate_url(raw_url: &str) -> Result<Url, String> {
    Url::parse(raw_url).map_err(|error| format!("invalid URL: {error}"))
}

// Keep the video bitstream intact. Only repair AAC after the MP4 muxer rejects
// its ADTS headers; normal downloads retain lossless, low-CPU stream copying.
fn needs_aac_repair(error: &str) -> bool {
    error.contains("aac_adtstoasc") && error.contains("Error parsing ADTS frame header")
}

fn media_muxer(container: &str) -> &str {
    match container {
        "mkv" => "matroska",
        "m4a" => "ipod",
        other => other,
    }
}
fn configure_media_output(
    command: &mut Command,
    transfer: &crate::preferences::TransferPreferences,
) {
    match transfer.container.as_str() {
        "mp3" => {
            command.args(["-vn", "-c:a", "libmp3lame", "-q:a", "2"]);
        }
        "m4a" => {
            command.args(["-vn", "-c:a", "aac", "-b:a", "192k"]);
        }
        "webm" => {
            command.args(["-c:v", "libvpx-vp9", "-c:a", "libopus", "-threads", "2"]);
        }
        _ => {}
    }
    if transfer.max_height > 0 && !matches!(transfer.container.as_str(), "mp3" | "m4a") {
        if transfer.container != "webm" {
            command.args(["-c:v", "libx264", "-preset", "veryfast", "-threads", "2"]);
        }
        command
            .arg("-vf")
            .arg(format!("scale=-2:min(ih\\,{})", transfer.max_height));
    }
    if matches!(transfer.container.as_str(), "mp4" | "m4a") {
        command.args(["-movflags", "+faststart"]);
    }
}

fn configure_stream_codecs(command: &mut Command, repair_audio: bool) {
    command.args(["-c", "copy"]);
    if repair_audio {
        command.args(["-c:a", "aac", "-b:a", "192k", "-threads:a", "1"]);
    }
}

#[cfg(test)]
mod aac_repair_tests {
    use super::*;

    #[test]
    fn repair_is_limited_to_the_observed_adts_failure() {
        assert!(needs_aac_repair(
            "[aac_adtstoasc] Error parsing ADTS frame header!"
        ));
        assert!(!needs_aac_repair("404 Not Found"));
        assert!(!needs_aac_repair("Error muxing a packet"));
    }

    #[test]
    fn audio_repair_preserves_video_copy_and_bounds_audio_threads() {
        let mut command = Command::new("ffmpeg");
        configure_stream_codecs(&mut command, false);
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["-c", "copy"]);
        let mut command = Command::new("ffmpeg");
        configure_stream_codecs(&mut command, true);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "-c",
                "copy",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-threads:a",
                "1"
            ]
        );
    }
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

#[cfg(test)]
fn build_segment_manifest(total_bytes: u64) -> SegmentedDownloadManifest {
    build_segment_manifest_with_count(total_bytes, 0)
}

/// Keep the downloaded prefix and offer the unreceived tail to idle workers.
/// Existing segment indexes never change, so pause/resume can reuse every file.
fn split_remaining_segment(
    manifest: &mut SegmentedDownloadManifest,
    index: usize,
    downloaded_bytes: u64,
    available_slots: usize,
) -> Option<std::ops::Range<usize>> {
    let segment = manifest.segments.get(index)?;
    let length = segment.end.checked_sub(segment.start)?.checked_add(1)?;
    if downloaded_bytes == 0 || downloaded_bytes >= length || available_slots < 2 {
        return None;
    }
    let remaining = length - downloaded_bytes;
    if remaining < DYNAMIC_SPLIT_MIN_REMAINING || manifest.segments.len() >= 64 {
        return None;
    }
    let pieces = available_slots
        .min((remaining / (1024 * 1024)) as usize)
        .min(64 - manifest.segments.len());
    if pieces < 2 {
        return None;
    }
    let first = manifest.segments.len();
    let mut start = segment.start + downloaded_bytes;
    let old_end = segment.end;
    manifest.segments[index].end = start - 1;
    manifest.segments[index].downloaded_bytes = downloaded_bytes;
    let base = remaining / pieces as u64;
    for part in 0..pieces {
        let end = if part + 1 == pieces { old_end } else { start + base - 1 };
        manifest.segments.push(SegmentPart {
            index: first + part,
            start,
            end,
            downloaded_bytes: 0,
        });
        start = end + 1;
    }
    Some(first..manifest.segments.len())
}

fn build_segment_manifest_with_count(
    total_bytes: u64,
    requested: usize,
) -> SegmentedDownloadManifest {
    let segment_count = if requested == 0 {
        determine_segment_count(total_bytes)
    } else {
        requested
            .clamp(1, 16)
            .min((total_bytes / (512 * 1024)).max(1) as usize)
    };
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

#[cfg(test)]
mod segment_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(".temp_files");
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join(format!(
                "segment-tests-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn target(&self) -> PathBuf {
            self.0.join("download.bin")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let result = std::fs::remove_dir_all(&self.0);
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }

    #[test]
    fn dynamic_split_keeps_downloaded_prefix_and_covers_the_tail() {
        let mut manifest = build_segment_manifest_with_count(16 * 1024 * 1024, 2);
        let old_end = manifest.segments[0].end;
        let prefix = 1024 * 1024;
        let added = split_remaining_segment(&mut manifest, 0, prefix, 3).unwrap();
        assert_eq!(added, 2..5);
        assert_eq!(manifest.segments[0].end, prefix - 1);
        assert_eq!(manifest.segments[0].downloaded_bytes, prefix);
        let mut ranges: Vec<_> = manifest.segments.iter().map(|s| (s.start, s.end)).collect();
        ranges.sort_unstable();
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 16 * 1024 * 1024 - 1);
        assert!(ranges.windows(2).all(|pair| pair[0].1 + 1 == pair[1].0));
        assert_eq!(manifest.segments[added.start].start, prefix);
        assert_eq!(manifest.segments[added.end - 1].end, old_end);
        let encoded = serde_json::to_string(&manifest).unwrap();
        let resumed: SegmentedDownloadManifest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(resumed.segments.len(), 5);
        assert!(split_remaining_segment(&mut manifest, 0, prefix, 3).is_none());
    }

    #[tokio::test]
    async fn segment_details_use_live_sizes_and_clamp_to_ranges() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let mut manifest = build_segment_manifest(20);
        manifest.segments[0].downloaded_bytes = 9;
        service
            .save_segment_manifest(&target, &manifest)
            .await
            .unwrap();
        fs::write(service.segment_path_for(&target, 0), [0; 3])
            .await
            .unwrap();
        fs::write(service.segment_path_for(&target, 1), [0; 15])
            .await
            .unwrap();

        let details = service
            .segment_details(&target, "in_progress", 0, None)
            .await
            .unwrap();
        assert_eq!(details.len(), 2);
        assert_eq!(
            (details[0].index, details[0].start, details[0].end),
            (0, 0, 9)
        );
        assert_eq!(
            (details[0].downloaded_bytes, details[0].total_bytes),
            (3, 10)
        );
        assert_eq!(details[0].status, "receiving");
        assert_eq!(
            (details[1].index, details[1].start, details[1].end),
            (1, 10, 19)
        );
        assert_eq!(
            (details[1].downloaded_bytes, details[1].total_bytes),
            (10, 10)
        );
        assert_eq!(details[1].status, "completed");

        fs::write(service.segment_path_for(&target, 0), [0; 6])
            .await
            .unwrap();
        let refreshed = service
            .segment_details(&target, "paused", 0, None)
            .await
            .unwrap();
        assert_eq!(refreshed[0].downloaded_bytes, 6);
        assert_eq!(refreshed[0].status, "paused");
    }

    #[tokio::test]
    async fn missing_segment_files_fall_back_to_clamped_manifest_counts() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let mut manifest = build_segment_manifest(20);
        manifest.segments[0].downloaded_bytes = 4;
        manifest.segments[1].downloaded_bytes = 99;
        service
            .save_segment_manifest(&target, &manifest)
            .await
            .unwrap();

        let details = service
            .segment_details(&target, "paused", 999, Some(999))
            .await
            .unwrap();
        assert_eq!(details.len(), 2);
        assert_eq!(details[0].downloaded_bytes, 4);
        assert_eq!(details[0].status, "paused");
        assert_eq!(details[1].downloaded_bytes, 10);
        assert_eq!(details[1].status, "completed");
    }

    #[tokio::test]
    async fn absent_manifest_uses_single_segment_or_empty_fallback() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        assert!(
            service
                .segment_details(&target, "queued", 0, None)
                .await
                .unwrap()
                .is_empty()
        );
        let details = service
            .segment_details(&target, "paused", 4, Some(10))
            .await
            .unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(
            (details[0].index, details[0].start, details[0].end),
            (0, 0, 9)
        );
        assert_eq!(
            (details[0].downloaded_bytes, details[0].total_bytes),
            (4, 10)
        );
        assert_eq!(details[0].status, "paused");
        let unknown = service
            .segment_details(&target, "paused", 4, None)
            .await
            .unwrap();
        assert_eq!(
            (
                unknown[0].downloaded_bytes,
                unknown[0].total_bytes,
                unknown[0].end
            ),
            (4, 4, 3)
        );
    }

    #[tokio::test]
    async fn corrupt_manifest_is_reported_not_silently_replaced_by_fallback() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        for contents in ["{", "{}", "null"] {
            fs::write(service.manifest_path_for(&target), contents)
                .await
                .unwrap();
            let error = service
                .segment_details(&target, "paused", 4, Some(10))
                .await
                .unwrap_err();
            assert!(
                error.starts_with("failed to parse segment manifest:"),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn manifest_read_errors_other_than_not_found_are_reported() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        fs::create_dir(service.manifest_path_for(&target))
            .await
            .unwrap();
        let error = service.load_segment_manifest(&target).await.unwrap_err();
        assert!(
            error.starts_with("failed to read segment manifest:"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn atomic_publication_preserves_open_snapshot_and_removes_staging_file() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        service
            .save_segment_manifest(&target, &build_segment_manifest(20))
            .await
            .unwrap();
        let path = service.manifest_path_for(&target);
        let mut snapshot = fs::File::open(&path).await.unwrap();
        service
            .save_segment_manifest(&target, &build_segment_manifest(40))
            .await
            .unwrap();
        let mut contents = String::new();
        snapshot.read_to_string(&mut contents).await.unwrap();
        let old: SegmentedDownloadManifest = serde_json::from_str(&contents).unwrap();
        assert_eq!(old.total_bytes, 20);
        assert_eq!(
            service
                .load_segment_manifest(&target)
                .await
                .unwrap()
                .unwrap()
                .total_bytes,
            40
        );
        assert!(!PathBuf::from(format!("{}.tmp", path.display())).exists());
    }

    #[tokio::test]
    async fn concurrent_publication_and_reads_only_observe_complete_manifests() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        service
            .save_segment_manifest(&target, &build_segment_manifest(20))
            .await
            .unwrap();
        let writer = async {
            for index in 0..100 {
                let total = if index % 2 == 0 { 40 } else { 20 };
                service
                    .save_segment_manifest(&target, &build_segment_manifest(total))
                    .await
                    .unwrap();
            }
        };
        let reader = async {
            for _ in 0..200 {
                let manifest = service
                    .load_segment_manifest(&target)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(matches!(manifest.total_bytes, 20 | 40));
                assert_eq!(manifest.segments.len(), 2);
                assert_eq!(manifest.segments[1].end, manifest.total_bytes - 1);
            }
        };
        tokio::join!(writer, reader);
    }

    #[tokio::test]
    async fn reads_racing_manifest_deletion_return_snapshot_or_absence() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let path = service.manifest_path_for(&target);
        for _ in 0..100 {
            service
                .save_segment_manifest(&target, &build_segment_manifest(20))
                .await
                .unwrap();
            let (read, deleted) = tokio::join!(
                service.load_segment_manifest(&target),
                fs::remove_file(&path)
            );
            deleted.unwrap();
            if let Some(manifest) = read.unwrap() {
                assert_eq!(manifest.total_bytes, 20);
                assert_eq!(manifest.segments.len(), 2);
            }
            assert!(
                service
                    .load_segment_manifest(&target)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    /// Regression for the real-world failure
    /// `failed to open segment for merging: No such file or directory`.
    ///
    /// `download_with_segments` removes each segment file once it has been
    /// copied into the merged output, but it never rewrites the manifest
    /// afterwards. A merge that dies part way through therefore leaves a
    /// manifest that still advertises the already-consumed segments as
    /// complete. Trusting that manifest makes the next attempt skip those
    /// segments and then fail while opening them for merging.
    #[tokio::test]
    async fn stale_manifest_does_not_hide_a_deleted_segment_file() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let mut manifest = build_segment_manifest(20);
        let first = manifest.segments[0].clone();
        let first_size = first.end - first.start + 1;

        // Segment 0 was consumed by an earlier merge: the manifest still says
        // it is complete, but its file no longer exists on disk.
        manifest.segments[0].downloaded_bytes = first_size;
        service
            .save_segment_manifest(&target, &manifest)
            .await
            .unwrap();
        fs::write(service.segment_path_for(&target, 1), [9; 10])
            .await
            .unwrap();

        // This is the decision the old code made from the manifest alone, and
        // it is exactly what caused the merge to fail.
        assert!(
            manifest.segments[0].downloaded_bytes >= first_size,
            "precondition: the stale manifest claims the segment is complete"
        );
        assert!(
            !service.segment_path_for(&target, 0).exists(),
            "precondition: the claimed-complete segment file is gone"
        );

        // The fix reads the disk instead of the manifest, so the missing
        // segment is reported as incomplete and gets re-downloaded rather than
        // being skipped and breaking the merge.
        let (on_disk, complete) = service
            .segment_progress_on_disk(&target, &manifest.segments[0])
            .await;
        assert_eq!(on_disk, 0, "a deleted segment reports zero bytes on disk");
        assert!(!complete, "a deleted segment must never count as complete");

        // The surviving segment is still trusted, so a retry re-downloads only
        // what is actually missing.
        let (kept, kept_complete) = service
            .segment_progress_on_disk(&target, &manifest.segments[1])
            .await;
        assert!(kept_complete);
        assert_eq!(kept, 10);
    }

    /// The resume offset must come from the file, not the manifest. Trusting a
    /// manifest that under-reports progress while the part file is appended to
    /// would re-fetch bytes that are already present.
    #[tokio::test]
    async fn resume_offset_follows_the_part_file_not_the_manifest() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let mut manifest = build_segment_manifest(20);
        let first = manifest.segments[0].clone();
        let first_size = first.end - first.start + 1;

        manifest.segments[0].downloaded_bytes = 4;
        service
            .save_segment_manifest(&target, &manifest)
            .await
            .unwrap();
        // More was actually written than the manifest recorded.
        fs::write(service.segment_path_for(&target, 0), [7; 8])
            .await
            .unwrap();

        let (on_disk, complete) = service
            .segment_progress_on_disk(&target, &manifest.segments[0])
            .await;
        assert_eq!(on_disk, 8, "progress is measured from the file on disk");
        assert!(!complete);
        assert_eq!(first.start + on_disk, first.start + 8);

        // A part file larger than its declared range is clamped, so the offset
        // can never run past the segment end.
        fs::write(service.segment_path_for(&target, 0), [7; 40])
            .await
            .unwrap();
        let (clamped, clamped_complete) = service
            .segment_progress_on_disk(&target, &manifest.segments[0])
            .await;
        assert_eq!(clamped, first_size);
        assert!(clamped_complete);
    }

    #[tokio::test]
    async fn explicit_file_deletion_removes_only_its_output_and_temporary_data() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let neighbor = target.with_file_name("unrelated.mp4");
        fs::write(&neighbor, b"keep").await.unwrap();
        fs::write(&target, b"video").await.unwrap();
        fs::write(service.partial_path_for(&target), b"partial")
            .await
            .unwrap();
        let name = target.file_name().unwrap().to_string_lossy();
        let fragment = target.with_file_name(format!("{name}.part-Frag73"));
        let ytdl = target.with_file_name(format!("{name}.ytdl"));
        let other_fragment = target.with_file_name(format!("{name} (1).part-Frag73"));
        fs::write(&fragment, b"fragment").await.unwrap();
        fs::write(&ytdl, b"state").await.unwrap();
        fs::write(&other_fragment, b"keep").await.unwrap();
        let workspace = hidden_work_dir(&target, "ytdlp");
        fs::create_dir_all(&workspace).await.unwrap();
        fs::write(workspace.join("fragment"), b"fragment").await.unwrap();
        let cache = hls::cache_path(&target);
        fs::create_dir_all(&cache).await.unwrap();
        fs::write(cache.join("segment-0.ts"), b"cached")
            .await
            .unwrap();
        service.delete_download_files(&target).await.unwrap();
        assert!(!target.exists());
        assert!(!cache.exists());
        assert!(!service.partial_path_for(&target).exists());
        assert!(!workspace.exists());
        assert!(!fragment.exists());
        assert!(!ytdl.exists());
        assert_eq!(fs::read(other_fragment).await.unwrap(), b"keep");
        assert_eq!(fs::read(neighbor).await.unwrap(), b"keep");
        service.delete_download_files(&target).await.unwrap(); // missing files are safe
        fs::create_dir(&target).await.unwrap();
        assert!(service.delete_download_files(&target).await.is_err()); // never recursively delete the target
        assert!(target.is_dir());
    }

    #[tokio::test]
    async fn a_failed_segmented_run_leaves_no_segment_files_behind() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let manifest = build_segment_manifest(20);
        service
            .save_segment_manifest(&target, &manifest)
            .await
            .unwrap();
        fs::write(service.segment_path_for(&target, 0), [0; 7])
            .await
            .unwrap();
        fs::write(service.segment_path_for(&target, 1), [0; 4])
            .await
            .unwrap();
        fs::write(service.partial_path_for(&target), [0; 3])
            .await
            .unwrap();

        // A segment error aborts the surviving writers and then clears the
        // partial files. Leaving them behind would let a later run resume from
        // segments that no longer match the manifest.
        service.remove_temp_artifacts(&target).await.unwrap();

        assert!(!service.segment_path_for(&target, 0).exists());
        assert!(!service.segment_path_for(&target, 1).exists());
        assert!(!service.manifest_path_for(&target).exists());
        assert!(!service.partial_path_for(&target).exists());
    }

    #[tokio::test]
    async fn aborted_segment_writers_stop_appending_before_cleanup() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let mut writers = FuturesUnordered::new();
        for index in 0..2usize {
            let path = service.segment_path_for(&target, index);
            fs::write(&path, [0; 1]).await.unwrap();
            let writer = tokio::spawn(async move {
                let mut file = fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .await
                    .unwrap();
                for _ in 0..200 {
                    if file.write_all(&[1; 64]).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            writers.push(writer);
        }

        tokio::time::sleep(Duration::from_millis(40)).await;
        for pending in writers.iter() {
            pending.abort();
        }
        while writers.next().await.is_some() {}

        // Once the aborts are observed, a writer that ignored them would still
        // be appending here; the size must be frozen.
        let settled = std::fs::metadata(service.segment_path_for(&target, 0))
            .unwrap()
            .len();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let after = std::fs::metadata(service.segment_path_for(&target, 0))
            .unwrap()
            .len();
        assert_eq!(
            settled, after,
            "aborted writer kept appending after failure"
        );
    }

    /// Serves `Range` requests for a deterministic body, but truncates the
    /// first `truncate_first` connections mid-body. That is the failure the
    /// Ubuntu mirror showed: some parallel range requests were dropped in
    /// flight instead of refused outright.
    async fn spawn_range_server(
        body_len: usize,
        truncate_first: usize,
    ) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let seen = counter.fetch_add(1, Ordering::SeqCst);

                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    match socket.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(read) => {
                            request.extend_from_slice(&chunk[..read]);
                            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }

                let text = String::from_utf8_lossy(&request).to_lowercase();
                let range = text
                    .lines()
                    .find_map(|line| line.strip_prefix("range: bytes="))
                    .and_then(|value| {
                        let mut parts = value.trim().split('-');
                        let start = parts.next()?.trim().parse::<usize>().ok()?;
                        let end = parts.next()?.trim().parse::<usize>().ok()?;
                        Some((start, end))
                    })
                    .unwrap_or((0, body_len.saturating_sub(1)));

                let start = range.0.min(body_len);
                let end = range.1.min(body_len.saturating_sub(1));
                let length = end.saturating_sub(start).saturating_add(1);

                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{body_len}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
                );
                if socket.write_all(head.as_bytes()).await.is_err() {
                    continue;
                }

                let payload: Vec<u8> = (start..=end).map(|index| (index % 251) as u8).collect();

                if seen < truncate_first {
                    let keep = payload.len().min(7);
                    let _ = socket.write_all(&payload[..keep]).await;
                    continue;
                }

                let _ = socket.write_all(&payload).await;
                let _ = socket.flush().await;
            }
        });

        (addr, attempts)
    }

    #[tokio::test]
    async fn idle_connection_splits_slow_range_without_losing_bytes() {
        use tokio::net::TcpListener;
        let dir = TestDir::new();
        let target = dir.target();
        let body_len = 16 * 1024 * 1024usize;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { break };
                let observed = observed.clone();
                tokio::spawn(async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    loop {
                        let Ok(read) = socket.read(&mut buffer).await else { return };
                        if read == 0 { return; }
                        request.extend_from_slice(&buffer[..read]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") { break; }
                    }
                    let text = String::from_utf8_lossy(&request).to_lowercase();
                    let Some((start, end)) = text.lines()
                        .find_map(|line| line.strip_prefix("range: bytes="))
                        .and_then(|value| value.trim().split_once('-'))
                        .and_then(|(a,b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))) else { return };
                    let header = format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{body_len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", end - start + 1);
                    if socket.write_all(header.as_bytes()).await.is_err() { return; }
                    let mut offset = start;
                    while offset <= end {
                        let next = (offset + 64 * 1024).min(end + 1);
                        let body: Vec<u8> = (offset..next).map(|n| (n % 251) as u8).collect();
                        if socket.write_all(&body).await.is_err() { return; }
                        offset = next;
                        if start == 0 { tokio::time::sleep(Duration::from_millis(12)).await; }
                    }
                });
            }
        });
        let service = DownloadService::new(Client::new());
        let url = Url::parse(&format!("http://{addr}/file.bin")).unwrap();
        let result = service.download_with_segments(
            &url, &target, build_segment_manifest_with_count(body_len as u64, 2),
            2, None, &mut |_,_,_| Ok(()), &mut |_| Ok(())
        ).await;
        server.abort();
        assert!(result.is_ok(), "dynamic download failed: {:?}", result.err());
        assert!(requests.load(Ordering::SeqCst) > 2, "idle worker never split the slow range");
        let bytes = fs::read(&target).await.unwrap();
        assert_eq!(bytes.len(), body_len);
        assert!(bytes.iter().enumerate().all(|(i, byte)| *byte == (i % 251) as u8));
    }

    #[tokio::test]
    async fn retried_segments_publish_live_bytes_and_merge_without_duplicates() {
        let dir = TestDir::new();
        let target = dir.target();
        let service = DownloadService::new(Client::new());
        let body_len = 200_usize;
        let manifest = build_segment_manifest(body_len as u64);

        let (addr, attempts) = spawn_range_server(body_len, 2).await;
        let url = Url::parse(&format!("http://{addr}/file.bin")).unwrap();

        let mut published: Vec<u64> = Vec::new();
        let result = service
            .download_with_segments(
                &url,
                &target,
                manifest,
                2,
                None,
                &mut |_started, _total, _count| Ok(()),
                &mut |progress| {
                    published.push(progress.downloaded_bytes);
                    Ok(())
                },
            )
            .await;

        assert!(result.is_ok(), "retry should recover: {:?}", result.err());
        assert!(
            attempts.load(Ordering::SeqCst) > 2,
            "at least one segment had to retry"
        );

        // Each published value is what becomes `download://state`, so the list
        // can show Speed and Time left: it must advance and never rewind.
        assert!(
            published.len() >= 2,
            "progress was not published repeatedly: {published:?}"
        );
        assert!(
            published.windows(2).all(|pair| pair[0] <= pair[1]),
            "published progress went backwards: {published:?}"
        );
        assert_eq!(
            published.last().copied(),
            Some(body_len as u64),
            "the final published byte count must match the real size"
        );

        // Retrying from the on-disk offset must neither duplicate nor drop bytes.
        let expected: Vec<u8> = (0..body_len).map(|index| (index % 251) as u8).collect();
        assert_eq!(std::fs::read(&target).unwrap(), expected);
    }
}

#[cfg(test)]
mod ytdlp_detection_tests {
    use super::*;

    fn host_url(url: &str) -> Url {
        Url::parse(url).expect("test URL must parse")
    }

    #[test]
    fn molystream_embed_is_a_player_page_not_a_direct_file() {
        assert!(is_embedded_player_page(&host_url("https://dbx.molystream.org/embed/sheila/episode")));
        assert!(is_media_source("https://dbx.molystream.org/embed/sheila/episode"));
        assert!(!is_embedded_player_page(&host_url("https://media.molystream.org/video.mp4")));
        assert!(!is_embedded_player_page(&host_url("https://molystream.org.evil.example/embed/x")));
    }

    // Regression coverage for Reddit support. Reddit URLs used to fall through
    // every yt-dlp branch, so the app saved the post's HTML page instead of the
    // video. Keep the existing YouTube/X/Facebook expectations alongside the
    // Reddit ones so adding a host cannot silently drop the others.

    #[test]
    fn reddit_post_pages_are_recognized() {
        for page in [
            "https://www.reddit.com/r/videos/comments/abc123/regression_test/",
            "https://old.reddit.com/r/videos/comments/abc123/regression_test/",
            "https://reddit.com/r/videos/comments/abc123/regression_test/",
        ] {
            assert!(
                is_ytdlp_supported_page(page),
                "reddit post page must route to yt-dlp: {page}"
            );
        }
    }

    #[test]
    fn other_supported_pages_still_resolve() {
        for page in [
            "https://www.youtube.com/watch?v=abc123",
            "https://youtu.be/abc123",
            "https://x.com/user/status/123",
            "https://twitter.com/user/status/123",
            "https://www.instagram.com/p/abc123/",
            "https://www.facebook.com/watch/?v=123",
        ] {
            assert!(
                is_ytdlp_supported_page(page),
                "previously supported page must stay supported: {page}"
            );
        }
    }

    #[test]
    fn reddit_video_cdns_are_recognized() {
        for url in [
            "https://v.redd.it/abc123/DASH_1080.mp4",
            "https://v.redd.it/abc123/DASHPlaylist.mpd",
            "https://v.redd.it/abc123/HLSPlaylist.m3u8",
            "https://preview.redd.it/abc123.jpg",
            "https://external-preview.redd.it/abc123.jpg",
            "https://packaged-media.redd.it/abc123",
            "https://i.redd.it/abc123.jpg",
        ] {
            assert!(
                is_ytdlp_supported_cdn(&host_url(url)),
                "reddit media CDN must route to yt-dlp: {url}"
            );
        }
    }

    #[test]
    fn other_video_cdns_still_resolve() {
        for url in [
            "https://rr3---sn-x.googlevideo.com/videoplayback?x=1",
            "https://i.ytimg.com/vi/abc123/maxres.jpg",
            "https://video.twimg.com/pl/avc1/abc.mp4",
        ] {
            assert!(
                is_ytdlp_supported_cdn(&host_url(url)),
                "previously supported CDN must stay supported: {url}"
            );
        }
    }

    #[test]
    fn unrelated_hosts_are_rejected() {
        // Guards against an over-broad suffix match: "notreddit.com" and
        // "evil-v.redd.it.attacker.tld" must not be treated as Reddit.
        for url in [
            "https://example.com/video.mp4",
            "https://notreddit.com/some/video.mp4",
            "https://evil-v.redd.it.attacker.tld/video.mp4",
        ] {
            assert!(
                !is_ytdlp_supported_cdn(&host_url(url)),
                "unrelated CDN must not be treated as Reddit: {url}"
            );
        }
    }

    #[test]
    fn unrelated_pages_are_rejected() {
        for page in [
            "https://example.com/watch/abc",
            "https://notreddit.com/r/x/",
        ] {
            assert!(
                !is_ytdlp_supported_page(page),
                "unrelated page must not be treated as a supported page: {page}"
            );
        }
    }
}

#[cfg(test)]
mod live_progress_tests {
    use super::*;

    #[test]
    fn ytdlp_progress_accepts_estimates_and_missing_measurements() {
        let p = parse_ytdlp_progress(r#"LDM_PROGRESS:{"downloaded_bytes":100,"total_bytes":null,"total_bytes_estimate":1000,"speed":25.5,"eta":36}"#).unwrap();
        assert_eq!(p.total_bytes, Some(1000));
        assert_eq!(p.speed_bytes_per_second, Some(25));
        assert_eq!(p.eta_seconds, Some(36));
        let unknown =
            parse_ytdlp_progress(r#"LDM_PROGRESS:{"downloaded_bytes":0,"speed":null,"eta":null}"#)
                .unwrap();
        assert_eq!(unknown.total_bytes, None);
        assert_eq!(unknown.eta_seconds, None);
        assert!(parse_ytdlp_progress("[download] normal log message").is_none());
        assert!(parse_ytdlp_progress("LDM_PROGRESS:bad json").is_none());
    }

    #[tokio::test]
    async fn media_fragment_details_are_real_and_reset_between_streams() {
        let dir = std::env::temp_dir().join(format!("ldm-fragments-{}", std::process::id()));
        fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("video.mp4");
        let path = media_progress_path(&target);
        assert_ne!(path.parent(), target.parent());
        assert_ne!(path, media_progress_path(&dir.join("other.mp4")));
        write_media_progress(
            &path,
            10,
            Some(r#"{"fragment_index":17,"fragment_count":40}"#),
        )
        .await
        .unwrap();
        let value = read_media_progress(&target).await.unwrap();
        assert_eq!(value.parallel_limit, 10);
        assert_eq!(value.completed_fragments, Some(17));
        assert_eq!(value.total_fragments, Some(40));
        write_media_progress(&path, 10, Some(r#"{"downloaded_bytes":4096}"#))
            .await
            .unwrap();
        let value = read_media_progress(&target).await.unwrap();
        assert_eq!(value.total_fragments, None);
        assert_eq!(value.completed_fragments, None);
        assert!(!dir.join("video.mp4.ldm-media-progress.json").exists());
        fs::remove_file(path).await.unwrap();
        fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn ytdlp_publishes_before_exit_and_drains_stderr() {
        let dir = std::env::temp_dir().join(format!("ldm-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ack = dir.join("ack");
        let _ = std::fs::remove_file(&ack);
        let mut command = Command::new("sh");
        command.arg("-c").arg(r#"
            i=0; while [ "$i" -lt 2000 ]; do echo 'diagnostic diagnostic diagnostic diagnostic diagnostic' >&2; i=$((i+1)); done
            printf '%s\n' 'LDM_PROGRESS:{"downloaded_bytes":128,"total_bytes":1024,"speed":256,"eta":3}'
            while [ ! -f "$1" ]; do sleep 0.02; done
            printf '%s\n' 'LDM_FILE:"/tmp/test video.mp4"'
        "#).arg("ldm-test").arg(&ack).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut samples = Vec::new();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            run_ytdlp(
                command,
                &mut |p| {
                    samples.push(p);
                    std::fs::write(&ack, b"received").unwrap();
                    Ok(())
                },
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, PathBuf::from("/tmp/test video.mp4"));
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].eta_seconds, Some(3));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn ytdlp_cancel_stops_the_child() {
        let pidfile = std::env::temp_dir().join(format!("ldm-child-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("echo $$ > \"$1\"; exec sleep 30")
            .arg("ldm-test")
            .arg(&pidfile)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(300),
                run_ytdlp(command, &mut |_| Ok(()), None)
            )
            .await
            .is_err()
        );
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        for _ in 0..40 {
            if !Path::new(&format!("/proc/{}", pid.trim())).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!Path::new(&format!("/proc/{}", pid.trim())).exists());
        std::fs::remove_file(pidfile).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a locally installed yt-dlp; uses only a localhost fixture"]
    async fn real_ytdlp_reports_live_speed_and_eta() {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut request = [0u8; 8192];
                    let _ = socket.read(&mut request).await;
                    let header = "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 2097152\r\nConnection: close\r\n\r\n";
                    if socket.write_all(header.as_bytes()).await.is_err() {
                        return;
                    }
                    for _ in 0..128 {
                        if socket.write_all(&[7u8; 16384]).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                });
            }
        });
        let target = std::env::temp_dir().join(format!("ldm-real-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&target);
        let service = DownloadService::new(Client::new());
        let url = Url::parse(&format!("http://{address}/video.mp4")).unwrap();
        let mut live_samples = 0;
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            service.download_with_ytdlp(
                &url,
                &target,
                YtdlpOptions {
                    format: Some("best"),
                    use_browser_cookies: false,
                    transfer: &Default::default(),
                    bandwidth_limit_kbps: None,
                    http_headers: None,
                },
                &mut |_, _, _| Ok(()),
                &mut |p| {
                    if p.downloaded_bytes < 2097152
                        && p.speed_bytes_per_second.unwrap_or(0) > 0
                        && p.eta_seconds.is_some()
                    {
                        live_samples += 1;
                    }
                    Ok(())
                },
            ),
        )
        .await;
        server.abort();
        result.unwrap().unwrap();
        assert!(live_samples >= 2, "missing live speed/ETA: {live_samples}");
        assert_eq!(std::fs::read(&target).unwrap(), vec![7u8; 2097152]);
        assert!(!hidden_work_dir(&target, "ytdlp").exists());
        std::fs::remove_file(target).unwrap();
    }
}

#[cfg(test)]
mod transfer_feature_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    async fn fixture(mode: &'static str) -> (Url, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/test.bin",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
                if mode == "throttle" && n == 0 {
                    let _=stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    continue;
                }
                let (start, end) = if mode == "ignore" {
                    (0, 999)
                } else {
                    text.lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|v| v.trim().split_once('-'))
                        .map(|(a, b)| (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()))
                        .unwrap_or((0, 999))
                };
                let code = if mode == "ignore" {
                    "200 OK"
                } else {
                    "206 Partial Content"
                };
                let range_start = if mode == "wrong" { start + 1 } else { start };
                let header = format!(
                    "HTTP/1.1 {code}\r\nContent-Length: {}\r\nContent-Range: bytes {range_start}-{end}/1000\r\nConnection: close\r\n\r\n",
                    end - start + 1
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let body: Vec<_> = (start..=end).map(|n| (n % 251) as u8).collect();
                let _ = stream.write_all(&body).await;
            }
        });
        (url, calls, handle)
    }
    async fn run_case(mode: &'static str, success: bool) {
        let root = std::env::temp_dir().join(format!("ldm-transfer-{}-{mode}", std::process::id()));
        fs::create_dir_all(&root).await.unwrap();
        let target = root.join("file.bin");
        let service = DownloadService::new(Client::new());
        let (url, calls, server) = fixture(mode).await;
        let result = service
            .download_with_segments(
                &url,
                &target,
                build_segment_manifest(1000),
                2,
                None,
                &mut |_, _, _| Ok(()),
                &mut |_| Ok(()),
            )
            .await;
        assert_eq!(result.is_ok(), success, "{result:?}");
        if success {
            assert_eq!(
                fs::read(&target).await.unwrap(),
                (0..1000).map(|n| (n % 251) as u8).collect::<Vec<_>>()
            );
        }
        if mode == "throttle" {
            assert!(calls.load(Ordering::SeqCst) >= 3);
        }
        server.abort();
        let _ = fs::remove_dir_all(root).await;
    }
    #[tokio::test]
    async fn ignored_range_falls_back_without_duplicate_bytes() {
        run_case("ignore", true).await;
    }
    #[tokio::test]
    async fn incorrect_content_range_is_rejected() {
        run_case("wrong", false).await;
    }
    #[tokio::test]
    async fn rate_limit_retries_without_writing_error_body() {
        run_case("throttle", true).await;
    }
    #[test]
    fn connection_count_respects_small_files_and_bounds() {
        assert_eq!(
            build_segment_manifest_with_count(64 * 1024 * 1024, 16)
                .segments
                .len(),
            16
        );
        assert_eq!(
            build_segment_manifest_with_count(1024 * 1024, 16)
                .segments
                .len(),
            2
        );
        assert_eq!(
            build_segment_manifest_with_count(128 * 1024 * 1024, 1)
                .segments
                .len(),
            1
        );
    }
    #[test]
    fn partial_paths_do_not_collide_for_different_extensions() {
        let s = DownloadService::new(Client::new());
        assert_ne!(
            s.partial_path_for(Path::new("/nonexistent/archive.zip")),
            s.partial_path_for(Path::new("/nonexistent/archive.rar"))
        );
    }
    #[tokio::test]
    async fn media_conversion_produces_real_audio_and_containers() {
        if !Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let root = std::env::temp_dir().join(format!("ldm-media-{}", std::process::id()));
        fs::create_dir_all(&root).await.unwrap();
        for container in ["mp3", "m4a", "mkv", "mov", "webm", "mp4"] {
            let path = root.join(format!("sample.{container}"));
            let mut command = Command::new("ffmpeg");
            command.args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=640x480:d=0.2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
                "-shortest",
            ]);
            configure_media_output(
                &mut command,
                &crate::preferences::TransferPreferences {
                    container: container.into(),
                    max_height: 360,
                    connections: 1,
                    audio_language: None,
                },
            );
            let output = command
                .arg("-f")
                .arg(media_muxer(container))
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{container}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let probe = Command::new("ffprobe")
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&path)
                .output()
                .unwrap();
            let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
            let streams = probe["streams"].as_array().unwrap();
            if ["mp3", "m4a"].contains(&container) {
                assert!(streams.iter().all(|v| v["codec_type"] == "audio"));
            } else {
                assert!(
                    streams
                        .iter()
                        .any(|v| v["codec_type"] == "video" && v["height"] == 360)
                );
            }
        }
        let _ = fs::remove_dir_all(root).await;
    }
}

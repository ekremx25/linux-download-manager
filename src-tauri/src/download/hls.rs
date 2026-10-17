//! Bounded parallel fetch for plain, finite HLS media playlists.
//! Complex/encrypted/live playlists stay with FFmpeg's existing demuxer.
use super::*;
use std::sync::atomic::AtomicUsize;

mod dns_fallback;

const PLAYLIST_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Debug, PartialEq)]
enum SegmentFailure {
    Network,
    /// The CDN asked us to slow down; it is transient, not fatal.
    RateLimited(Option<Duration>),
    Local(String),
}

/// How long to wait before retrying a failed segment, or None when the failure
/// must abort. A 429 is retried with a real pause so parallel prefetch adapts
/// to a rate-limited CDN instead of collapsing into the sequential fallback.
fn segment_retry_delay(failure: &SegmentFailure, attempt: u32) -> Option<Duration> {
    match failure {
        SegmentFailure::RateLimited(retry_after) if attempt < 5 => Some(
            retry_after.unwrap_or_default().max(Duration::from_secs(1 << attempt.min(4)))
        ),
        SegmentFailure::Network if attempt + 1 < SEGMENT_MAX_ATTEMPTS =>
            Some(Duration::from_millis(500 * (attempt as u64 + 1))),
        _ => None,
    }
}

fn retry_after_delay(value: Option<&str>) -> Option<Duration> {
    let value = value?;
    if let Ok(seconds) = value.parse::<u64>() { return Some(Duration::from_secs(seconds)); }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(Duration::from_secs((date.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64))
}

struct ActiveRequest(Arc<AtomicUsize>);
impl Drop for ActiveRequest {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); }
}

async fn acquire_request(active: &Arc<AtomicUsize>, live: &AtomicUsize) -> ActiveRequest {
    loop {
        let count = active.load(Ordering::SeqCst);
        if count < live.load(Ordering::SeqCst)
            && active.compare_exchange(count, count + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            return ActiveRequest(active.clone());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Concurrency ceiling the prefetch drops to on the first rate-limit response.
const RATE_LIMIT_SOFT_CAP: usize = 4;
/// Successful segments needed before the prefetch widens again.
const CONCURRENCY_RECOVERY_STREAK: usize = 24;

/// How many segments may be in flight next. A rate-limited CDN wants fewer
/// requests at once (10 -> 4 -> 1); sustained success slowly widens back up to
/// the configured parallelism.
fn next_concurrency(current: usize, parallel: usize, rate_limited: bool) -> usize {
    if rate_limited {
        return if current > RATE_LIMIT_SOFT_CAP {
            RATE_LIMIT_SOFT_CAP
        } else {
            1
        };
    }
    current.saturating_mul(2).clamp(1, parallel)
}

pub(super) struct Prepared {
    pub playlist: PathBuf,
    /// Prefetched audio rendition when the master publishes video and audio apart.
    pub audio_playlist: Option<PathBuf>,
    cache: PathBuf,
}
pub(super) fn cache_path(target: &Path) -> PathBuf {
    let legacy = legacy_cache_path(target);
    if legacy.exists() {
        return legacy;
    }
    hidden_work_dir(target, "hls")
}

pub(super) fn legacy_cache_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".hls-cache");
    target.with_file_name(name)
}

impl Prepared {
    pub async fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.cache).await;
    }
}

fn parse(text: &str, base: &Url) -> Option<(String, Vec<Url>)> {
    if !text.trim_start().starts_with("#EXTM3U")
        || !text.lines().any(|l| l.trim() == "#EXT-X-ENDLIST")
    {
        return None;
    }
    let mut local = String::new();
    let mut urls = Vec::new();
    let mut duration_pending = false;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('#') {
            let tag = line.split(':').next()?;
            if ![
                "#EXTM3U",
                "#EXTINF",
                "#EXT-X-ENDLIST",
                "#EXT-X-TARGETDURATION",
                "#EXT-X-MEDIA-SEQUENCE",
                "#EXT-X-VERSION",
                // Legacy cache hint does not change segment layout or encoding.
                "#EXT-X-ALLOW-CACHE",
                "#EXT-X-PLAYLIST-TYPE",
                "#EXT-X-DISCONTINUITY",
                "#EXT-X-DISCONTINUITY-SEQUENCE",
                "#EXT-X-INDEPENDENT-SEGMENTS",
            ]
            .contains(&tag)
            {
                return None;
            }
            if tag == "#EXTINF" {
                if duration_pending {
                    return None;
                }
                duration_pending = true;
            }
            local.push_str(line);
            local.push('\n');
        } else {
            if !duration_pending || urls.len() >= 20000 {
                return None;
            }
            let url = base.join(line).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            local.push_str(&format!("segment-{}.ts\n", urls.len()));
            urls.push(url);
            duration_pending = false;
        }
    }
    (!urls.is_empty() && !duration_pending).then_some((local, urls))
}

// Only self-contained renditions can be prefetched as one media playlist.
// Separate audio/subtitles and unknown master features stay with FFmpeg.
// Passing only a video child playlist would discard its master's audio group.
fn master_variant(text: &str, base: &Url) -> Option<Url> {
    if !text.trim_start().starts_with("#EXTM3U") {
        return None;
    }
    let mut pending = None;
    let mut variants = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            if pending.is_some() {
                return None;
            }
            let fields: Vec<_> = attrs.split(',').map(str::trim).collect();
            // Keep rendition groups on the original master so FFmpeg sees all tracks.
            if fields.iter().any(|f| {
                f.starts_with("AUDIO=")
                    || f.starts_with("VIDEO=")
                    || f.starts_with("SUBTITLES=")
                    || f.starts_with("CLOSED-CAPTIONS=\"")
            }) {
                return None;
            }
            pending = Some(
                fields
                    .iter()
                    .find_map(|f| f.strip_prefix("BANDWIDTH="))?
                    .parse::<u64>()
                    .ok()?,
            );
        } else if line.starts_with('#') {
            // I-frame playlists are trick-play renditions carried inline as comment
            // lines; they name no media URL, so skipping them keeps the real variants.
            if ![
                "#EXTM3U",
                "#EXT-X-VERSION",
                "#EXT-X-INDEPENDENT-SEGMENTS",
                "#EXT-X-I-FRAME-STREAM-INF",
            ]
            .contains(&line.split(':').next()?)
            {
                return None;
            }
        } else {
            let bandwidth = pending.take()?;
            let url = base.join(line).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            variants.push((bandwidth, url));
        }
    }
    if pending.is_some() {
        return None;
    }
    variants
        .into_iter()
        .max_by_key(|(bandwidth, _)| *bandwidth)
        .map(|(_, url)| url)
}

// Parse HLS attribute lists without splitting commas inside quoted codec/URI values.
fn attributes(text: &str) -> Option<HashMap<String, String>> {
    let mut quoted = false;
    let mut start = 0;
    let mut fields = Vec::new();
    for (i, ch) in text.char_indices() {
        if ch == '"' { quoted = !quoted; }
        if ch == ',' && !quoted { fields.push(&text[start..i]); start = i + 1; }
    }
    if quoted { return None; }
    fields.push(&text[start..]);
    let mut result = HashMap::new();
    for field in fields {
        let (key, value) = field.trim().split_once('=')?;
        if result.insert(key.to_owned(), value.trim_matches('"').to_owned()).is_some() { return None; }
    }
    Some(result)
}

// Simple separate audio groups can also use the cache. Other rendition types
// remain with FFmpeg. Match AUDIO to GROUP-ID, not the first audio in the file.
fn master_tracks(text: &str, base: &Url) -> Option<(Url, Option<Url>)> {
    if let Some(video) = master_variant(text, base) { return Some((video, None)); }
    let mut clean = String::new();
    let mut renditions = Vec::new();
    let mut variants = Vec::new();
    let mut pending = None;
    for line in text.lines().map(str::trim) {
        if let Some(raw) = line.strip_prefix("#EXT-X-MEDIA:") {
            let attrs = attributes(raw)?;
            if attrs.get("TYPE").map(String::as_str) != Some("AUDIO") { return None; }
            renditions.push(attrs);
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let mut attrs = attributes(raw)?;
            pending = Some(attrs.remove("AUDIO"));
            clean.push_str("#EXT-X-STREAM-INF:");
            clean.push_str(&attrs.iter().map(|(k,v)| if v == "NONE" { format!("{k}=NONE") } else { format!("{k}=\"{v}\"") }).collect::<Vec<_>>().join(","));
            // master_variant expects an unquoted integer bandwidth.
            if let Some(bw) = attrs.get("BANDWIDTH") {
                clean = clean.replace(&format!("BANDWIDTH=\"{bw}\""), &format!("BANDWIDTH={bw}"));
            }
            clean.push('\n');
            continue;
        }
        if !line.is_empty() && !line.starts_with('#') {
            variants.push((base.join(line).ok()?, pending.take()?));
        }
        clean.push_str(line); clean.push('\n');
    }
    let video = master_variant(&clean, base)?;
    let group = variants.into_iter().find(|(url,_)| *url == video)?.1;
    let Some(group) = group else { return Some((video, None)); };
    let track = renditions.iter().filter(|a| a.get("GROUP-ID") == Some(&group))
        .max_by_key(|a| (a.get("DEFAULT").map(String::as_str) == Some("YES"),
                        a.get("AUTOSELECT").map(String::as_str) == Some("YES")))?;
    let Some(uri) = track.get("URI") else { return Some((video, None)); };
    let audio = base.join(uri).ok()?;
    if !matches!(audio.scheme(), "http" | "https") { return None; }
    Some((video, Some(audio)))
}

async fn fetch_playlist(
    client: &Client,
    url: &Url,
    credential_origin: &Url,
    headers: &HashMap<String, String>,
    referer: Option<&str>,
) -> Option<(Url, String)> {
    let response = request(client, url, credential_origin, headers, referer)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let base = response.url().clone();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if body.len() + chunk.len() > PLAYLIST_LIMIT {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some((base, String::from_utf8(body).ok()?))
}

pub(super) async fn expected_duration(
    client: &Client,
    url: &Url,
    headers: &HashMap<String, String>,
    referer: Option<&str>,
) -> Option<f64> {
    let mut next = url.clone();
    let mut visited = std::collections::HashSet::new();
    for _ in 0..4 {
        if !visited.insert(next.clone()) {
            return None;
        }
        let (base, text) = fetch_playlist(client, &next, url, headers, referer).await?;
        if text.lines().any(|line| line.trim() == "#EXT-X-ENDLIST") {
            let durations: Vec<f64> = text
                .lines()
                .filter_map(|line| line.trim().strip_prefix("#EXTINF:"))
                .map(|value| value.split(',').next()?.parse::<f64>().ok())
                .collect::<Option<_>>()?;
            let total: f64 = durations.iter().sum();
            return (durations.len() > 0 && total.is_finite() && total > 0.0).then_some(total);
        }
        next = master_tracks(&text, &base)?.0;
    }
    None
}

fn request(
    client: &Client,
    url: &Url,
    base: &Url,
    headers: &HashMap<String, String>,
    referer: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut req = client.get(url.clone()).timeout(Duration::from_secs(30));
    if let Some(referer) = referer {
        req = req.header("referer", referer);
    }
    for (name, value) in headers {
        if ["host", "content-length", "range"].contains(&name.to_ascii_lowercase().as_str()) {
            continue;
        }
        if ["cookie", "authorization"].contains(&name.to_ascii_lowercase().as_str())
            && url.origin() != base.origin()
        {
            continue;
        }
        req = req.header(name, value);
    }
    req
}

#[cfg(test)]
async fn prepare(
    client: &Client,
    url: &Url,
    target: &Path,
    headers: &HashMap<String, String>,
    referer: Option<&str>,
    progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
) -> Result<Option<Prepared>, String> {
    prepare_parallel(client, url, target, headers, referer, 4, progress).await
}

pub(super) async fn prepare_parallel(
    client: &Client,
    url: &Url,
    target: &Path,
    headers: &HashMap<String, String>,
    referer: Option<&str>,
    parallel: usize,
    progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
) -> Result<Option<Prepared>, String> {
    let parallel = parallel.clamp(1, 16);
    // Resolve a bounded master chain; unsupported media still uses FFmpeg.
    let mut next = url.clone();
    let mut visited = std::collections::HashSet::new();
    let mut media = None;
    let mut audio = None;
    for _ in 0..4 {
        if !visited.insert(next.clone()) {
            return Ok(None);
        }
        let Some((base, text)) = fetch_playlist(client, &next, url, headers, referer).await else {
            return Ok(None);
        };
        if let Some((local, urls)) = parse(&text, &base) {
            media = Some((base, text, local, urls));
            break;
        }
        let Some((variant, track)) = master_tracks(&text, &base) else {
            return Ok(None);
        };
        if track.is_some() {
            if audio.is_some() { return Ok(None); }
            audio = track;
        }
        next = variant;
    }
    let Some((base, text, _, _)) = media else {
        return Ok(None);
    };
    // Validate audio before fetching video; unsupported/encrypted audio must
    // keep the original master intact for the compatibility path.
    let audio_media = if let Some(audio) = audio {
        let Some((base, text)) = fetch_playlist(client, &audio, url, headers, referer).await else {
            return Err("Audio playlist is unavailable. Resume / retry will preserve cached fragments.".into());
        };
        if parse(&text, &base).is_none() { return Ok(None); }
        Some((base, text))
    } else { None };
    let cache = cache_path(target);
    let mut video_progress = |p: DownloadProgress| {
        progress(DownloadProgress::bytes(p.downloaded_bytes,
            if audio_media.is_some() { None } else { p.total_bytes }, parallel))
    };
    let Some((playlist, video_bytes, _)) = prefetch_playlist(
        client, url, headers, referer, &base, &text, &cache, parallel, &mut video_progress,
    ).await? else { return Ok(None); };
    let audio_playlist = if let Some((base, text)) = audio_media {
        let audio_parallel = parallel.min(RATE_LIMIT_SOFT_CAP);
        let mut audio_progress = |p: DownloadProgress| {
            progress(DownloadProgress::bytes(video_bytes + p.downloaded_bytes,
                p.total_bytes.map(|n| video_bytes + n), audio_parallel))
        };
        // Audio has many small fragments: bounded parallelism avoids paying
        // one network round trip per fragment. Existing 429 handling reduces
        // concurrency to one when needed; retries retain completed fragments.
        let Some((playlist, _, _)) = prefetch_playlist(
            client, url, headers, referer, &base, &text, &cache, audio_parallel, &mut audio_progress,
        ).await? else {
            return Err("Audio download failed. Video fragments are saved; Resume / retry will reuse them.".into());
        };
        Some(playlist)
    } else { None };
    Ok(Some(Prepared {
        playlist,
        audio_playlist,
        cache,
    }))
}

/// Downloads every segment of one media playlist in parallel and writes the local
/// playlist FFmpeg reads later. Returns None when the network drops a segment so
/// the caller can fall back to FFmpeg.
#[allow(clippy::too_many_arguments)]
async fn prefetch_playlist(
    client: &Client,
    credential_origin: &Url,
    headers: &HashMap<String, String>,
    referer: Option<&str>,
    base: &Url,
    text: &str,
    cache: &Path,
    parallel: usize,
    progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
) -> Result<Option<(PathBuf, u64, usize)>, String> {
    let Some((local, urls)) = parse(text, base) else {
        return Ok(None);
    };
    // Completed segments are reusable only for the exact same playlist and URL.
    let fingerprint = format!("{:x}", Sha256::digest(format!("all-segments-v2\n{base}\n{text}")));
    let folder = cache.join(fingerprint);
    fs::create_dir_all(&folder)
        .await
        .map_err(|e| format!("HLS cache: {e}"))?;
    let bytes = Arc::new(AtomicU64::new(0));
    let count = urls.len();
    let mut completed = 0usize;
    // Rate limiting is global, so the in-flight cap lives here and every segment
    // job shares it instead of relying on the fixed buffer_unordered width.
    let active = Arc::new(AtomicUsize::new(0));
    let cooldown = Arc::new(std::sync::Mutex::new(Instant::now()));
    let live = Arc::new(AtomicUsize::new(parallel));
    let streak = Arc::new(AtomicUsize::new(0));
    let dns = dns_fallback::Fallback::default();
    let jobs = futures_util::stream::iter(urls.into_iter().enumerate().map(|(index, url)| {
        let dns = &dns;
        let folder = folder.clone();
        let bytes = bytes.clone();
        let credential_origin = credential_origin.clone();
        let (active, live, streak, cooldown) = (active.clone(), live.clone(), streak.clone(), cooldown.clone());
        async move {
            let output = folder.join(format!("segment-{index}.ts"));
            if let Ok(meta) = fs::metadata(&output).await {
                if meta.len() > 0 {
                    bytes.fetch_add(meta.len(), Ordering::Relaxed);
                    return Ok(());
                }
            }
            let part = folder.join(format!("segment-{index}.tmp"));
            for attempt in 0..6 {
                let result = async {
                    let _permit = acquire_request(&active, &live).await;
                    loop {
                        let delay = cooldown.lock().unwrap().saturating_duration_since(Instant::now());
                        if delay.is_zero() { break; }
                        tokio::time::sleep(delay).await;
                    }
                    let response = dns.send(client, &url, &credential_origin, headers, referer)
                        .await.map_err(|_| SegmentFailure::Network)?;
                    if response.status().as_u16() == 429 {
                        let delay = retry_after_delay(response.headers().get("retry-after").and_then(|v| v.to_str().ok()));
                        return Err(SegmentFailure::RateLimited(delay));
                    }
                    let response = response.error_for_status().map_err(|_| SegmentFailure::Network)?;
                    let file = fs::File::create(&part)
                        .await
                        .map_err(|e| SegmentFailure::Local(e.to_string()))?;
                    let mut file = BufWriter::with_capacity(256 * 1024, file);
                    let mut stream = response.bytes_stream();
                    let mut written = 0u64;
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.map_err(|_| SegmentFailure::Network)?;
                        file.write_all(&chunk)
                            .await
                            .map_err(|e| SegmentFailure::Local(e.to_string()))?;
                        written += chunk.len() as u64;
                    }
                    file.flush()
                        .await
                        .map_err(|e| SegmentFailure::Local(e.to_string()))?;
                    if written == 0 {
                        return Err(SegmentFailure::Network);
                    }
                    fs::rename(&part, &output)
                        .await
                        .map_err(|e| SegmentFailure::Local(e.to_string()))?;
                    bytes.fetch_add(written, Ordering::Relaxed);
                    Ok::<(), SegmentFailure>(())
                }
                .await;
                match result {
                    Ok(()) => {
                        // Sustained progress earns back some parallelism.
                        if streak.fetch_add(1, Ordering::Relaxed) + 1 >= CONCURRENCY_RECOVERY_STREAK
                        {
                            streak.store(0, Ordering::Relaxed);
                            let current = live.load(Ordering::Relaxed);
                            let raised = next_concurrency(current, parallel, false);
                            if raised > current {
                                live.store(raised, Ordering::Relaxed);
                            }
                        }
                        return Ok(());
                    }
                    Err(failure) => {
                        if matches!(failure, SegmentFailure::RateLimited(_)) {
                            let current = live.load(Ordering::Relaxed);
                            let reduced = next_concurrency(current, parallel, true);
                            if reduced < current {
                                live.store(reduced, Ordering::Relaxed);
                            }
                            streak.store(0, Ordering::Relaxed);
                        }
                        let Some(delay) = segment_retry_delay(&failure, attempt) else {
                            return Err(match failure {
                                SegmentFailure::Local(error) => SegmentFailure::Local(format!(
                                    "HLS segment {}: {error}",
                                    index + 1
                                )),
                                _ => SegmentFailure::Network,
                            });
                        };
                        if matches!(failure, SegmentFailure::RateLimited(_)) {
                            let mut deadline = cooldown.lock().unwrap();
                            if let Some(next) = Instant::now().checked_add(delay) {
                                *deadline = (*deadline).max(next);
                            }
                        }
                        tokio::time::sleep(delay).await;
                    }
                }
            }
            unreachable!()
        }
    }))
    .buffer_unordered(parallel);
    tokio::pin!(jobs);
    while let Some(result) = jobs.next().await {
        match result {
            Ok(()) => {}
            // Drop unfinished requests but keep completed cache files. The caller
            // can now try FFmpeg and its alternative playlists instead of failing.
            Err(SegmentFailure::Network | SegmentFailure::RateLimited(_)) => return Ok(None),
            Err(SegmentFailure::Local(error)) => return Err(error),
        }
        completed += 1;
        let downloaded = bytes.load(Ordering::Relaxed);
        // This estimate is based on finished segment sizes, not the MP4 output.
        let estimated = downloaded.saturating_mul(count as u64) / (completed as u64);
        progress(DownloadProgress::bytes(
            downloaded,
            Some(estimated.max(downloaded)),
            parallel.min(count - completed).max(1),
        ))?;
    }
    let playlist = folder.join("local.m3u8");
    fs::write(&playlist, local)
        .await
        .map_err(|e| format!("HLS playlist: {e}"))?;
    Ok(Some((playlist, bytes.load(Ordering::Relaxed), count)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_retry_after_is_honored() {
        assert_eq!(retry_after_delay(Some("7")), Some(Duration::from_secs(7)));
        assert_eq!(retry_after_delay(Some("invalid")), None);
        assert_eq!(segment_retry_delay(&SegmentFailure::RateLimited(Some(Duration::from_secs(7))), 0), Some(Duration::from_secs(7)));
    }

    #[tokio::test]
    async fn reducing_busy_concurrency_restricts_new_requests() {
        let active = Arc::new(AtomicUsize::new(0));
        let live = AtomicUsize::new(4);
        let mut held = Vec::new();
        for _ in 0..4 { held.push(acquire_request(&active, &live).await); }
        live.store(1, Ordering::SeqCst);
        held.truncate(1);
        assert!(tokio::time::timeout(Duration::from_millis(60), acquire_request(&active, &live)).await.is_err());
        drop(held);
        let guard = tokio::time::timeout(Duration::from_millis(60), acquire_request(&active, &live)).await.unwrap();
        drop(guard);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn separate_audio_selection_matches_group_and_default() {
        let base = Url::parse("https://example.com/master").unwrap();
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"wrong\",DEFAULT=YES,URI=\"wrong.m3u8\"\n#EXT-X-MEDIA:URI=\"other.m3u8\",GROUP-ID=\"sound\",TYPE=AUDIO\n#EXT-X-MEDIA:URI=\"default,audio.m3u8\",DEFAULT=YES,GROUP-ID=\"sound\",TYPE=AUDIO\n#EXT-X-STREAM-INF:BANDWIDTH=123,CODECS=\"avc1,mp4a\",AUDIO=\"sound\",CLOSED-CAPTIONS=NONE\nvideo.m3u8\n";
        let (video, audio) = master_tracks(master, &base).unwrap();
        assert!(video.path().ends_with("video.m3u8"));
        assert!(audio.unwrap().path().ends_with("default,audio.m3u8"));
        assert!(master_tracks(&master.replace("AUDIO=\"sound\"", "AUDIO=\"missing\""), &base).is_none());
        assert!(master_tracks(&master.replace("default,audio.m3u8", "file:///tmp/audio"), &base).is_none());
        assert!(master_tracks(&master.replace("TYPE=AUDIO", "TYPE=SUBTITLES"), &base).is_none());
    }
    #[test]
    fn audio_named_segments_are_never_discarded() {
        let base = Url::parse("https://example.com/media.m3u8").unwrap();
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\npart-f2-1.ts\n#EXTINF:1,\npart-f2-2.ts\n#EXT-X-ENDLIST\n";
        let (local, urls) = parse(text, &base).unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].path().ends_with("part-f2-1.ts"));
        assert_eq!(local.matches("#EXTINF:").count(), 2);
        assert!(local.contains("segment-0.ts\n"));
        assert!(local.contains("segment-1.ts\n"));
    }
    #[test]
    fn master_selection_preserves_external_tracks_and_rejects_unsafe_sources() {
        let base = Url::parse("https://example.com/hls/master").unwrap();
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nlow/list.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=200,CODECS=\"avc1,mp4a\"\nhigh/list.m3u8\n";
        assert_eq!(
            master_variant(text, &base).unwrap().as_str(),
            "https://example.com/hls/high/list.m3u8"
        );
        let grouped = text.replace("BANDWIDTH=100", "BANDWIDTH=100,AUDIO=\"group\"");
        assert!(master_variant(&grouped, &base).is_none());
        for attributes in [
            "TYPE=AUDIO,GROUP-ID=\"group\",DEFAULT=YES,URI=\"audio\"",
            "URI=\"audio\",GROUP-ID=\"group\",TYPE=AUDIO",
        ] {
            let with_audio = text.replace("#EXTM3U", &format!("#EXTM3U\n#EXT-X-MEDIA:{attributes}"));
            assert!(master_variant(&with_audio, &base).is_none());
        }
        // Rendition groups the prefetcher cannot handle still belong to FFmpeg.
        assert!(
            master_variant(
                &text.replace("BANDWIDTH=100", "BANDWIDTH=100,SUBTITLES=\"subs\""),
                &base
            )
            .is_none()
        );
        assert!(
            master_variant(&text.replace("low/list.m3u8", "file:///tmp/secret"), &base).is_none()
        );
        assert!(master_variant("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100", &base).is_none());
        // Trick-play I-frame renditions must not push the whole master to FFmpeg.
        let iframe = text.replace(
            "high/list.m3u8",
            "#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=71931,URI=\"iframes.m3u8\"\nhigh/list.m3u8",
        );
        assert_eq!(
            master_variant(&iframe, &base).unwrap().as_str(),
            "https://example.com/hls/high/list.m3u8"
        );
    }

    #[test]
    fn rate_limited_segments_wait_and_retry_before_giving_up() {
        // A 429 is transient: it must survive the first attempts instead of
        // collapsing the whole parallel prefetch into the sequential fallback.
        assert!(segment_retry_delay(&SegmentFailure::RateLimited(None), 0).is_some());
        assert!(segment_retry_delay(&SegmentFailure::RateLimited(None), 1).is_some());
        // But it cannot retry forever, otherwise a hard limit never surfaces.
        assert!(segment_retry_delay(&SegmentFailure::RateLimited(None), 2).is_some());
        assert!(segment_retry_delay(&SegmentFailure::RateLimited(None), 5).is_none());
        // Rate limiting waits longer than a plain transport hiccup.
        assert!(
            segment_retry_delay(&SegmentFailure::RateLimited(None), 0)
                > segment_retry_delay(&SegmentFailure::Network, 0)
        );
        // Local write errors are never retried.
        assert!(segment_retry_delay(&SegmentFailure::Local("disk full".into()), 0).is_none());
    }

    #[test]
    fn rate_limiting_steps_concurrency_down_and_success_widens_it_back() {
        // 10 -> 4 on the first 429, then 4 -> 1 once the CDN keeps refusing.
        assert_eq!(next_concurrency(10, 10, true), RATE_LIMIT_SOFT_CAP);
        assert_eq!(next_concurrency(RATE_LIMIT_SOFT_CAP, 10, true), 1);
        assert_eq!(next_concurrency(1, 10, true), 1);
        // Success widens gradually and never exceeds the configured parallelism.
        assert_eq!(next_concurrency(1, 10, false), 2);
        assert_eq!(next_concurrency(2, 10, false), 4);
        assert_eq!(next_concurrency(8, 10, false), 10);
        assert_eq!(next_concurrency(10, 10, false), 10);
    }

    #[test]
    fn master_child_requests_do_not_forward_cross_origin_credentials() {
        let client = Client::new();
        let base = Url::parse("https://example.com/master").unwrap();
        let child = Url::parse("https://cdn.example.com/media").unwrap();
        let headers = HashMap::from([
            ("cookie".into(), "private".into()),
            ("authorization".into(), "Bearer test".into()),
        ]);
        let req = request(&client, &child, &base, &headers, None)
            .build()
            .unwrap();
        assert!(!req.headers().contains_key("cookie"));
        assert!(!req.headers().contains_key("authorization"));
        let req = request(&client, &base, &base, &headers, None)
            .build()
            .unwrap();
        assert!(req.headers().contains_key("cookie"));
    }

    #[test]
    fn legacy_cache_hint_does_not_disable_parallel_downloads() {
        let url = Url::parse("https://example.com/playlist").unwrap();
        for value in ["YES", "NO"] {
            let text = format!(
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-ALLOW-CACHE:{value}\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:10,\npart.ts\n#EXT-X-ENDLIST\n"
            );
            let (_, urls) = parse(&text, &url).expect("legacy HLS cache hints must allow prefetch");
            assert_eq!(urls.len(), 1);
            assert!(parse(&text.replace("#EXT-X-ENDLIST", ""), &url).is_none());
            assert!(parse(&format!("#EXT-X-KEY:METHOD=AES-128,URI=key\n{text}"), &url).is_none());
        }
    }

    #[test]
    fn only_plain_finite_playlists_are_prefetched() {
        let url = Url::parse("https://example.com/hls/list").unwrap();
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\npart.ts\n#EXT-X-ENDLIST\n";
        let (local, urls) = parse(text, &url).unwrap();
        assert!(local.contains("segment-0.ts"));
        assert_eq!(urls[0].as_str(), "https://example.com/hls/part.ts");
        assert!(parse(&text.replace("#EXT-X-ENDLIST", ""), &url).is_none());
        for tag in [
            "#EXT-X-KEY:METHOD=AES-128,URI=key",
            "#EXT-X-MAP:URI=init",
            "#EXT-X-BYTERANGE:10",
            "#EXT-X-STREAM-INF:BANDWIDTH=100",
        ] {
            assert!(parse(&format!("{tag}\n{text}"), &url).is_none());
        }
        assert!(parse(&text.replace("part.ts", "file:///etc/passwd"), &url).is_none());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    struct Fixture {
        url: Url,
        requests: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Fixture {
        fn start(segment: Vec<u8>) -> Self {
            Self::start_count(segment, 8)
        }
        fn start_count(segment: Vec<u8>, count: usize) -> Self {
            Self::start_tracks(segment, None, count)
        }
        fn start_tracks(segment: Vec<u8>, audio: Option<Vec<u8>>, count: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = Url::parse(&format!(
                "http://{}/playlist.m3u8",
                listener.local_addr().unwrap()
            ))
            .unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let active = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (r, a, p, s) = (requests.clone(), active, peak.clone(), stop.clone());
            let thread = std::thread::spawn(move || {
                let mut workers = Vec::new();
                while !s.load(Ordering::Relaxed) {
                    let Ok((mut socket, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    let (r, a, p, segment) = (r.clone(), a.clone(), p.clone(), segment.clone());
                    let audio = audio.clone();
                    workers.push(std::thread::spawn(move || {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut input = [0; 4096];
                        let n = socket.read(&mut input).unwrap_or(0);
                        let header = String::from_utf8_lossy(&input[..n]);
                        if header.starts_with("GET /limited.ts ") && r.fetch_add(1, Ordering::Relaxed) == 0 {
                            let _ = write!(socket, "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            return;
                        }
                        let body = if header.starts_with("GET /limited.m3u8 ") {
                            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nlimited.ts\n#EXT-X-ENDLIST\n".to_vec()
                        } else if header.starts_with("GET /limited.ts ") {
                            segment
                        } else if header.starts_with("GET /split-unreachable.m3u8 ") {
                            b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"sound\",URI=\"unreachable.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1000000,AUDIO=\"sound\"\nmedia.m3u8\n".to_vec()
                        } else if header.starts_with("GET /split.m3u8 ") {
                            b"#EXTM3U\n#EXT-X-MEDIA:URI=\"audio.m3u8\",TYPE=AUDIO,GROUP-ID=\"sound\",NAME=\"Original\",DEFAULT=YES,AUTOSELECT=YES\n#EXT-X-STREAM-INF:BANDWIDTH=1000000,AUDIO=\"sound\"\nmedia.m3u8\n".to_vec()
                        } else if header.starts_with("GET /audio.m3u8 ") {
                            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:1,\nsegment-f2-0.ts\n#EXT-X-ENDLIST\n".to_vec()
                        } else if header.starts_with("GET /segment-f2-0.ts ") {
                            audio.unwrap_or_default()
                        } else if header.starts_with("GET /unreachable.m3u8 ") {
                            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nhttp://127.0.0.1:0/unreachable.ts\n#EXT-X-ENDLIST\n".to_vec()
                        } else if header.starts_with("GET /playlist.m3u8 ") {
                            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\nmedia.m3u8\n".to_vec()
                        } else if header.starts_with("GET /media.m3u8 ") {
                            format!(
                                "#EXTM3U\n#EXT-X-ALLOW-CACHE:YES\n#EXT-X-TARGETDURATION:1\n{}#EXT-X-ENDLIST\n",
                                (0..count)
                                    .map(|i| format!("#EXTINF:1,\nsegment-{i}.ts\n"))
                                    .collect::<String>()
                            )
                            .into_bytes()
                        } else {
                            r.fetch_add(1, Ordering::Relaxed);
                            let current = a.fetch_add(1, Ordering::Relaxed) + 1;
                            p.fetch_max(current, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(120));
                            a.fetch_sub(1, Ordering::Relaxed);
                            segment
                        };
                        let _ = write!(
                            socket,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(&body);
                    }));
                }
                for worker in workers {
                    let _ = worker.join();
                }
            });
            Self {
                url,
                requests,
                peak,
                stop,
                thread: Some(thread),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = self.thread.take().unwrap().join();
        }
    }
    fn test_target(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ldm-hls-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root.join("movie.mp4")
    }
    #[tokio::test]
    async fn rate_limited_http_response_waits_then_completes() {
        let fixture = Fixture::start_count(vec![7; 4096], 1);
        let target = test_target("retry-after");
        let start = Instant::now();
        let prepared = prepare(&Client::new(), &fixture.url.join("limited.m3u8").unwrap(),
            &target, &HashMap::new(), None, &mut |_| Ok(())).await.unwrap().unwrap();
        assert!(start.elapsed() >= Duration::from_secs(1));
        assert_eq!(fixture.requests.load(Ordering::Relaxed), 2);
        assert!(prepared.playlist.is_file());
        prepared.cleanup().await;
        let _ = fs::remove_dir_all(target.parent().unwrap()).await;
    }
    #[tokio::test]
    async fn failed_audio_retry_reuses_video_instead_of_falling_back() {
        let fixture = Fixture::start_count(vec![7; 4096], 2);
        let target = test_target("audio-retry");
        let master = fixture.url.join("split-unreachable.m3u8").unwrap();
        for _ in 0..2 {
            let result = prepare(&Client::new(), &master, &target, &HashMap::new(), None, &mut |_| Ok(())).await;
            assert!(result.err().unwrap().contains("Audio download failed"));
            assert_eq!(fixture.requests.load(Ordering::Relaxed), 2, "completed video must not download again");
        }
        let _ = fs::remove_dir_all(target.parent().unwrap()).await;
    }
    #[tokio::test]
    async fn configured_ten_connections_are_used() {
        let fixture = Fixture::start_count(vec![7; 4096], 20);
        let target = test_target("ten-connections");
        assert_eq!(
            expected_duration(&Client::new(), &fixture.url, &HashMap::new(), None).await,
            Some(20.0)
        );
        let prepared = prepare_parallel(
            &Client::new(),
            &fixture.url,
            &target,
            &HashMap::new(),
            None,
            10,
            &mut |_| Ok(()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(fixture.peak.load(Ordering::Relaxed), 10);
        assert_eq!(fixture.requests.load(Ordering::Relaxed), 20);
        prepared.cleanup().await;
        let _ = fs::remove_dir_all(target.parent().unwrap()).await;
    }

    #[tokio::test]
    async fn unreachable_segment_falls_back_but_local_storage_failure_does_not() {
        let fixture = Fixture::start(vec![7; 128]);
        let client = Client::new();
        let target = test_target("unreachable");
        let result = prepare(
            &client,
            &fixture.url.join("unreachable.m3u8").unwrap(),
            &target,
            &HashMap::new(),
            None,
            &mut |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(
            result.is_none(),
            "transport errors must leave FFmpeg fallback available"
        );
        let blocked = target.with_file_name("blocked");
        fs::write(&blocked, b"not a directory").await.unwrap();
        assert!(
            prepare(
                &client,
                &fixture.url,
                &blocked.join("movie.mp4"),
                &HashMap::new(),
                None,
                &mut |_| Ok(())
            )
            .await
            .is_err()
        );
        fs::remove_dir_all(target.parent().unwrap()).await.unwrap();
    }

    #[tokio::test]
    async fn parallel_segments_keep_order_and_resume_without_redownloading() {
        let fixture = Fixture::start(vec![7; 4096]);
        let client = Client::new();
        let target = test_target("parallel");
        let headers = HashMap::new();
        let start = Instant::now();
        for i in 0..8 {
            client
                .get(fixture.url.join(&format!("segment-{i}.ts")).unwrap())
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
        }
        let sequential = start.elapsed();
        fixture.requests.store(0, Ordering::Relaxed);
        let mut updates = Vec::new();
        let start = Instant::now();
        let prepared = prepare(&client, &fixture.url, &target, &headers, None, &mut |p| {
            updates.push(p);
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
        let parallel = start.elapsed();
        assert_eq!(fixture.peak.load(Ordering::Relaxed), 4);
        assert_eq!(fixture.requests.load(Ordering::Relaxed), 8);
        assert!(
            parallel < sequential,
            "parallel={parallel:?}, sequential={sequential:?}"
        );
        println!("HLS benchmark: sequential={sequential:?}; parallel={parallel:?}");
        let text = fs::read_to_string(&prepared.playlist).await.unwrap();
        let names = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            (0..8)
                .map(|i| format!("segment-{i}.ts"))
                .collect::<Vec<_>>()
        );
        assert_eq!(updates.last().unwrap().downloaded_bytes, 8 * 4096);
        prepare(&client, &fixture.url, &target, &headers, None, &mut |_| {
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            fixture.requests.load(Ordering::Relaxed),
            8,
            "completed segments must be reused"
        );
        prepared.cleanup().await;
        let _ = fs::remove_dir_all(target.parent().unwrap()).await;
    }
    #[tokio::test]
    #[ignore = "requires installed FFmpeg; generated localhost media only"]
    async fn separate_audio_master_produces_audible_mp4() {
        let target = test_target("separate-audio");
        let video = target.with_extension("video.ts");
        let audio = target.with_extension("audio.ts");
        for (filter, codec, output) in [
            ("color=c=blue:s=160x90:r=25", "libx264", &video),
            ("sine=frequency=440:sample_rate=44100", "aac", &audio),
        ] {
            let generated = Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", filter,
                       "-t", "1", "-c", codec, "-threads", "1", "-f", "mpegts"])
                .arg(output).output().unwrap();
            assert!(generated.status.success(), "{}", String::from_utf8_lossy(&generated.stderr));
        }
        let fixture = Fixture::start_tracks(fs::read(&video).await.unwrap(), Some(fs::read(&audio).await.unwrap()), 1);
        let master = fixture.url.join("split.m3u8").unwrap();
        let prepared = prepare(&Client::new(), &master, &target, &HashMap::new(), None, &mut |_| Ok(())).await.unwrap().unwrap();
        assert!(prepared.audio_playlist.is_some());
        assert_eq!(fixture.requests.load(Ordering::Relaxed), 1);
        DownloadService::new(Client::new()).download_stream_manifest_to_path(
            &[master], &target, super::super::StreamOptions {
                source_page_url: None, http_headers: &HashMap::new(), transfer: &Default::default(),
            }, &mut |_, _, _| Ok(()), &mut |_| Ok(()),
        ).await.unwrap();
        let probe = Command::new("ffprobe").args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "json"]).arg(&target).output().unwrap();
        assert!(probe.status.success());
        let data: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let tracks = data["streams"].as_array().unwrap();
        assert!(tracks.iter().any(|s| s["codec_type"] == "video"));
        assert!(tracks.iter().any(|s| s["codec_type"] == "audio"));
        let decoded = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&target)
            .args(["-map", "0:a:0", "-f", "s16le", "-"]).output().unwrap();
        assert!(decoded.status.success());
        assert!(decoded.stdout.iter().any(|b| *b != 0), "audio must decode to non-silent samples");
        let _ = fs::remove_dir_all(target.parent().unwrap()).await;
    }
    #[tokio::test]
    #[ignore = "requires installed FFmpeg; generated localhost media only"]
    async fn prefetched_hls_muxes_to_playable_mp4() {
        let target = test_target("mux");
        let source = target.with_extension("ts");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=160x90:r=25",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=44100",
                "-t",
                "1",
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-c:a",
                "aac",
                "-f",
                "mpegts",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let fixture = Fixture::start_count(fs::read(&source).await.unwrap(), 1);
        let old_workspace = hidden_work_dir(&target, "ytdlp");
        fs::create_dir_all(&old_workspace).await.unwrap();
        fs::write(old_workspace.join("movie.mp4.part-Frag1"), b"old fragment")
            .await
            .unwrap();
        let service = DownloadService::new(Client::new());
        service
            .download_stream_manifest_to_path(
                std::slice::from_ref(&fixture.url),
                &target,
                super::super::StreamOptions {
                    source_page_url: None,
                    http_headers: &HashMap::new(),
                    transfer: &Default::default(),
                },
                &mut |_, _, _| Ok(()),
                &mut |_| Ok(()),
            )
            .await
            .unwrap();
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&target)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            decoded.status.success(),
            "{}",
            String::from_utf8_lossy(&decoded.stderr)
        );
        assert!(!cache_path(&target).exists());
        assert!(!old_workspace.exists());
        let _ = fs::remove_dir_all(target.parent().unwrap()).await;
    }
}

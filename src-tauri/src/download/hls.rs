//! Bounded parallel fetch for plain, finite HLS media playlists.
//! Complex/encrypted/live playlists stay with FFmpeg's existing demuxer.
use super::*;

const PARALLEL: usize = 4;
const PLAYLIST_LIMIT: usize = 2 * 1024 * 1024;

pub(super) struct Prepared {
    pub playlist: PathBuf,
    cache: PathBuf,
}
pub(super) fn cache_path(target: &Path) -> PathBuf {
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
            if !["#EXTM3U", "#EXT-X-VERSION", "#EXT-X-INDEPENDENT-SEGMENTS"]
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

pub(super) async fn prepare(
    client: &Client,
    url: &Url,
    target: &Path,
    headers: &HashMap<String, String>,
    referer: Option<&str>,
    progress: &mut impl FnMut(DownloadProgress) -> Result<(), String>,
) -> Result<Option<Prepared>, String> {
    // Resolve a bounded master chain; unsupported media still uses FFmpeg.
    let mut next = url.clone();
    let mut visited = std::collections::HashSet::new();
    let mut media = None;
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
        let Some(variant) = master_variant(&text, &base) else {
            return Ok(None);
        };
        next = variant;
    }
    let Some((base, text, local, urls)) = media else {
        return Ok(None);
    };
    let cache = cache_path(target);
    // Completed segments are reusable only for the exact same playlist and URL.
    let fingerprint = format!("{:x}", Sha256::digest(format!("{base}\n{text}")));
    let folder = cache.join(fingerprint);
    fs::create_dir_all(&folder)
        .await
        .map_err(|e| format!("HLS cache: {e}"))?;
    let bytes = Arc::new(AtomicU64::new(0));
    let count = urls.len();
    let mut completed = 0usize;
    let credential_origin = url.clone();
    let jobs = futures_util::stream::iter(urls.into_iter().enumerate().map(|(index, url)| {
        let folder = folder.clone();
        let bytes = bytes.clone();
        let credential_origin = credential_origin.clone();
        async move {
            let output = folder.join(format!("segment-{index}.ts"));
            if let Ok(meta) = fs::metadata(&output).await {
                if meta.len() > 0 {
                    bytes.fetch_add(meta.len(), Ordering::Relaxed);
                    return Ok(());
                }
            }
            let part = folder.join(format!("segment-{index}.tmp"));
            for attempt in 0..3 {
                let result = async {
                    let response = request(client, &url, &credential_origin, headers, referer)
                        .send()
                        .await
                        .map_err(|e| e.to_string())?
                        .error_for_status()
                        .map_err(|e| e.to_string())?;
                    let file = fs::File::create(&part).await.map_err(|e| e.to_string())?;
                    let mut file = BufWriter::with_capacity(256 * 1024, file);
                    let mut stream = response.bytes_stream();
                    let mut written = 0u64;
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.map_err(|e| e.to_string())?;
                        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                        written += chunk.len() as u64;
                    }
                    file.flush().await.map_err(|e| e.to_string())?;
                    if written == 0 {
                        return Err("empty segment".to_string());
                    }
                    fs::rename(&part, &output)
                        .await
                        .map_err(|e| e.to_string())?;
                    bytes.fetch_add(written, Ordering::Relaxed);
                    Ok::<(), String>(())
                }
                .await;
                match result {
                    Ok(()) => return Ok(()),
                    Err(error) if attempt == 2 => {
                        return Err(format!("HLS segment {} failed: {error}", index + 1));
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
                }
            }
            unreachable!()
        }
    }))
    .buffer_unordered(PARALLEL);
    tokio::pin!(jobs);
    while let Some(result) = jobs.next().await {
        result?;
        completed += 1;
        let downloaded = bytes.load(Ordering::Relaxed);
        // This estimate is based on finished segment sizes, not the MP4 output.
        let estimated = downloaded.saturating_mul(count as u64) / (completed as u64);
        progress(DownloadProgress::bytes(
            downloaded,
            Some(estimated.max(downloaded)),
            PARALLEL.min(count - completed).max(1),
        ))?;
    }
    let playlist = folder.join("local.m3u8");
    fs::write(&playlist, local)
        .await
        .map_err(|e| format!("HLS playlist: {e}"))?;
    Ok(Some(Prepared { playlist, cache }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn master_selection_preserves_external_tracks_and_rejects_unsafe_sources() {
        let base = Url::parse("https://example.com/hls/master").unwrap();
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nlow/list.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=200,CODECS=\"avc1,mp4a\"\nhigh/list.m3u8\n";
        assert_eq!(
            master_variant(text, &base).unwrap().as_str(),
            "https://example.com/hls/high/list.m3u8"
        );
        assert!(
            master_variant(
                &text.replace("BANDWIDTH=100", "BANDWIDTH=100,AUDIO=\"audio\""),
                &base
            )
            .is_none()
        );
        assert!(
            master_variant(&format!("#EXT-X-MEDIA:TYPE=AUDIO,URI=audio\n{text}"), &base).is_none()
        );
        assert!(
            master_variant(&text.replace("low/list.m3u8", "file:///tmp/secret"), &base).is_none()
        );
        assert!(master_variant("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100", &base).is_none());
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
                    workers.push(std::thread::spawn(move || {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut input = [0; 4096];
                        let n = socket.read(&mut input).unwrap_or(0);
                        let header = String::from_utf8_lossy(&input[..n]);
                        let body = if header.starts_with("GET /playlist.m3u8 ") {
                            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\nmedia.m3u8\n".to_vec()
                        } else if header.starts_with("GET /media.m3u8 ") {
                            format!(
                                "#EXTM3U\n#EXT-X-ALLOW-CACHE:YES\n#EXT-X-TARGETDURATION:1\n{}#EXT-X-ENDLIST\n",
                                (0..8)
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
        let fixture = Fixture::start(fs::read(&source).await.unwrap());
        let service = DownloadService::new(Client::new());
        service
            .download_stream_manifest_to_path(
                std::slice::from_ref(&fixture.url),
                None,
                &HashMap::new(),
                &target,
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
        let _ = fs::remove_dir_all(target.parent().unwrap()).await;
    }
}

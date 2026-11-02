use super::*;
use std::sync::atomic::AtomicUsize;

// Tests use only loopback and disposable files; TMPDIR selects the fixture root.
struct Fixture {
    root: PathBuf,
    url: Url,
    requests: Arc<tokio::sync::Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(mode: &'static str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ldm-http-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/file.bin",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let observed = requests.clone();
        let server = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { break };
                        let observed = observed.clone();
                        tasks.spawn(async move {
                            let mut request = Vec::new();
                            let mut buf = [0; 1024];
                            loop {
                                let n = socket.read(&mut buf).await.unwrap_or(0);
                                if n == 0 { return; }
                                request.extend_from_slice(&buf[..n]);
                                if request.windows(4).any(|w| w == b"\r\n\r\n") { break; }
                            }
                            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                            let sequence = {
                                let mut requests = observed.lock().await;
                                requests.push(request.clone());
                                requests.len()
                            };
                            if mode == "headers-stall" || (mode == "range-headers-stall" && request.starts_with("get ")) {
                                std::future::pending::<()>().await;
                            }
                            let body = if mode == "changed" || (["change-between-ranges", "changed-on-resume", "date-changed", "lying-206"].contains(&mode) && sequence > 1) {
                                b"BBBBBBBBBBBB".as_slice()
                            } else { b"AAAAAAAAAAAA".as_slice() };
                            let validator = if mode.starts_with("date-") {
                                if body[0] == b'B' { "Thu, 22 Oct 2015 07:28:00 GMT" } else { "Wed, 21 Oct 2015 07:28:00 GMT" }
                            } else if body[0] == b'B' { "\"b\"" } else { "\"a\"" };
                            let range = request.lines().find_map(|l| l.strip_prefix("range: bytes="))
                                .and_then(|v| v.trim().split_once('-'))
                                .map(|(a, b)| (a.parse::<usize>().unwrap(), b.parse().unwrap_or(body.len() - 1)));
                            let if_range = request.lines().find_map(|l| l.strip_prefix("if-range: ")).map(str::trim);
                            let range = if mode != "lying-206" && if_range.is_some_and(|v| v != validator.to_ascii_lowercase()) { None } else { range };
                            let (start, end) = range.unwrap_or((0, body.len() - 1));
                            let status = if range.is_some() { "206 Partial Content" } else { "200 OK" };
                            let range_header = if range.is_some() { format!("Content-Range: bytes {start}-{end}/{}\r\n", body.len()) } else { String::new() };
                            let identity = if mode == "validatorless" {
                                String::new()
                            } else if mode.starts_with("date-") {
                                format!("Last-Modified: {validator}\r\nDate: Thu, 22 Oct 2015 07:30:00 GMT\r\n")
                            } else {
                                format!("ETag: {validator}\r\n")
                            };
                            let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n{range_header}{identity}Connection: close\r\n\r\n", end - start + 1);
                            if socket.write_all(head.as_bytes()).await.is_err() { return; }
                            if request.starts_with("head ") { return; }
                            if mode == "body-stall" {
                                std::future::pending::<()>().await;
                            }
                            if mode == "steady" {
                                for byte in &body[start..=end] {
                                    if socket.write_all(&[*byte]).await.is_err() { return; }
                                    tokio::time::sleep(Duration::from_millis(250)).await;
                                }
                            } else {
                                let _ = socket.write_all(&body[start..=end]).await;
                            }
                        });
                    }
                    _ = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        });
        Self {
            root,
            url,
            requests,
            server,
        }
    }

    fn target(&self) -> PathBuf {
        self.root.join("output.bin")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn single(
    service: &DownloadService,
    fixture: &Fixture,
    resume_from: u64,
) -> Result<(u64, Option<u64>, usize), String> {
    service
        .download_single(
            &fixture.url,
            &fixture.target(),
            SingleOptions {
                resume_from,
                total_bytes: Some(12),
                throttle: None,
            },
            &mut |_, _, _| Ok(()),
            &mut |_| Ok(()),
        )
        .await
}

#[tokio::test]
async fn validatorless_old_partial_is_not_mixed_with_same_length_new_content() {
    let fixture = Fixture::new("changed").await;
    let service = DownloadService::new(Client::new());
    fs::write(service.partial_path_for(&fixture.target()), b"AAAAAA")
        .await
        .unwrap();
    single(&service, &fixture, 6).await.unwrap();
    assert_eq!(fs::read(fixture.target()).await.unwrap(), b"BBBBBBBBBBBB");
    assert!(!fixture.requests.lock().await[0].contains("range: bytes=6-"));
}

#[tokio::test]
async fn http_identity_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("unchanged").await;
    let service = DownloadService::new(Client::new());
    prime_single_identity(&service, &fixture).await;
    assert_eq!(
        std::fs::metadata(http_identity_path(&fixture.target()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[tokio::test]
async fn http_identity_publication_does_not_follow_staging_symlinks() {
    let fixture = Fixture::new("unchanged").await;
    let service = DownloadService::new(Client::new());
    prime_single_identity(&service, &fixture).await;
    let identity = load_http_identity(&fixture.target())
        .await
        .unwrap()
        .unwrap();
    let victim = fixture.root.join("unrelated-secret");
    fs::write(&victim, b"unchanged").await.unwrap();
    std::os::unix::fs::symlink(
        &victim,
        http_identity_path(&fixture.target()).with_extension("json.tmp"),
    )
    .unwrap();
    let result = save_http_identity(&fixture.target(), &identity).await;
    assert_eq!(fs::read(&victim).await.unwrap(), b"unchanged");
    assert!(result.is_err());
}

async fn prime_single_identity(service: &DownloadService, fixture: &Fixture) {
    // Pause after headers, before reading the body: exercise real sidecar creation.
    let result = service
        .download_single(
            &fixture.url,
            &fixture.target(),
            SingleOptions {
                resume_from: 0,
                total_bytes: Some(12),
                throttle: None,
            },
            &mut |_, _, _| Err("fixture pause".into()),
            &mut |_| Ok(()),
        )
        .await;
    assert_eq!(result.unwrap_err(), "fixture pause");
    fs::write(service.partial_path_for(&fixture.target()), b"AAAAAA")
        .await
        .unwrap();
}

#[tokio::test]
async fn unchanged_strong_etag_resumes_actual_disk_prefix_using_if_range() {
    let fixture = Fixture::new("unchanged").await;
    let service = DownloadService::new(Client::new());
    prime_single_identity(&service, &fixture).await;
    // Deliberately stale UI progress: the disk, not this hint, owns the offset.
    single(&service, &fixture, 10).await.unwrap();
    assert_eq!(fs::read(fixture.target()).await.unwrap(), b"AAAAAAAAAAAA");
    let requests = fixture.requests.lock().await;
    assert!(requests[1].contains("range: bytes=6-\r\n"));
    assert!(requests[1].contains("if-range: \"a\"\r\n"));
    assert!(!http_identity_path(&fixture.target()).exists());
}

#[tokio::test]
async fn changed_same_length_etag_restarts_instead_of_appending() {
    let fixture = Fixture::new("changed-on-resume").await;
    let service = DownloadService::new(Client::new());
    prime_single_identity(&service, &fixture).await;
    single(&service, &fixture, 6).await.unwrap();
    assert_eq!(fs::read(fixture.target()).await.unwrap(), b"BBBBBBBBBBBB");
    assert!(fixture.requests.lock().await[1].contains("if-range: \"a\"\r\n"));
}

#[tokio::test]
async fn strong_last_modified_resumes_and_changed_date_restarts() {
    for (mode, expected) in [
        ("date-unchanged", b"AAAAAAAAAAAA"),
        ("date-changed", b"BBBBBBBBBBBB"),
    ] {
        let fixture = Fixture::new(mode).await;
        let service = DownloadService::new(Client::new());
        prime_single_identity(&service, &fixture).await;
        single(&service, &fixture, 6).await.unwrap();
        assert_eq!(fs::read(fixture.target()).await.unwrap(), expected);
        let requests = fixture.requests.lock().await;
        assert!(requests[1].contains("range: bytes=6-\r\n"));
        assert!(requests[1].contains("if-range: wed, 21 oct 2015 07:28:00 gmt\r\n"));
    }
}

#[tokio::test]
async fn conflicting_206_validator_is_rejected_before_append() {
    let fixture = Fixture::new("lying-206").await;
    let service = DownloadService::new(Client::new());
    prime_single_identity(&service, &fixture).await;
    assert!(single(&service, &fixture, 6).await.is_err());
    assert_eq!(
        fs::read(service.partial_path_for(&fixture.target()))
            .await
            .unwrap(),
        b"AAAAAA"
    );
    assert!(!fixture.target().exists());
}

async fn segmented(
    service: &DownloadService,
    fixture: &Fixture,
) -> Result<(u64, Option<u64>, usize), String> {
    service
        .download_with_segments(
            &fixture.url,
            &fixture.target(),
            build_segment_manifest(12),
            2,
            None,
            &mut |_, _, _| Ok(()),
            &mut |_| Ok(()),
        )
        .await
}

async fn seed_segment_prefix(service: &DownloadService, fixture: &Fixture, validated: bool) {
    let manifest = build_segment_manifest(12);
    service
        .save_segment_manifest(&fixture.target(), &manifest)
        .await
        .unwrap();
    fs::write(service.segment_path_for(&fixture.target(), 0), b"AAA")
        .await
        .unwrap();
    if validated {
        save_http_identity(
            &fixture.target(),
            &HttpRepresentation {
                source_url: fixture.url.to_string(),
                response_url: fixture.url.to_string(),
                total_bytes: Some(12),
                validator: HttpValidator::Etag("\"a\"".into()),
            },
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn unchanged_segment_prefix_is_retained_and_every_range_is_conditional() {
    let fixture = Fixture::new("unchanged").await;
    let service = DownloadService::new(Client::new());
    seed_segment_prefix(&service, &fixture, true).await;
    segmented(&service, &fixture).await.unwrap();
    assert_eq!(fs::read(fixture.target()).await.unwrap(), b"AAAAAAAAAAAA");
    let requests = fixture.requests.lock().await;
    let ranges: Vec<_> = requests
        .iter()
        .filter(|request| request.starts_with("get "))
        .collect();
    assert_eq!(ranges.len(), 2);
    assert!(
        ranges
            .iter()
            .any(|request| request.contains("range: bytes=3-5\r\n"))
    );
    assert!(
        ranges
            .iter()
            .all(|request| request.contains("if-range: \"a\"\r\n"))
    );
}

#[tokio::test]
async fn validatorless_or_changed_segment_prefix_is_never_reused() {
    for validated in [false, true] {
        let fixture = Fixture::new("changed").await;
        let service = DownloadService::new(Client::new());
        seed_segment_prefix(&service, &fixture, validated).await;
        segmented(&service, &fixture).await.unwrap();
        assert_eq!(fs::read(fixture.target()).await.unwrap(), b"BBBBBBBBBBBB");
        let requests = fixture.requests.lock().await;
        assert!(
            !requests
                .iter()
                .any(|request| request.contains("range: bytes=3-5\r\n"))
        );
    }
}

#[tokio::test]
async fn representation_change_between_parallel_ranges_falls_back_to_clean_full_get() {
    let fixture = Fixture::new("change-between-ranges").await;
    let service = DownloadService::new(Client::new());
    segmented(&service, &fixture).await.unwrap();
    assert_eq!(fs::read(fixture.target()).await.unwrap(), b"BBBBBBBBBBBB");
    assert!(!service.manifest_path_for(&fixture.target()).exists());
    assert!(!service.segment_path_for(&fixture.target(), 0).exists());
}

#[tokio::test]
async fn segment_rejects_conflicting_206_before_appending_to_validated_prefix() {
    let fixture = Fixture::new("lying-206").await;
    let service = DownloadService::new(Client::new());
    seed_segment_prefix(&service, &fixture, true).await;
    assert!(segmented(&service, &fixture).await.is_err());
    assert_eq!(
        fs::read(service.segment_path_for(&fixture.target(), 0))
            .await
            .unwrap(),
        b"AAA"
    );
    assert!(!fixture.target().exists());
}

#[tokio::test]
async fn server_without_strong_validator_uses_only_one_full_body() {
    let fixture = Fixture::new("validatorless").await;
    let service = DownloadService::new(Client::new());
    seed_segment_prefix(&service, &fixture, true).await;
    segmented(&service, &fixture).await.unwrap();
    assert_eq!(fs::read(fixture.target()).await.unwrap(), b"AAAAAAAAAAAA");
    let requests = fixture.requests.lock().await;
    let gets: Vec<_> = requests
        .iter()
        .filter(|request| request.starts_with("get "))
        .collect();
    assert_eq!(gets.len(), 1);
    assert!(!gets[0].contains("range:"));
}

#[test]
fn only_strong_validators_are_eligible_for_if_range() {
    let mut headers = HeaderMap::new();
    headers.insert(
        LAST_MODIFIED,
        "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
    );
    assert!(HttpValidator::from_headers(&headers).is_none());
    headers.insert(DATE, "Wed, 21 Oct 2015 07:28:59 GMT".parse().unwrap());
    assert!(HttpValidator::from_headers(&headers).is_none());
    headers.insert(DATE, "Wed, 21 Oct 2015 07:29:00 GMT".parse().unwrap());
    assert!(matches!(
        HttpValidator::from_headers(&headers),
        Some(HttpValidator::LastModified(_))
    ));
    headers.insert(ETAG, "W/\"weak\"".parse().unwrap());
    assert!(HttpValidator::from_headers(&headers).is_none());
    headers.insert(ETAG, "\"strong\"".parse().unwrap());
    assert_eq!(
        HttpValidator::from_headers(&headers),
        Some(HttpValidator::Etag("\"strong\"".into()))
    );
}

#[tokio::test]
async fn inspect_and_single_header_waits_are_bounded() {
    let fixture = Fixture::new("headers-stall").await;
    let service = DownloadService::new(Client::new());
    let inspected = tokio::time::timeout(
        Duration::from_secs(6),
        service.inspect_url(fixture.url.as_str()),
    )
    .await;
    assert!(
        inspected.is_ok(),
        "inspection waited indefinitely for headers"
    );
    assert!(inspected.unwrap().is_err());
    let result = tokio::time::timeout(Duration::from_secs(4), single(&service, &fixture, 0)).await;
    assert!(result.is_ok(), "download waited indefinitely for headers");
    assert!(result.unwrap().is_err());
    let result = tokio::time::timeout(Duration::from_secs(4), segmented(&service, &fixture)).await;
    assert!(
        result.is_ok(),
        "segment validation waited indefinitely for HEAD headers"
    );
    assert!(result.unwrap().is_err());
}

#[tokio::test]
async fn segment_request_headers_have_bounded_retries() {
    let fixture = Fixture::new("range-headers-stall").await;
    let service = DownloadService::new(Client::new());
    let result = tokio::time::timeout(Duration::from_secs(12), segmented(&service, &fixture)).await;
    assert!(
        result.is_ok(),
        "segment requests waited indefinitely for headers"
    );
    assert!(result.unwrap().is_err());
}

#[tokio::test]
async fn single_idle_read_is_bounded_without_total_transfer_deadline() {
    let service = DownloadService::new(Client::new());
    let fixture = Fixture::new("body-stall").await;
    let result = tokio::time::timeout(Duration::from_secs(4), single(&service, &fixture, 0)).await;
    assert!(result.is_ok(), "download waited indefinitely for body data");
    assert!(result.unwrap().is_err());
    let fixture = Fixture::new("steady").await;
    single(&service, &fixture, 0).await.unwrap();
    assert_eq!(fs::read(fixture.target()).await.unwrap(), b"AAAAAAAAAAAA");
}

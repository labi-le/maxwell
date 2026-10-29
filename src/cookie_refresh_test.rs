use crate::prelude::*;
use crate::test_support::*;
use crate::*;

#[test]
fn cookie_source_maps_supported_hosts_to_refresh_sources() {
    for (raw, expected) in [
        ("https://youtu.be/abc", "youtube"),
        ("https://www.youtube.com/watch?v=abc", "youtube"),
        ("https://vt.tiktok.com/abc", "tiktok"),
        ("https://www.tiktok.com/@user/photo/123", "tiktok"),
        ("https://pin.it/abc", "pinterest"),
        ("https://www.pinterest.co.uk/pin/123", "pinterest"),
        ("https://www.instagram.com/p/abc/", "instagram"),
        ("https://mobile.twitter.com/user/status/1", "twitter"),
        ("https://x.com/user/status/1", "twitter"),
        ("https://example.com/post", "generic"),
    ] {
        assert_eq!(cookie_source(&Url::parse(raw).unwrap()), expected, "{raw}");
    }
}

#[test]
fn only_explicit_cookie_auth_details_request_refresh() {
    for detail in [
        "Authentication required",
        "Cookies expired",
        "Cookies unavailable",
    ] {
        assert!(is_cookie_auth_detail(detail), "{detail}");
    }
    for detail in ["HTTP 403", "HTTP 401", "Download failed", "Timeout"] {
        assert!(!is_cookie_auth_detail(detail), "{detail}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_warning_regression_invalid_cookie_warning_and_403_refresh_once_and_complete_video()
{
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_warning_regression_fixture(root.path(), true, false).await;
    let (url, calls, sources, server) = fake_cookie_service(source.clone(), 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("invalid-cookie warning refresh stalled")
    .unwrap();
    server.abort();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the warning must submit exactly one refresh request"
    );
    assert_eq!(sources.lock().unwrap().as_slice(), ["youtube"]);
    assert!(
        result.process_ok,
        "one acknowledged refresh must complete the download"
    );
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        2,
        "one failed download plus exactly one retry"
    );
    assert_eq!(result.selection.files.len(), 1);
    assert_eq!(result.selection.files[0].kind, MediaKind::Video);
    assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"video");
    assert_eq!(
        fs::read_to_string(work.path().join("completed-paths.txt")).unwrap(),
        format!("{}\n", result.selection.files[0].path.display())
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(true)
    );
    assert_eq!(
        result
            .events
            .first()
            .map(|event| (event.stage, event.detail, event.exit_code)),
        Some(("yt-dlp", "Cookies expired", Some(1))),
        "invalid-cookie warning must take precedence over the accompanying HTTP 403"
    );
    let lifecycle: Vec<_> = result
        .events
        .iter()
        .filter(|event| {
            (event.stage == "refresh"
                && matches!(event.detail, "Request submitted" | "Acknowledged"))
                || (event.stage == "retry" && event.detail == "Started")
        })
        .map(|event| (event.stage, event.detail))
        .collect();
    assert_eq!(
        lifecycle,
        vec![
            ("refresh", "Request submitted"),
            ("refresh", "Acknowledged"),
            ("retry", "Started"),
        ],
        "the single retry must follow the request and its acknowledgement"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_warning_regression_successful_download_does_not_refresh_or_retry() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_warning_regression_fixture(root.path(), true, true).await;
    let (url, calls, _sources, server) = fake_cookie_service(source.clone(), 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(2),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(1),
        ),
    )
    .await
    .expect("successful download must not wait for cookie refresh")
    .unwrap();
    server.abort();
    assert!(result.process_ok);
    assert_eq!(result.selection.files.len(), 1);
    assert_eq!(result.selection.files[0].kind, MediaKind::Video);
    assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"video");
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        1,
        "successful media must not be downloaded again"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a warning during a successful download must not request fresh cookies"
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event.stage, "refresh" | "retry")),
        "a successful download must not enter the refresh lifecycle"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(false)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_warning_regression_bare_403_does_not_refresh_or_retry() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_warning_regression_fixture(root.path(), false, false).await;
    let (url, calls, _sources, server) = fake_cookie_service(source.clone(), 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(2),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(1),
        ),
    )
    .await
    .expect("bare HTTP 403 must not wait for cookie refresh")
    .unwrap();
    server.abort();
    assert!(!result.process_ok);
    assert!(result.selection.files.is_empty());
    assert_eq!(
        result
            .diagnostic
            .map(|diagnostic| (diagnostic.detail, diagnostic.exit_code)),
        Some(("HTTP 403", Some(1)))
    );
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        1,
        "bare HTTP 403 must not trigger another download"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "HTTP 403 alone must not request fresh cookies"
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event.stage, "refresh" | "retry")),
        "bare HTTP 403 must not enter the refresh lifecycle"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(false)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_refresh_service_unavailable_keeps_original_failure() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_fixture(root.path(), true).await;
    let (url, calls, sources, server) = fake_cookie_service(source.clone(), 503).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let url = allowed_url("https://youtu.be/abc").unwrap();
    let result = time::timeout(
        Duration::from_secs(3),
        download_with_timeout(&url, &cfg, work.path(), Duration::from_secs(3)),
    )
    .await
    .expect("authentication request exceeded download budget")
    .unwrap();
    server.abort();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(sources.lock().unwrap().as_slice(), ["youtube"]);
    assert!(!result.process_ok);
    assert_eq!(
        result.diagnostic.map(|diagnostic| diagnostic.detail),
        Some("Authentication required")
    );
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        1,
        "a failed service must not retry the download"
    );
    let lifecycle: Vec<_> = result
        .events
        .iter()
        .filter(|event| event.stage == "refresh")
        .map(|event| event.detail)
        .collect();
    assert_eq!(lifecycle, ["Request submitted", "Request failed"]);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(false)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_preflight_refreshes_corrupt_jar_before_video() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_fixture(root.path(), false).await;
    fs::write(&source, b"synthetic corrupt jar\n").unwrap();
    let (url, calls, sources, server) = fake_cookie_service(source.clone(), 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("corrupt jar refresh stalled")
    .unwrap();
    server.abort();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        sources
            .lock()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["youtube"]
    );
    assert!(result.process_ok);
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        1,
        "the preflight refresh must happen before the download attempt"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(true)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_preflight_storage_failure_does_not_request_refresh() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_fixture(root.path(), false).await;
    let (url, calls, _sources, server) = fake_cookie_service(source.clone(), 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    fs::set_permissions(work.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = download_with_timeout(
        &allowed_url("https://youtu.be/abc").unwrap(),
        &cfg,
        work.path(),
        Duration::from_secs(3),
    )
    .await;
    fs::set_permissions(work.path(), fs::Permissions::from_mode(0o700)).unwrap();
    server.abort();
    let failure = match result {
        Ok(_) => panic!("an unwritable work directory must fail"),
        Err(failure) => failure,
    };
    assert_eq!(failure.diagnostic.detail, "File I/O error");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a storage failure must not request fresh cookies"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(false)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_refresh_connection_refused_keeps_original_failure() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_fixture(root.path(), true).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    cfg.cookie_refresh_url = Some(Url::parse(&format!("http://{address}")).unwrap());
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(3),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("connection-refused refresh stalled")
    .unwrap();
    assert!(!result.process_ok);
    assert_eq!(
        result.diagnostic.map(|diagnostic| diagnostic.detail),
        Some("Authentication required")
    );
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        1,
        "a refused service must not retry the download"
    );
    let lifecycle: Vec<_> = result
        .events
        .iter()
        .filter(|event| event.stage == "refresh")
        .map(|event| event.detail)
        .collect();
    assert_eq!(lifecycle, ["Request submitted", "Request failed"]);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(false)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_refresh_ack_retries_with_new_cookie_copy_and_completes_video() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_fixture(root.path(), false).await;
    let (url, calls, sources, server) = fake_cookie_service(source.clone(), 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let url = allowed_url("https://youtu.be/abc").unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(&url, &cfg, work.path(), Duration::from_secs(3)),
    )
    .await
    .expect("refresh retry stalled")
    .unwrap();
    server.abort();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one failed download must submit one refresh request"
    );
    assert_eq!(
        sources
            .lock()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["youtube"]
    );
    assert!(
        result.process_ok,
        "acknowledged refresh must allow one retry"
    );
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        2,
        "one failed download plus one retry"
    );
    assert_eq!(result.selection.files.len(), 1);
    assert_eq!(result.selection.files[0].kind, MediaKind::Video);
    assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"video");
    let manifest = fs::read_to_string(work.path().join("completed-paths.txt")).unwrap();
    assert_eq!(
        manifest.lines().collect::<Vec<_>>(),
        vec![result.selection.files[0].path.to_str().unwrap()]
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        cookie_refresh_jar(true)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_refresh_second_auth_error_keeps_original_failure_without_third_attempt() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_fixture(root.path(), true).await;
    let (url, calls, _sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let url = allowed_url("https://youtu.be/abc").unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(&url, &cfg, work.path(), Duration::from_secs(3)),
    )
    .await
    .expect("second authentication failure stalled")
    .unwrap();
    server.abort();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the refresh must be requested once"
    );
    assert!(!result.process_ok);
    assert_eq!(
        result
            .diagnostic
            .map(|diagnostic| (diagnostic.detail, diagnostic.exit_code)),
        Some(("Authentication required", Some(1)))
    );
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        2,
        "refresh must never trigger a third download"
    );
    assert!(result.selection.files.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_refresh_partial_auth_retry_exposes_finished_video_and_keeps_first_diagnostic() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_partial_fixture(root.path(), true).await;
    let (url, calls, _sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("partial retry stalled")
    .unwrap();
    server.abort();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        2,
        "refresh must never trigger a third download"
    );
    assert_eq!(
        fs::read_to_string(work.path().join("completed-paths.txt")).unwrap(),
        format!("{}\n", work.path().join("finished.mp4").display())
    );
    assert!(
        !result.process_ok,
        "the retry exited with an authentication error"
    );
    assert_eq!(
        result.selection.files.len(),
        1,
        "completed media must survive a failed auth retry"
    );
    assert_eq!(result.selection.files[0].kind, MediaKind::Video);
    assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"video");
    assert_eq!(
        result.diagnostic.map(|diagnostic| (
            diagnostic.stage,
            diagnostic.detail,
            diagnostic.exit_code
        )),
        Some(("yt-dlp", "Authentication required", Some(1)))
    );
    assert_eq!(
        result
            .events
            .first()
            .map(|event| (event.stage, event.detail, event.exit_code)),
        Some(("yt-dlp", "Authentication required", Some(1)))
    );
    assert_eq!(
        result
            .events
            .last()
            .map(|event| (event.stage, event.detail, event.exit_code)),
        Some(("yt-dlp", "Authentication required", Some(9)))
    );
    let ack = result
        .events
        .iter()
        .position(|event| event.stage == "refresh" && event.detail == "Acknowledged")
        .expect("refresh acknowledgement missing from trace");
    let retry = result
        .events
        .iter()
        .position(|event| event.stage == "retry" && event.detail == "Started")
        .expect("retry start missing from trace");
    assert!(0 < ack && ack < retry && retry < result.events.len() - 1);
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_refresh_partial_non_auth_retry_keeps_first_diagnostic_and_traces_http_error() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_partial_fixture(root.path(), false).await;
    let (url, calls, _sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("non-auth retry stalled")
    .unwrap();
    server.abort();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        2,
        "refresh must never trigger a third download"
    );
    assert!(
        !work.path().join("completed-paths.txt").exists(),
        "no download should be completed"
    );
    assert!(!result.process_ok);
    assert!(result.selection.files.is_empty());
    assert_eq!(
        result.diagnostic.map(|diagnostic| (
            diagnostic.stage,
            diagnostic.detail,
            diagnostic.exit_code
        )),
        Some(("yt-dlp", "Authentication required", Some(1))),
        "the parent diagnostic must retain the original authentication failure"
    );
    assert_eq!(
        result
            .events
            .first()
            .map(|event| (event.stage, event.detail, event.exit_code)),
        Some(("yt-dlp", "Authentication required", Some(1)))
    );
    assert_eq!(
        result
            .events
            .last()
            .map(|event| (event.stage, event.detail, event.exit_code)),
        Some(("yt-dlp", "HTTP 403", Some(9)))
    );
    let ack = result
        .events
        .iter()
        .position(|event| event.stage == "refresh" && event.detail == "Acknowledged")
        .expect("refresh acknowledgement missing from trace");
    let retry = result
        .events
        .iter()
        .position(|event| event.stage == "retry" && event.detail == "Started")
        .expect("retry start missing from trace");
    assert!(0 < ack && ack < retry && retry < result.events.len() - 1);
}

#[cfg(unix)]
#[tokio::test]
async fn cookie_refresh_non_auth_failure_does_not_request_or_retry() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("mounted-cookies.txt");
    fs::write(&source, cookie_refresh_jar(false)).unwrap();
    let script = format!("#!/bin/sh\n# CUSTOM_METADATA\ncase \"$*\" in *--dump-single-json*) printf '%s\\n' '{{\"formats\":[]}}'; exit 0 ;; esac\nprintf 'attempt\\n' >> '{}/attempts'\nprintf '%s\\n' 'ERROR: [youtube] HTTP Error 403' >&2\nexit 1\n", root.path().display());
    let mut cfg = fake_settings(root.path(), &script).await;
    cfg.cookies_file = Some(source.clone());
    let (url, calls, _sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(2),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(1),
        ),
    )
    .await
    .expect("non-auth failure must not wait for refresh")
    .unwrap();
    server.abort();
    assert!(!result.process_ok);
    assert_eq!(
        result.diagnostic.map(|diagnostic| diagnostic.detail),
        Some("HTTP 403")
    );
    assert_eq!(cookie_refresh_attempts(root.path()), 1);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "non-auth errors must not request fresh cookies"
    );
}

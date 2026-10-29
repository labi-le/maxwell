use crate::feature_support::*;
use crate::test_support::*;
use crate::*;

#[cfg(unix)]
#[tokio::test]
async fn image_only_metadata_produces_photo_and_mixed_video_stays_complete() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(root.path(), r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
    case "$*" in
      *pinterest*) printf '%s\n' '{"formats":[],"thumbnails":[{"url":"https://i.pinimg.com/originals/pin.jpg","width":1600,"height":2400}]}' ;;
      *instagram.com/p/photo*) printf '%s\n' '{"formats":[],"thumbnails":[{"url":"https://scontent.cdninstagram.com/original.jpg","width":1200,"height":1600}]}' ;;
      *) printf '%s\n' '{"entries":[{"formats":[{"url":"https://example.org/video.mp4"}],"thumbnails":[{"url":"https://scontent.cdninstagram.com/cover.jpg","width":4000,"height":4000}]},{"formats":[],"thumbnails":[{"url":"https://scontent.cdninstagram.com/photo.jpg","width":1200,"height":1600}]}]}' ;;
    esac
    exit 0
  fi
done
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf video > "$dir/finished.mp4"
printf fragment > "$dir/unfinished.mp4.part"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
exit 9
"#).await;
    for (link, source, video_expected) in [
        ("https://pinterest.com/pin/123", "pinterest", false),
        ("https://pinterest.ca/pin/123", "pinterest", false),
        ("https://www.pinterest.co.uk/pin/456", "pinterest", false),
        ("https://instagram.com/p/photo/", "instagram", false),
        ("https://instagram.com/p/post/", "instagram", true),
    ] {
        let dir = tempfile::tempdir_in(root.path()).unwrap();
        let url = allowed_url(link).unwrap();
        let info = extract_images(&url, &settings, source, dir.path(), Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(info.has_video, video_expected);
        assert_eq!(info.images.len(), 1);
        let manifest = dir.path().join("completed-paths.txt");
        if video_expected {
            let video = download_videos(&url, &settings, dir.path(), Duration::from_secs(3))
                .await
                .unwrap();
            assert!(!video.process_ok);
            assert_eq!(video.selection.files.len(), 1);
        }
        let response = mock_photo_response("200 OK", b"photo", true, "image/jpeg").await;
        let photo = dir.path().join("photo-00.jpg");
        save_image_response(response, dir.path(), "photo-00.jpg", 100, 100, Some("jpg"))
            .await
            .unwrap();
        assert_eq!(fs::read(&photo).unwrap(), b"photo");
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&manifest)
            .unwrap()
            .write_all(format!("{}\n", photo.display()).as_bytes())
            .unwrap();
        let selected = select_media(dir.path(), &manifest, 100).unwrap();
        assert_eq!(selected.files.len(), if video_expected { 2 } else { 1 });
        assert_eq!(selected.files.last().unwrap().kind, MediaKind::Photo);
        assert!(!selected
            .files
            .iter()
            .any(|file| file.path.ends_with("unfinished.mp4.part")));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn metadata_auth_refresh_retries_pinterest_and_instagram_once() {
    for (label, link, expected_source) in [
        ("pinterest", "https://pinterest.com/pin/123", "pinterest"),
        ("instagram", "https://www.instagram.com/p/abc/", "instagram"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("mounted-cookies.txt");
        fs::write(&source, cookie_refresh_jar(false)).unwrap();
        let script = format!(
            r#"#!/bin/sh
# CUSTOM_METADATA
cookie=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --cookies) shift; cookie=$1 ;;
  esac
  shift
done
printf 'metadata\n' >> '{}/metadata-attempts'
[ -f "$cookie" ] || exit 22
case "$(cat "$cookie")" in
  *fresh-synthetic-marker*) ;;
  *) printf '%s\n' 'ERROR: [metadata] cookies are no longer valid' >&2; exit 1 ;;
esac
printf '%s\n' '{{"entries":[]}}'
"#,
            root.path().display()
        );
        let mut cfg = fake_settings(root.path(), &script).await;
        cfg.cookies_file = Some(source.clone());
        let (url, calls, sources, server) = fake_cookie_service(source, 200).await;
        cfg.cookie_refresh_url = Some(url);
        let work = tempfile::tempdir_in(root.path()).unwrap();
        let result = time::timeout(
            Duration::from_secs(4),
            download_with_timeout(
                &allowed_url(link).unwrap(),
                &cfg,
                work.path(),
                Duration::from_secs(3),
            ),
        )
        .await
        .expect("metadata refresh stalled")
        .unwrap();
        server.abort();
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "{label}"
        );
        assert_eq!(
            sources
                .lock()
                .unwrap()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [expected_source],
            "{label}"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("metadata-attempts"))
                .unwrap()
                .lines()
                .count(),
            2,
            "{label}"
        );
        assert!(result.process_ok, "{label}");
        assert!(result.selection.files.is_empty(), "{label}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn tiktok_metadata_refresh_retries_after_cookies_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("mounted-cookies.txt");
    fs::write(&source, cookie_refresh_jar(false)).unwrap();
    let mut cfg = fake_settings(root.path(), "#!/bin/sh\nexit 1\n").await;
    cfg.cookies_file = Some(source.clone());
    let (url, calls, sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let failure = match time::timeout(
        Duration::from_secs(4),
        download_with_timeout(
            &allowed_url("https://www.tiktok.com/@user/photo/123").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("tiktok metadata refresh stalled")
    {
        Ok(_) => panic!("tiktok metadata must fail when the refreshed jar lacks TikTok cookies"),
        Err(failure) => failure,
    };
    server.abort();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        sources
            .lock()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["tiktok"]
    );
    assert_eq!(failure.diagnostic.detail, "Cookies unavailable");
    assert!(failure
        .events
        .iter()
        .any(|event| event.stage == "refresh" && event.detail == "Acknowledged"));
    assert!(failure
        .events
        .iter()
        .any(|event| event.stage == "photo-page" && event.detail == "Cookies unavailable"));
}

#[cfg(unix)]
#[tokio::test]
async fn twitter_photo_auth_refresh_retries_helper_without_repeating_video() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("mounted-cookies.txt");
    fs::write(&source, cookie_refresh_jar(false)).unwrap();
    let video_script = format!(
        r#"#!/bin/sh
# CUSTOM_METADATA
printf 'video\n' >> '{}/video-attempts'
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${{1%/*}} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf video > "$dir/finished.mp4"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
"#,
        root.path().display()
    );
    let mut cfg = fake_settings(root.path(), &video_script).await;
    cfg.cookies_file = Some(source.clone());
    let helper = root.path().join("fake-python");
    fs::write(
        &helper,
        format!(
            r#"#!/bin/sh
printf 'helper\n' >> '{}/helper-attempts'
case "$(cat "$3")" in
  *fresh-synthetic-marker*) printf '%s\n' '{{"photos":[],"description":null,"has_video":true}}' ;;
  *) exit 3 ;;
esac
"#,
            root.path().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    cfg.python_bin = helper;
    let (url, calls, sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(
            &allowed_url("https://x.com/user/status/123").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("twitter photo refresh stalled")
    .unwrap();
    server.abort();
    assert_eq!(
        fs::read_to_string(root.path().join("helper-attempts"))
            .unwrap()
            .lines()
            .count(),
        2,
        "the photo helper must be retried once"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("video-attempts"))
            .unwrap()
            .lines()
            .count(),
        1,
        "the completed video must not be downloaded twice"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        sources
            .lock()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["twitter"]
    );
    assert!(result.process_ok);
    assert!(!result.twitter_photos_failed);
    assert_eq!(result.selection.files.len(), 1);
    assert_eq!(result.selection.files[0].kind, MediaKind::Video);
    assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"video");
}

#[cfg(unix)]
#[tokio::test]
async fn twitter_photo_failed_retry_keeps_authentication_diagnostic() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("mounted-cookies.txt");
    fs::write(&source, cookie_refresh_jar(false)).unwrap();
    let mut cfg = fake_settings(root.path(), "#!/bin/sh\nexit 1\n").await;
    cfg.cookies_file = Some(source.clone());
    let helper = root.path().join("fake-python");
    fs::write(
        &helper,
        format!(
            r#"#!/bin/sh
printf 'helper\n' >> '{}/helper-attempts'
exit 3
"#,
            root.path().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    cfg.python_bin = helper;
    let (url, calls, _sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(4),
        download_with_timeout(
            &allowed_url("https://x.com/user/status/123").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("failed twitter photo refresh stalled")
    .unwrap();
    server.abort();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        fs::read_to_string(root.path().join("helper-attempts"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(!result.process_ok);
    assert!(result.twitter_photos_failed);
    assert!(result.selection.files.is_empty());
    assert_eq!(
        result
            .diagnostic
            .map(|diagnostic| (diagnostic.stage, diagnostic.detail)),
        Some(("metadata", "Authentication required"))
    );
    assert!(result
        .events
        .iter()
        .any(|event| event.stage == "refresh" && event.detail == "Acknowledged"));
    assert!(result
        .events
        .iter()
        .any(|event| event.stage == "metadata" && event.detail == "Authentication required"));
}

#[cfg(unix)]
#[tokio::test]
async fn features_aggregate_storage_budget_is_not_a_per_file_size_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = fake_settings(root.path(), r#"#!/bin/sh
dir=; while [ "$#" -gt 0 ]; do if [ "$1" = --output ]; then shift; dir="${1%/*}"; fi; shift; done
for n in 1 2 3 4 5 6 7 8 9 10 11; do dd if=/dev/zero of="$dir/$n.part" bs=91 count=1 status=none; done
sleep 60
"#).await;
    settings.max_media_bytes = 100;
    let cfg = Arc::new(settings);
    let (bot, calls, server) = recording_bot(true, true).await;
    let message = serde_json::from_value(serde_json::json!({"message_id": 7, "date": 1700000000,
            "chat": {"id": 1, "type": "private"}, "from": {"id": 1, "is_bot": false, "first_name": "Owner"},
            "text": "https://www.youtube.com/playlist?list=quota"})).unwrap();
    time::timeout(
        Duration::from_secs(3),
        handle_message(bot, message, Arc::clone(&cfg)),
    )
    .await
    .unwrap()
    .unwrap();
    let detail: String = sqlx::query_scalar("SELECT detail FROM download_failures")
        .fetch_one(&cfg.database)
        .await
        .unwrap();
    assert_eq!(detail, "Temporary storage limit");
    let calls = calls.lock().await;
    let reply: Value = serde_json::from_str(
        &calls
            .iter()
            .find(|(path, _)| api_method_is(path, "sendMessage"))
            .unwrap()
            .1,
    )
    .unwrap();
    assert!(
        reply["text"].as_str().unwrap().contains("временн"),
        "explain aggregate storage, not a file's byte cap"
    );
    assert!(
        calls
            .iter()
            .all(|(path, _)| api_method_is(path, "sendMessage")),
        "no delivery or source deletion after quota failure"
    );
    server.abort();
}

#[tokio::test]
async fn features_auth_retry_timeout_overrides_auth_for_user_while_preserving_auth_provenance() {
    let root = tempfile::tempdir().unwrap();
    let cookies = root.path().join("mounted-cookies.txt");
    fs::write(&cookies, cookie_refresh_jar(false)).unwrap();
    fs::write(root.path().join("oversized-fixture.mp4"), [0u8; 1025]).unwrap();
    let script = format!(
        r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
    printf '%s\n' '{{"formats":[]}}'
    exit 0
  fi
done
cookie=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --cookies) shift; cookie=$1 ;;
    --output) shift; dir=${{1%/*}} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf 'attempt\n' >> '{0}/attempts'
[ -f "$cookie" ] || exit 22
case "$(cat "$cookie")" in
  *fresh-synthetic-marker*)
    cp '{0}/oversized-fixture.mp4' "$dir/finished.mp4" || exit 23
    printf '%s\n' "$dir/finished.mp4" > "$manifest"
    exec sleep 5
    ;;
  *)
    printf '%s\n' 'ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies.' >&2
    exit 1
    ;;
esac
"#,
        root.path().display()
    );
    let mut cfg = fake_settings(root.path(), &script).await;
    cfg.cookies_file = Some(cookies.clone());
    cfg.max_media_bytes = 1024;
    let (url, calls, _sources, server) = fake_cookie_service(cookies, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = time::timeout(
        Duration::from_secs(6),
        download_with_timeout(
            &allowed_url("https://youtu.be/abc").unwrap(),
            &cfg,
            work.path(),
            Duration::from_secs(3),
        ),
    )
    .await
    .expect("retry must respect the short download deadline")
    .unwrap();
    server.abort();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the fresh cookie request must be acknowledged"
    );
    assert_eq!(
        cookie_refresh_attempts(root.path()),
        2,
        "the acknowledged refresh permits exactly one retry"
    );
    assert_eq!(
        fs::metadata(work.path().join("finished.mp4"))
            .unwrap()
            .len(),
        1025
    );
    assert_eq!(
        fs::read_to_string(work.path().join("completed-paths.txt")).unwrap(),
        format!("{}\n", work.path().join("finished.mp4").display())
    );
    assert!(
        disk_usage(work.path()).unwrap() < cfg.max_media_bytes * MAX_FILES as u64,
        "the retry must time out, not hit the aggregate disk limit first"
    );
    assert!(!result.process_ok);
    assert!(
        !result.limited,
        "disk limiting must not replace the deadline failure"
    );
    assert!(
        result.selection.files.is_empty(),
        "the completed oversized file is not sendable media"
    );
    assert_eq!(result.selection.size_skipped, 1);
    let original = result
        .diagnostic
        .expect("the original authentication diagnostic must remain");
    assert_eq!(original.detail, "Authentication required");
    assert!(result
        .events
        .iter()
        .any(|event| event.stage == "refresh" && event.detail == "Acknowledged"));
    assert!(
        result.events.iter().any(|event| event.detail == "Timeout"),
        "the real retry timeout must remain in the diagnostic event stream"
    );
    let user_diagnostic = source_user_diagnostic(original, &result.events);
    assert_eq!(user_diagnostic.detail, "Timeout",
            "the user's latest actionable retry failure is timeout, not the original authentication cause");
    let reason =
        user_errors::diagnostic_message(user_diagnostic, cfg.max_media_bytes).to_lowercase();
    assert!(
        ["ожидан", "время", "timeout", "time"]
            .iter()
            .any(|cause| reason.contains(cause)),
        "the public retry failure must explain waiting/time: {reason}"
    );
    assert!(
        !reason.contains("авторизац") && !reason.contains("cookies"),
        "a completed authentication refresh must not hide the later timeout: {reason}"
    );
}

#[tokio::test]
async fn features_latest_actionable_failure_after_cookie_retry_reaches_the_user_safely() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, cookies) = cookie_refresh_partial_fixture(root.path(), false).await;
    cfg.max_media_bytes = 1_000_000;
    let (url, calls, _sources, server) = fake_cookie_service(cookies, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let fixture = FeatureFixture {
        root,
        cfg: Arc::new(cfg),
        api: FeatureApi::new().await,
    };
    fixture.send(1, 1, "Owner", "https://youtu.be/abc").await;
    server.abort();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "retry fixture must acknowledge the requested cookies"
    );
    let requests = fixture.api.snapshot().await;
    feature_retained(&requests);
    feature_reason(&requests, &["403", "отказ", "запрет"]);
    feature_journal_is_safe(&fixture.cfg.database).await;
    assert_eq!(
        cookie_refresh_attempts(fixture.root.path()),
        2,
        "last actionable error must survive the permitted auth retry, not trigger another download"
    );
}

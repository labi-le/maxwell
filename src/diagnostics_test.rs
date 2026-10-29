use crate::feature_support::*;
use crate::test_support::*;
use crate::*;

#[test]
fn stderr_categories_are_specific_and_never_copy_secrets() {
    for (line, expected) in [
        (
            "ERROR: [youtube] abc: Requested format is not available.",
            "Format unavailable",
        ),
        (
            "ERROR: [youtube] abc: This video is available to this channel's members.",
            "Members only",
        ),
        (
            "ERROR: [youtube] abc: The uploader has not made this video available in your country.",
            "Region blocked",
        ),
        (
            "ERROR: [youtube] abc: Unable to download video data: Connection reset by peer",
            "Network error",
        ),
        (
            "ERROR: [youtube] abc: Postprocessing: Conversion failed!",
            "Processing failed",
        ),
        (
            "ERROR: [youtube] abc: No space left on device",
            "Temporary storage limit",
        ),
        (
            "ERROR: [youtube] abc: does not pass filter (!is_live & duration < 1800), skipping",
            "Video filtered",
        ),
    ] {
        assert_eq!(stderr_detail(line), expected, "{line}");
    }
    let secret = "ERROR: [youtube] Cookie: secret BOT_TOKEN=synthetic-secret token=secret";
    assert_eq!(stderr_detail(secret), "Download failed");
    assert!(!stderr_detail(secret).contains("secret"));
}

#[cfg(unix)]
#[tokio::test]
async fn failed_tiktok_download_persists_sanitized_http_diagnostic_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(
        root.path(),
        "#!/bin/sh\nprintf '%s\\n' 'ERROR: [TikTok] HTTP Error 403' >&2\nexit 9\n",
    )
    .await;
    let database_url = format!("sqlite://{}", root.path().join("state.db").display());
    let (bot, calls, server) = recording_bot(true, true).await;
    let message = serde_json::from_value::<Message>(serde_json::json!({
        "message_id": 73, "date": 1700000000,
        "chat": {"id": 1, "type": "private"},
        "from": {"id": 1, "is_bot": false, "first_name": "Owner"},
        "text": "https://www.tiktok.com/@name/video/111?token=do-not-store"
    }))
    .unwrap();
    time::timeout(
        Duration::from_secs(5),
        handle_message(bot, message, Arc::new(settings)),
    )
    .await
    .expect("fake yt-dlp request stalled")
    .unwrap();
    assert!(calls
        .lock()
        .await
        .iter()
        .any(|(path, _)| api_method_is(path, "sendMessage")));
    server.abort();

    // Read from a fresh pool, not the request's in-memory state.
    let database = open_database(&database_url).await.unwrap();
    let rows: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT CAST(created_at AS TEXT), url_host, url_path, stage, detail, exit_code FROM download_failures"
        ).fetch_all(&database).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "one failure should have one persistent diagnostic"
    );
    let (created_at, host, path, stage, detail, exit_code) = &rows[0];
    assert!(!created_at.is_empty());
    assert_eq!(host, "www.tiktok.com");
    assert_eq!(path, "/@name/video/111");
    assert_eq!(stage, "yt-dlp");
    assert_eq!(detail, "HTTP 403");
    assert_eq!(*exit_code, 9);
    for field in [created_at, host, path, stage, detail] {
        assert!(
            !field.contains("do-not-store") && !field.contains("token="),
            "persisted diagnostics must not include request secrets"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn failure_trace_persists_ordered_auth_refresh_ack_and_second_failure() {
    let root = tempfile::tempdir().unwrap();
    let (mut cfg, source) = cookie_refresh_fixture(root.path(), true).await;
    let database_url = format!("sqlite://{}", root.path().join("state.db").display());
    let (url, calls, _sources, server) = fake_cookie_service(source, 200).await;
    cfg.cookie_refresh_url = Some(url);
    let (bot, _, server_bot) = recording_bot(true, true).await;
    let message = serde_json::from_value::<Message>(serde_json::json!({
        "message_id": 74, "date": 1700000000,
        "chat": {"id": 1, "type": "private"},
        "from": {"id": 1, "is_bot": false, "first_name": "Owner"},
        "text": "https://youtu.be/abc"
    }))
    .unwrap();
    time::timeout(
        Duration::from_secs(5),
        handle_message(bot, message, Arc::new(cfg)),
    )
    .await
    .expect("authentication retry stalled")
    .unwrap();
    server.abort();
    server_bot.abort();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(cookie_refresh_attempts(root.path()), 2);

    let database = open_database(&database_url).await.unwrap();
    let parents: Vec<(i64, String, String, Option<i64>)> =
        sqlx::query_as("SELECT rowid, stage, detail, exit_code FROM download_failures")
            .fetch_all(&database)
            .await
            .unwrap();
    assert_eq!(
        parents.len(),
        1,
        "both attempts belong to one failure record"
    );
    let (failure_rowid, stage, detail, exit_code) = &parents[0];
    assert_eq!(
        (stage.as_str(), detail.as_str(), *exit_code),
        ("yt-dlp", "Authentication required", Some(1)),
        "the parent diagnostic must retain the first failure"
    );
    let table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'download_failure_events'"
        ).fetch_one(&database).await.unwrap();
    assert_eq!(table_count, 1, "failure events must be persisted in SQLite");
    let events: Vec<(i64, String, String, Option<i64>)> = sqlx::query_as(
            "SELECT event_index, stage, detail, exit_code FROM download_failure_events WHERE failure_rowid = ? ORDER BY event_index"
        ).bind(failure_rowid).fetch_all(&database).await.unwrap();
    assert!(
        (5..=12).contains(&events.len()),
        "missing lifecycle event or unbounded trace: {events:?}"
    );
    assert!(
        events.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "event order must be stable"
    );
    assert_eq!(
        (&events[0].1[..], &events[0].2[..], events[0].3),
        ("yt-dlp", "Authentication required", Some(1))
    );
    assert_eq!(
        (
            &events.last().unwrap().1[..],
            &events.last().unwrap().2[..],
            events.last().unwrap().3
        ),
        ("yt-dlp", "Authentication required", Some(9)),
        "the retry's exit code must not be replaced by the first attempt's"
    );
    let labels: Vec<String> = events
        .iter()
        .map(|(_, stage, detail, _)| format!("{stage} {detail}").to_ascii_lowercase())
        .collect();
    let request = labels
        .iter()
        .position(|label| label.contains("refresh") && label.contains("request"))
        .expect("refresh request event is missing");
    let ack = labels
        .iter()
        .position(|label| label.contains("ack"))
        .expect("refresh ACK event is missing");
    let retry = labels
        .iter()
        .position(|label| label.contains("retry"))
        .expect("retry attempt event is missing");
    assert!(
        0 < request && request < ack && ack < retry && retry < events.len() - 1,
        "refresh request, ACK, retry and failures must occur in that order: {events:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn failure_trace_retention_evicts_old_children_and_never_persists_stderr_secrets() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(
        root.path(),
        r#"#!/bin/sh
printf '%s\n' 'ERROR: [youtube] HTTP Error 403' 'Cookie: secret' \
  'https://youtube.com/watch?v=x&token=secret' 'BOT_TOKEN=synthetic-secret' >&2
exit 9
"#,
    )
    .await;
    let database_url = format!("sqlite://{}", root.path().join("state.db").display());
    let (bot, _, server) = recording_bot(true, true).await;
    let message = serde_json::from_value::<Message>(serde_json::json!({
        "message_id": 75, "date": 1700000000,
        "chat": {"id": 1, "type": "private"},
        "from": {"id": 1, "is_bot": false, "first_name": "Owner"},
        "text": "https://youtu.be/abc"
    }))
    .unwrap();
    time::timeout(
        Duration::from_secs(5),
        handle_message(bot, message, Arc::new(settings)),
    )
    .await
    .expect("synthetic stderr download stalled")
    .unwrap();
    server.abort();
    let database = open_database(&database_url).await.unwrap();
    let table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'download_failure_events'"
        ).fetch_one(&database).await.unwrap();
    assert_eq!(table_count, 1, "failure trace table must be initialized");
    let oldest: i64 = sqlx::query_scalar("SELECT rowid FROM download_failures")
        .fetch_one(&database)
        .await
        .unwrap();
    let first_events: Vec<(String, String, Option<i64>)> = sqlx::query_as(
            "SELECT stage, detail, exit_code FROM download_failure_events WHERE failure_rowid = ? ORDER BY event_index"
        ).bind(oldest).fetch_all(&database).await.unwrap();
    assert!(
        !first_events.is_empty(),
        "generic downloader failures need a final trace event"
    );
    assert_eq!(
        first_events.last().unwrap(),
        &("yt-dlp".to_string(), "HTTP 403".to_string(), Some(9))
    );
    for (stage, detail, _) in &first_events {
        for secret in [
            "Cookie: secret",
            "token=secret",
            "BOT_TOKEN=synthetic-secret",
        ] {
            assert!(
                !stage.contains(secret) && !detail.contains(secret),
                "persisted failure events must never contain raw stderr secrets"
            );
        }
    }

    let url = allowed_url("https://youtu.be/abc").unwrap();
    for _ in 0..201 {
        record_failure(
            &database,
            &url,
            FailureDiagnostic {
                stage: "yt-dlp",
                detail: "HTTP 403",
                exit_code: Some(9),
            },
        )
        .await;
    }
    let parents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download_failures")
        .fetch_one(&database)
        .await
        .unwrap();
    assert_eq!(
        parents, 200,
        "only the 200 most recent parent failures may remain"
    );
    let old_children: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM download_failure_events WHERE failure_rowid = ?")
            .bind(oldest)
            .fetch_one(&database)
            .await
            .unwrap();
    assert_eq!(
        old_children, 0,
        "evicted failures must not retain child events"
    );
    let orphaned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM download_failure_events AS e LEFT JOIN download_failures AS f ON f.rowid = e.failure_rowid WHERE f.rowid IS NULL"
        ).fetch_one(&database).await.unwrap();
    assert_eq!(orphaned, 0, "event retention must track parent retention");
    let missing_or_unbounded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM download_failures AS f WHERE (SELECT COUNT(*) FROM download_failure_events AS e WHERE e.failure_rowid = f.rowid) NOT BETWEEN 1 AND 12"
        ).fetch_one(&database).await.unwrap();
    assert_eq!(
        missing_or_unbounded, 0,
        "each retained generic failure needs a bounded final event"
    );
}

#[tokio::test]
async fn features_actual_source_failures_explain_distinct_causes_without_leaking_diagnostics() {
    let mut reasons = Vec::new();
    for (stderr, keywords) in [
        ("HTTP Error 403", &["403", "отказ", "запрет"][..]),
        ("HTTP Error 404", &["404", "не найден", "удал"][..]),
        (
            "This video is private",
            &["закрыт", "приват", "private"][..],
        ),
        (
            "cookies have expired",
            &["cookies", "cookie", "авторизац", "вход"][..],
        ),
        ("HTTP Error 429", &["429", "част", "лимит", "подожд"][..]),
        (
            "HTTP Error 503",
            &["503", "временн", "недоступ", "серв"][..],
        ),
        (
            "Unsupported URL",
            &["не поддерж", "неподдерж", "unsupported"][..],
        ),
        (
            "unrecognised synthetic downloader error",
            &["неизвест", "unknown", "причин"][..],
        ),
    ] {
        let tail = format!("printf '%s\\n' 'ERROR: {stderr} https://evil.invalid/?token={SECRET} Cookie: {SECRET} BOT_TOKEN={SECRET}' >&2\nexit 9");
        let fixture = FeatureFixture::new(&[], &tail).await;
        fixture
            .send(1, 1, "Owner", &format!("{LINK}&token={SECRET}"))
            .await;
        let requests = fixture.api.snapshot().await;
        feature_retained(&requests);
        assert!(!requests.iter().any(|request| request.is_media()));
        let reason = feature_reason(&requests, keywords);
        assert!(
            !reasons.contains(&reason),
            "distinct public failure causes must not collapse into check-link"
        );
        reasons.push(reason);
        feature_journal_is_safe(&fixture.cfg.database).await;
    }
}

#[tokio::test]
async fn features_source_size_limit_is_not_misreported_as_unsupported_or_missing_media() {
    let mut fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
    let mut cfg = (*fixture.cfg).clone();
    cfg.max_media_bytes = 5; // The completed supported file is eight bytes, not unsupported.
    fixture.cfg = Arc::new(cfg);
    fixture.send(1, 1, "Owner", LINK).await;
    let requests = fixture.api.snapshot().await;
    feature_retained(&requests);
    assert!(!requests.iter().any(|request| request.is_media()));
    let reason = feature_reason(&requests, &["размер", "слишком больш", "size"]);
    assert!(
        !reason.contains("неподдерж") && !reason.contains("не найдено медиа"),
        "known size-skipped media needs an actionable size reason, not a guessed extractor cause"
    );
    feature_journal_is_safe(&fixture.cfg.database).await;
}

#[tokio::test]
async fn features_empty_successful_source_reports_missing_media_not_an_invented_access_failure() {
    let fixture = FeatureFixture::new(&[], "exit 0").await;
    fixture.send(1, 1, "Owner", LINK).await;
    let requests = fixture.api.snapshot().await;
    feature_retained(&requests);
    assert!(!requests.iter().any(|request| request.is_media()));
    let reason = feature_reason(
        &requests,
        &["не найден", "нет медиа", "медиа не", "no media"],
    );
    assert!(!reason.contains("403") && !reason.contains("cookies") && !reason.contains("приват"));
}

#[tokio::test]
async fn features_source_access_restrictions_are_explained_without_leaking_details() {
    for (url, error) in [
        (
            "https://www.tiktok.com/@name/video/111",
            "Your IP address is blocked from accessing this post",
        ),
        (
            "https://www.instagram.com/reel/restricted/",
            "This content isn't available to everyone: It can't be seen by certain audiences.",
        ),
    ] {
        let fixture = FeatureFixture::new(&[], "exit 0").await;
        fs::write(
            &fixture.cfg.ytdlp_bin,
            format!("#!/bin/sh\nprintf '%s\\n' \"ERROR: {error} Cookie: {SECRET}\" >&2\nexit 1\n"),
        )
        .unwrap();
        fixture.send(1, 1, "Owner", url).await;
        let requests = fixture.api.snapshot().await;
        assert!(!requests.iter().any(|request| request.is_media()));
        feature_retained(&requests);
        feature_reason(&requests, &["ограничил"]);
        feature_journal_cause(&fixture.cfg.database, &["restricted"]).await;
    }
}

#[tokio::test]
async fn features_latest_actionable_source_error_after_format_fallback_reaches_the_user() {
    let fixture = FeatureFixture::new(&[], "exit 0").await;
    let script = format!(
        r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
    printf '%s\n' '{{"live_status":"not_live","duration":1,"formats":[{{"format_id":"18","ext":"mp4","vcodec":"avc1.42001e","acodec":"mp4a.40.2","width":32,"height":32,"filesize":100000}}]}}'
    exit 0
  fi
done
selected=no
while [ "$#" -gt 0 ]; do
  case "$1" in -f) shift; if [ "$1" = 18 ]; then selected=yes; fi ;; esac
  shift
done
printf 'attempt\n' >> '{}/attempts'
if [ "$selected" = yes ]; then
  printf '%s\n' 'ERROR: HTTP Error 403' >&2
else
  printf '%s\n' 'ERROR: HTTP Error 404' >&2
fi
exit 9
"#,
        fixture.root.path().display()
    );
    fs::write(&fixture.cfg.ytdlp_bin, script).unwrap();
    fixture.send(1, 1, "Owner", "https://youtu.be/abc").await;
    let requests = fixture.api.snapshot().await;
    feature_retained(&requests);
    assert!(!requests.iter().any(|request| request.is_media()));
    feature_reason(&requests, &["404", "не найден", "удал"]);
    feature_journal_cause(&fixture.cfg.database, &["404"]).await;
    assert_eq!(
        cookie_refresh_attempts(fixture.root.path()),
        2,
        "the source format fallback should happen once and preserve its newer failure cause"
    );
}

#[tokio::test]
async fn features_metadata_source_errors_explain_safe_http_causes_for_pinterest_and_instagram() {
    for (url, code, keywords) in [
        (
            "https://www.pinterest.com/pin/123456789/",
            "403",
            &["403", "отказ", "запрет"][..],
        ),
        (
            "https://www.instagram.com/p/feature-post/",
            "404",
            &["404", "не найден", "удал"][..],
        ),
    ] {
        let fixture = FeatureFixture::new(&[], "exit 0").await;
        let script = format!(
            r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
    printf '%s\n' 'ERROR: HTTP Error {code} https://evil.invalid/?token={SECRET} Cookie: {SECRET} BOT_TOKEN={SECRET}' >&2
    exit 9
  fi
done
exit 19
"#
        );
        fs::write(&fixture.cfg.ytdlp_bin, script).unwrap();
        fixture
            .send(1, 1, "Owner", &format!("{url}?token={SECRET}"))
            .await;
        let requests = fixture.api.snapshot().await;
        assert!(
            !requests.iter().any(|request| request.is_media()),
            "failed source metadata must not invent deliverable media"
        );
        feature_retained(&requests);
        feature_reason(&requests, keywords);
        feature_journal_is_safe(&fixture.cfg.database).await;
        feature_journal_cause(&fixture.cfg.database, &[code]).await;
    }
}

#[tokio::test]
async fn features_fresh_single_ambiguous_errors_never_document_fallback_or_cache() {
    for (extension, method) in [("jpg", "sendphoto"), ("mp3", "sendaudio")] {
        for (reply, keywords) in [
            (
                FeatureReply::Disconnect,
                &["сет", "соедин", "связ", "network", "тайм", "врем"][..],
            ),
            (
                FeatureReply::InvalidResponse,
                &["некоррект", "неверн", "ответ", "invalid response"][..],
            ),
            (
                FeatureReply::rate_limit(),
                &["17", "429", "част", "лимит", "подожд"][..],
            ),
        ] {
            let fixture = FeatureFixture::new(&[extension], "exit 0").await;
            fixture.api.reject_next(reply).await;
            fixture.send(1, 1, "Owner", LINK).await;
            let requests = fixture.api.snapshot().await;
            let media: Vec<_> = requests
                .iter()
                .filter(|request| request.is_media())
                .collect();
            assert_eq!(
                media.len(),
                1,
                "fresh single media cannot be resent after ambiguous delivery or rate limiting"
            );
            assert_eq!(media[0].method, method);
            assert!(
                !requests
                    .iter()
                    .any(|request| request.method == "senddocument"),
                "document fallback is unsafe without a definitive media rejection"
            );
            feature_retained(&requests);
            feature_reason(&requests, keywords);
            feature_journal_is_safe(&fixture.cfg.database).await;
            let calls = fixture.calls();
            fixture.api.clear().await;
            fixture.send(1, 1, "Next request", LINK).await;
            assert_eq!(
                fixture.calls(),
                calls + 1,
                "unsuccessful or ambiguous fresh delivery must not create a reusable cache entry"
            );
        }
    }
}

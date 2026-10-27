use crate::prelude::*;
use crate::*;
use crate::test_support::*;
use crate::feature_support::*;


    #[tokio::test]
    async fn features_cached_album_uses_ids_and_latest_sender_after_database_reopen() {
        let mut fixture = FeatureFixture::new(&["jpg", "mp4"], "exit 0").await;
        fixture.send(1, 1, "Original", LINK).await;
        assert_eq!(fixture.calls(), 1);
        fixture.api.clear().await;
        fixture.send(1, 1, "Current & <Name>", LINK).await;
        assert_eq!(fixture.calls(), 1, "same-chat exact-link cache hit must not invoke yt-dlp");
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].method, "sendmediagroup");
        assert!(media[0].uploads.is_empty(), "file_id cache hits cannot upload binary data");
        let items = media[0].media();
        assert_eq!(items.iter().map(|item| item["media"].as_str().unwrap()).collect::<Vec<_>>(),
            ["features-photo-1", "features-video-2"]);
        for (index, item) in items.iter().enumerate() {
            let caption = item["caption"].as_str().unwrap();
            assert!(caption.contains(&format!("Post {index} &amp; &lt;source&gt;")));
            assert!(caption.contains("<code>От Current &amp; &lt;Name&gt;</code>"));
            assert!(!caption.contains("Original") && !caption.contains("tg://"));
            assert!(caption.contains("list=features&amp;variant=one"));
            assert_eq!(item["parse_mode"], "HTML");
        }
        assert_eq!(items[1]["supports_streaming"], true);
        assert_eq!(requests.last().unwrap().method, "deletemessage");
        feature_silent(&requests);
        let mut cfg = (*fixture.cfg).clone();
        cfg.database.close().await;
        cfg.database = open_database(&format!("sqlite://{}", fixture.root.path().join("state.db").display()))
            .await.unwrap();
        fixture.cfg = Arc::new(cfg);
        fixture.api.clear().await;
        fixture.send(1, 1, "Reopened", LINK).await;
        assert_eq!(fixture.calls(), 1, "cached publications must survive a database reopen");
        assert!(fixture.api.snapshot().await.iter().filter(|request| request.is_media())
            .all(|request| request.uploads.is_empty()));
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT cache_key, items_json FROM media_cache")
            .fetch_all(&fixture.cfg.database).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.len(), 64, "cache identity must be a SHA-256 digest, not a URL or token");
        for (key, items) in rows {
            for secret in [LINK, "features-token-one", "Original", "<code>"] {
                assert!(!key.contains(secret) && !items.contains(secret), "cache persisted URL/token/old attribution");
            }
        }
    }

    #[tokio::test]
    async fn features_cache_is_scoped_to_chat_full_query_and_bot_token() {
        let mut fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.warm().await;
        let mut expected = fixture.calls();
        for (chat, url) in [(2, LINK), (1, "https://www.youtube.com/playlist?list=features&variant=two")] {
            fixture.api.clear().await;
            fixture.send(chat, 1, "Owner", url).await;
            expected += 1;
            assert_eq!(fixture.calls(), expected, "different chat/query must download a separate publication");
            assert!(fixture.api.snapshot().await.iter().filter(|request| request.is_media())
                .all(|request| !request.uploads.is_empty()));
        }
        fixture.api.bot = Bot::new("456:features-token-two").set_api_url(fixture.api.url.clone());
        fixture.api.clear().await;
        fixture.send(1, 1, "Owner", LINK).await;
        assert_eq!(fixture.calls(), expected + 1, "a different bot token cannot reuse another bot's file_ids");
        assert!(fixture.api.snapshot().await.iter().filter(|request| request.is_media())
            .all(|request| !request.uploads.is_empty()));
    }

    #[tokio::test]
    async fn features_cache_hit_does_not_renew_twenty_four_hour_expiry() {
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.warm().await; // Baseline must fail here, not at the unimplemented SQL table below.
        sqlx::query("UPDATE media_cache SET created_at = unixepoch() - 86300")
            .execute(&fixture.cfg.database).await.unwrap();
        let stored: i64 = sqlx::query_scalar("SELECT created_at FROM media_cache")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        let calls = fixture.calls();
        fixture.api.clear().await;
        fixture.send(1, 1, "Near expiry", LINK).await;
        assert_eq!(fixture.calls(), calls, "cache must remain usable before 24 hours");
        let after_hit: i64 = sqlx::query_scalar("SELECT created_at FROM media_cache")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        assert_eq!(after_hit, stored, "hits cannot extend the source download's TTL");
        sqlx::query("UPDATE media_cache SET created_at = unixepoch() - 86401")
            .execute(&fixture.cfg.database).await.unwrap();
        fixture.api.clear().await;
        fixture.send(1, 1, "Expired", LINK).await;
        assert_eq!(fixture.calls(), calls + 1, "expired cache must return to the source");
        assert!(fixture.api.snapshot().await.iter().filter(|request| request.is_media())
            .all(|request| !request.uploads.is_empty()));
    }

    #[tokio::test]
    async fn features_definitively_invalid_cached_id_downloads_once_before_any_delivery() {
        for (extensions, method) in [(vec!["mp4"], "sendvideo"), (vec!["jpg", "mp4"], "sendmediagroup")] {
            let fixture = FeatureFixture::new(&extensions, "exit 0").await;
            fixture.warm().await;
            let calls = fixture.calls();
            fixture.api.clear().await;
            fixture.api.reject_next(FeatureReply::reject("Bad Request: wrong file identifier/HTTP URL specified")).await;
            fixture.send(1, 1, "Recovered", LINK).await;
            let requests = fixture.api.snapshot().await;
            let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
            assert_eq!(fixture.calls(), calls + 1, "zero-delivery definitive invalid ID permits one fresh download");
            assert_eq!(media.len(), 2);
            assert!(media[0].uploads.is_empty(), "first send must attempt the cached ID");
            assert!(!media[1].uploads.is_empty(), "recovery must deliver fresh source bytes");
            assert!(media.iter().all(|request| request.method == method),
                "invalid cached album must recover as an album, not duplicate-prone single sends");
            assert_eq!(requests.last().unwrap().method, "deletemessage");
            assert!(!requests.iter().any(|request| request.method == "sendmessage"));
            fixture.api.clear().await;
            fixture.send(1, 1, "New cache", LINK).await;
            assert_eq!(fixture.calls(), calls + 1, "replacement IDs should populate a new complete cache entry");
            assert!(fixture.api.snapshot().await.iter().filter(|request| request.is_media())
                .all(|request| request.uploads.is_empty()));
        }
    }

    #[tokio::test]
    async fn features_invalid_cached_id_after_partial_delivery_never_restarts_or_changes_type() {
        let fixture = FeatureFixture::new(&["mp4", "mp3"], "exit 0").await;
        fixture.warm().await;
        let calls = fixture.calls();
        fixture.api.clear().await;
        // Queue success for the first item by using a missing-ID response, which still delivered it.
        fixture.api.reject_next(FeatureReply::MissingIds).await;
        fixture.api.reject_next(FeatureReply::reject("Bad Request: wrong file identifier/HTTP URL specified")).await;
        fixture.send(1, 1, "Partial", LINK).await;
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(fixture.calls(), calls, "a delivered cached item forbids fresh-download restart");
        assert_eq!(media.iter().map(|request| request.method.as_str()).collect::<Vec<_>>(),
            ["sendvideo", "sendaudio"], "cached audio IDs cannot fall back to a document ID");
        assert!(media.iter().all(|request| request.uploads.is_empty()));
        feature_retained(&requests);
        feature_reason(&requests, &["file_id", "идентификатор", "файл", "id"]);
        fixture.api.clear().await;
        fixture.send(1, 1, "Next attempt", LINK).await;
        assert_eq!(fixture.calls(), calls + 1, "known invalid cache must be invalidated for a later request");
    }

    #[tokio::test]
    async fn features_cached_rate_limit_unknown_rejection_and_network_error_never_restart() {
        for (reply, keywords) in [
            (FeatureReply::rate_limit(), &["17", "429", "част", "лимит", "подожд"][..]),
            (FeatureReply::reject(&format!("Bad Request: unknown failure https://evil.invalid/?token={SECRET} Cookie: {SECRET} BOT_TOKEN={SECRET}")),
                &["неизвест", "unknown", "отклонил", "отклон", "причин"][..]),
            (FeatureReply::Disconnect, &["сет", "соедин", "связ", "network", "тайм", "врем"][..]),
        ] {
            let fixture = FeatureFixture::new(&["jpg"], "exit 0").await;
            fixture.warm().await;
            let calls = fixture.calls();
            fixture.api.clear().await;
            fixture.api.reject_next(reply).await;
            fixture.send(1, 1, "Failing hit", LINK).await;
            let requests = fixture.api.snapshot().await;
            let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
            assert_eq!(fixture.calls(), calls, "ambiguous cache errors cannot trigger a fresh source attempt");
            assert_eq!(media.len(), 1, "no cached resend or photo-to-document fallback is safe");
            assert_eq!(media[0].method, "sendphoto");
            assert!(media[0].uploads.is_empty());
            feature_retained(&requests);
            feature_reason(&requests, keywords);
            feature_journal_is_safe(&fixture.cfg.database).await;
        }
    }

    #[tokio::test]
    async fn features_malformed_or_oversized_cache_entries_are_safe_source_misses() {
        let invalid_entries = [
            "{".to_string(),
            "[]".to_string(),
            serde_json::json!([{"kind": "unknown", "file_id": "id", "description": null}]).to_string(),
            serde_json::json!([{"kind": "video", "file_id": "", "description": null}]).to_string(),
            serde_json::json!([{"kind": "video", "file_id": 4, "description": null}]).to_string(),
            serde_json::json!([{"kind": "video", "file_id": "id", "description": "🐈".repeat(513)}]).to_string(),
            serde_json::json!(vec![serde_json::json!({"kind": "video", "file_id": "id",
                "description": null}); MAX_FILES + 1]).to_string(),
        ];
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.warm().await; // Establish the missing behavior before any SQL mutation on baseline.
        for invalid in invalid_entries {
            sqlx::query("UPDATE media_cache SET items_json = ?").bind(&invalid)
                .execute(&fixture.cfg.database).await.unwrap();
            let before = fixture.calls();
            fixture.api.clear().await;
            fixture.send(1, 1, "Defensive miss", LINK).await;
            assert_eq!(fixture.calls(), before + 1, "invalid cache shape must not reach Telegram: {invalid}");
            let requests = fixture.api.snapshot().await;
            let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
            assert_eq!(media.len(), 1);
            assert!(!media[0].uploads.is_empty());
            assert_eq!(requests.last().unwrap().method, "deletemessage");
        }
        sqlx::query("UPDATE media_cache SET items_json = '{'").execute(&fixture.cfg.database).await.unwrap();
        fs::write(&fixture.cfg.ytdlp_bin, feature_script(fixture.root.path(), &[],
            "printf '%s\\n' 'ERROR: HTTP Error 404' >&2\nexit 9")).unwrap();
        fixture.api.clear().await;
        fixture.send(1, 1, "Unavailable source", LINK).await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_cache")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        assert_eq!(count, 0, "defensively rejected cache entries must be removed even when the source fails");
        let requests = fixture.api.snapshot().await;
        assert!(!requests.iter().any(|request| request.is_media()));
        feature_retained(&requests);
    }

    #[tokio::test]
    async fn features_missing_telegram_ids_skip_cache_without_failing_successful_delivery() {
        for extensions in [vec!["mp4"], vec!["jpg", "mp4"]] {
            let fixture = FeatureFixture::new(&extensions, "exit 0").await;
            fixture.api.reject_next(FeatureReply::MissingIds).await;
            fixture.send(1, 1, "Owner", LINK).await;
            let requests = fixture.api.snapshot().await;
            assert_eq!(requests.last().unwrap().method, "deletemessage",
                "a successful Message without an expected file ID is still successful delivery");
            assert!(!requests.iter().any(|request| request.method == "sendmessage"));
            let calls = fixture.calls();
            fixture.api.clear().await;
            fixture.send(1, 1, "Owner", LINK).await;
            assert_eq!(fixture.calls(), calls + 1,
                "one missing response ID forbids caching either the full publication or a successful subset");
        }
    }

    #[tokio::test]
    async fn features_delete_failure_does_not_disqualify_successful_media_from_cache() {
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.api.state.lock().await.delete_fails = true;
        fixture.send(1, 1, "First", LINK).await;
        let requests = fixture.api.snapshot().await;
        assert!(requests.iter().any(|request| request.method == "deletemessage"));
        assert!(requests.iter().any(|request| request.method == "sendmessage"));
        feature_silent(&requests);
        let calls = fixture.calls();
        fixture.api.clear().await;
        fixture.send(1, 1, "Second", LINK).await;
        assert_eq!(fixture.calls(), calls, "delete permission failures must not force another media download");
        let requests = fixture.api.snapshot().await;
        assert!(requests.iter().filter(|request| request.is_media()).all(|request| request.uploads.is_empty()));
    }

    #[tokio::test]
    async fn features_cache_read_and_write_database_errors_do_not_fail_fresh_delivery() {
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.warm().await;
        sqlx::query("DROP TABLE media_cache").execute(&fixture.cfg.database).await.unwrap();
        let calls = fixture.calls();
        fixture.api.clear().await;
        fixture.send(1, 1, "Database unavailable", LINK).await;
        assert_eq!(fixture.calls(), calls + 1, "failed cache read must safely return to the source");
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(media.len(), 1);
        assert!(!media[0].uploads.is_empty());
        assert_eq!(requests.last().unwrap().method, "deletemessage",
            "failed cache write must not convert successful media delivery into failure");
        assert!(!requests.iter().any(|request| request.method == "sendmessage"));
    }

    #[tokio::test]
    async fn features_cache_retention_bounds_publications_globally_and_keeps_the_newest() {
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.warm().await;
        let items: String = sqlx::query_scalar("SELECT items_json FROM media_cache")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        let mut transaction = fixture.cfg.database.begin().await.unwrap();
        for index in 0..205i64 {
            sqlx::query("INSERT INTO media_cache (chat_id, cache_key, created_at, items_json)
                VALUES (?, ?, unixepoch() - 1000 + ?, ?)")
                .bind(1000 + index).bind(format!("{index:064x}")).bind(index).bind(&items)
                .execute(&mut *transaction).await.unwrap();
        }
        transaction.commit().await.unwrap();
        fixture.send(1, 1, "Newest", &format!("{LINK}&publication=newest")).await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_cache")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        assert_eq!(count, 200, "bounded cache retention applies across chats, not per chat");
        let oldest: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_cache WHERE chat_id = 1000")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        let newest: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_cache WHERE chat_id = 1204")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        assert_eq!(oldest, 0);
        assert_eq!(newest, 1, "oldest publications should be evicted before newer publications");
        let calls = fixture.calls();
        fixture.api.clear().await;
        fixture.send(1, 1, "Still warm", &format!("{LINK}&publication=newest")).await;
        assert_eq!(fixture.calls(), calls, "the newly published entry must survive retention");
    }

    #[tokio::test]
    async fn features_cached_descriptions_are_bounded_raw_text_and_render_with_current_attribution() {
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        let description = "🐈 & <source> ".repeat(200);
        let script = feature_script(fixture.root.path(), &["mp4"], "exit 0").replace(
            &serde_json::to_string("Post 0 & <source>").unwrap(),
            &serde_json::to_string(&description).unwrap(),
        );
        fs::write(&fixture.cfg.ytdlp_bin, script).unwrap();
        fixture.warm().await;
        let raw: String = sqlx::query_scalar("SELECT items_json FROM media_cache")
            .fetch_one(&fixture.cfg.database).await.unwrap();
        let items: Value = serde_json::from_str(&raw).unwrap();
        let stored = items[0]["description"].as_str().unwrap();
        assert!(stored.encode_utf16().count() <= 1024, "cache must bound original description size");
        assert!(description.starts_with(stored) && stored.starts_with("🐈 & <source> "),
            "cached description must be an original raw-text prefix, not an old rendered caption");
        assert!(!stored.contains("<code>") && !stored.contains("&amp;"));
        let calls = fixture.calls();
        fixture.api.clear().await;
        fixture.send(1, 1, "Latest & <Sender>", LINK).await;
        assert_eq!(fixture.calls(), calls);
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(media.len(), 1);
        let item = media[0].media().remove(0);
        let caption = item["caption"].as_str().unwrap();
        assert!(caption.starts_with("🐈 &amp; &lt;source&gt; "));
        assert!(caption.contains("<code>От Latest &amp; &lt;Sender&gt;</code>"));
        assert!(caption.contains("list=features&amp;variant=one"));
        assert!(caption.encode_utf16().count() <= 1024);
        assert!(media[0].uploads.is_empty());
    }

    #[tokio::test]
    async fn features_cache_initialization_failures_are_not_silently_ignored() {
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.warm().await;
        sqlx::query("DROP TABLE media_cache").execute(&fixture.cfg.database).await.unwrap();
        fixture.cfg.database.close().await;
        let read_only = format!("sqlite://{}?mode=ro", fixture.root.path().join("state.db").display());
        assert!(open_database(&read_only).await.is_err(),
            "a read-only database missing cache schema must fail initialization, not start in a broken state");
    }

    #[tokio::test]
    async fn features_unverified_legacy_video_cache_requires_fresh_download() {
        use sha2::Digest;
        let fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        let legacy = format!("{:x}", sha2::Sha256::digest(
            format!("maksut/media-cache/v1\0{}\0{LINK}", fixture.api.bot.token()).as_bytes()));
        sqlx::query("INSERT INTO media_cache (chat_id, cache_key, created_at, items_json)
            VALUES (1, ?, unixepoch(), ?)")
            .bind(legacy).bind(r#"[{"kind":"video","file_id":"legacy-silent-video","description":null}]"#)
            .execute(&fixture.cfg.database).await.unwrap();
        fixture.send(1, 1, "Owner", LINK).await;
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(fixture.calls(), 1, "old unverified file_id must not bypass the new audio gate");
        assert_eq!(media[0].upload(&media[0].media()[0]).unwrap(), fs::read(av_fixture()).unwrap());
        assert_eq!(requests.last().unwrap().method, "deletemessage");
        fixture.api.clear().await;
        fixture.send(1, 1, "Owner", LINK).await;
        assert_eq!(fixture.calls(), 1, "verified media remains cacheable");
        assert!(fixture.api.snapshot().await.iter().filter(|request| request.is_media())
            .all(|request| request.uploads.is_empty()));
    }

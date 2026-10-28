use crate::prelude::*;
use crate::*;
use crate::test_support::*;
use crate::feature_support::*;


    #[cfg(unix)]
    #[tokio::test]
    async fn video_delivery_deletes_only_after_success_and_never_falls_back_to_document() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("real.webm");
        if std::process::Command::new("ffmpeg").args(["-v", "error", "-f", "lavfi", "-i",
            "testsrc2=size=32x32:rate=1", "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
            "-t", "1", "-c:v", "libvpx", "-c:a", "libopus", "-y"])
            .arg(&fixture).status().is_ok_and(|status| status.success())
        {
            assert!(fixture.exists());
        }
        let oversized = root.path().join("oversized.mkv");
        if fixture.exists() {
            assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-f", "lavfi", "-i",
                "testsrc2=size=4000x100:rate=1", "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
                "-t", "1", "-c:v", "mpeg4", "-c:a", "aac", "-y"])
                .arg(&oversized).status().unwrap().success());
        }
        for (extension, contents, video_ok, delete_ok, expect_video, expect_delete) in [
            ("mp4", "video", true, true, true, true),
            ("mp4", "video", false, true, true, false),
            ("webm", "invalid webm", true, true, false, false),
            ("webm", "real webm", true, true, true, true),
            ("mp4", "unsupported pair", true, true, true, false),
            ("mkv", "oversized mkv", true, true, false, false),
            ("mp4", "partial", true, true, true, false),
            ("mp4", "video", true, false, true, true),
        ] {
            if matches!(contents, "real webm" | "oversized mkv") && !fixture.exists() { continue; }
            let scenario = tempfile::tempdir_in(root.path()).unwrap();
            let script = format!(r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${{1%/*}} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
{create_file}
printf '%s\n' "$dir/finished.{extension}" > "$manifest"
{manifest_extra}
{exit}
"#, create_file = if contents == "real webm" {
                format!("cp '{}' \"$dir/finished.webm\"", fixture.display())
            } else if contents == "oversized mkv" {
                format!("cp '{}' \"$dir/finished.mkv\"", oversized.display())
            } else if contents == "unsupported pair" {
                format!("cp '{}' \"$dir/finished.mp4\"\nprintf unsupported > \"$dir/finished.flv\"", av_fixture().display())
            } else if extension == "mp4" {
                format!("cp '{}' \"$dir/finished.mp4\"", av_fixture().display())
            } else {
                format!("printf '%s' '{contents}' > \"$dir/finished.{extension}\"")
            }, manifest_extra = if contents == "unsupported pair" {
                "printf '%s\\n' \"$dir/finished.flv\" >> \"$manifest\""
            } else { "" }, exit = if contents == "partial" { "exit 9" } else { "exit 0" });
            let mut cfg = fake_settings(scenario.path(), &script).await;
            cfg.max_media_bytes = 1_000_000;
            if !delete_ok {
                set_group_enabled(&cfg.database, ChatId(-100123), true).await.unwrap();
            }
            let (bot, records, server) = recording_bot(video_ok, delete_ok).await;
            let message = serde_json::from_value::<Message>(serde_json::json!({
                "message_id": 73, "date": 1700000000,
                "chat": {"id": if delete_ok { 1 } else { -100123 },
                    "type": if delete_ok { "private" } else { "group" }},
                "from": {"id": 1, "is_bot": false, "first_name": "Owner"},
                "text": "https://youtu.be/abc?a=1&b=2"
            })).unwrap();
            time::timeout(Duration::from_secs(15), handle_message(bot, message, Arc::new(cfg)))
                .await.expect("Bot API interaction stalled").unwrap();
            let requests = records.lock().await;
            assert_eq!(requests.iter().any(|(path, _)| api_method_is(path, "sendVideo")), expect_video);
            assert_eq!(requests.iter().any(|(path, _)| api_method_is(path, "deleteMessage")), expect_delete);
            assert!(!requests.iter().any(|(path, _)| api_method_is(path, "sendDocument")));
            if contents == "unsupported pair" {
                assert!(requests.iter().any(|(path, body)| api_method_is(path, "sendMessage")
                    && body.contains("Некоторые файлы пропущены")));
            }
            if contents == "oversized mkv" {
                assert!(requests.iter().any(|(path, body)| api_method_is(path, "sendMessage")
                    && body.contains("Разрешение видео превышает допустимый предел")));
            }
            if expect_video {
                let video = requests.iter().find(|(path, _)| api_method_is(path, "sendVideo")).unwrap();
                assert!(video.1.contains("<code>От Owner</code>"));
                assert!(video.1.contains("https://youtu.be/abc?a=1&amp;b=2"));
                if contents == "real webm" {
                    assert!(video.1.contains(".mp4"), "converted video upload must be MP4");
                }
                assert!(video.1.contains("HTML"));
            }
            if !delete_ok {
                assert!(requests.iter().any(|(path, body)| api_method_is(path, "sendMessage")
                    && body.contains("can_delete_messages")));
            }
            let messages: Vec<_> = requests.iter().filter(|(path, _)| api_method_is(path, "sendMessage")).collect();
            assert_eq!(messages.len(), usize::from(!delete_ok || !expect_delete || !video_ok
                || contents == "unsupported pair" || contents == "partial" || contents == "oversized mkv"),
                "success must be quiet; failures must explain why");
            assert!(!messages.iter().any(|(_, body)| body.contains("Отправлено:") || body.contains("Отправлено файлов")));
            if contents == "partial" {
                assert!(messages[0].1.contains("Загрузка завершилась с ошибкой"));
            }
            drop(requests);
            server.abort();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_instagram_description_precedes_sender_code_and_preserves_query() {
        let root = tempfile::tempdir().unwrap();
        let url = "https://instagram.com/p/DdwhVt0RuIq/?stkn=one&next=two";
        let (calls, probes) = metadata_caption_recorded_delivery(root.path(), url,
            &serde_json::json!({
                "description": "Какой красивый котик & <милый>",
                "formats": [{"url": "https://example.org/real.mp4"}],
                "thumbnails": []
            }), None, None, None,
            serde_json::json!({"id": 1, "is_bot": false, "first_name": "Kitty", "username": "cat_sender"}),
            true).await;
        assert_eq!(probes, 1, "Instagram should fetch post metadata only once");
        assert_eq!(metadata_caption_from_video(&calls),
            "Какой красивый котик &amp; &lt;милый&gt;\n<code>От Kitty</code>\n<a href=\"https://instagram.com/p/DdwhVt0RuIq/?stkn=one&amp;next=two\">Ссылка</a>");
        let video_index = calls.iter().position(|(path, _)| api_method_is(path, "sendVideo")).unwrap();
        let delete_index = calls.iter().position(|(path, _)| api_method_is(path, "deleteMessage"))
            .expect("successful upload deletes source message");
        assert!(video_index < delete_index, "the source cannot be deleted before video delivery");
        assert!(!calls.iter().any(|(path, _)| api_method_is(path, "sendDocument")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_youtube_uses_download_description_when_probe_has_no_metadata() {
        let root = tempfile::tempdir().unwrap();
        let (calls, probes) = metadata_caption_recorded_delivery(root.path(), "https://youtu.be/abc",
            &serde_json::json!({"formats": []}), None, Some("Видео & <кот>"), None,
            serde_json::json!({"id": 1, "is_bot": false, "first_name": "Kitty", "username": "cat_sender"}),
            true).await;
        assert_eq!(probes, 1, "YouTube format probe must not be repeated for the description");
        assert_eq!(metadata_caption_from_video(&calls),
            "Видео &amp; &lt;кот&gt;\n<code>От Kitty</code>\n<a href=\"https://youtu.be/abc\">Ссылка</a>");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_keeps_post_title_before_description() {
        let root = tempfile::tempdir().unwrap();
        let (calls, _) = metadata_caption_recorded_delivery(root.path(), "https://youtu.be/abc",
            &serde_json::json!({"formats": []}), Some("Заголовок & <ролик>"),
            Some("Описание"), None,
            serde_json::json!({"id": 1, "is_bot": false, "first_name": "Kitty"}),
            true).await;
        assert_eq!(metadata_caption_from_video(&calls),
            "Заголовок &amp; &lt;ролик&gt;\n\nОписание\n<code>От Kitty</code>\n<a href=\"https://youtu.be/abc\">Ссылка</a>");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_ignores_synthesized_placeholder_title() {
        let root = tempfile::tempdir().unwrap();
        let (calls, _) = metadata_caption_recorded_delivery(root.path(), "https://youtu.be/abc",
            &serde_json::json!({"formats": []}), Some("Video by someone"),
            Some("Описание"), None,
            serde_json::json!({"id": 1, "is_bot": false, "first_name": "Kitty"}),
            true).await;
        assert_eq!(metadata_caption_from_video(&calls),
            "Описание\n<code>От Kitty</code>\n<a href=\"https://youtu.be/abc\">Ссылка</a>");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_does_not_copy_the_sender_text() {
        let root = tempfile::tempdir().unwrap();
        let (calls, _) = metadata_caption_recorded_delivery(root.path(), "https://youtu.be/abc",
            &serde_json::json!({"formats": []}), Some("Заголовок"), Some("Описание"),
            Some("девочка зажигает\nсмотрите до конца https://youtu.be/abc"),
            serde_json::json!({"id": 1, "is_bot": false, "first_name": "Вадик"}),
            true).await;
        assert_eq!(metadata_caption_from_video(&calls),
            "Заголовок\n\nОписание\n<code>От Вадик</code>\n<a href=\"https://youtu.be/abc\">Ссылка</a>");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_without_description_starts_with_escaped_name_in_code() {
        for description in [None, Some("")] {
            let root = tempfile::tempdir().unwrap();
            let (calls, _) = metadata_caption_recorded_delivery(root.path(),
                "https://instagram.com/p/abc/",
                &serde_json::json!({"formats": [{"url": "https://example.org/video.mp4"}],
                    "description": description}), None, None, None,
                serde_json::json!({"id": 1, "is_bot": false, "first_name": "Alice & <Bob>"}),
                true).await;
            assert_eq!(metadata_caption_from_video(&calls),
                "<code>От Alice &amp; &lt;Bob&gt;</code>\n<a href=\"https://instagram.com/p/abc/\">Ссылка</a>");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_long_html_and_emoji_remains_within_telegram_utf16_limit() {
        let root = tempfile::tempdir().unwrap();
        let description = "🐈 & <милый> ".repeat(180);
        let (calls, _) = metadata_caption_recorded_delivery(root.path(),
            "https://instagram.com/p/abc/",
            &serde_json::json!({"formats": [{"url": "https://example.org/video.mp4"}],
                "description": description}), None, None, None,
            serde_json::json!({"id": 1, "is_bot": false, "first_name": "Kitty", "username": "cat_sender"}),
            true).await;
        let caption = metadata_caption_from_video(&calls);
        let footer = "\n<code>От Kitty</code>\n<a href=\"https://instagram.com/p/abc/\">Ссылка</a>";
        let text = caption.strip_suffix(footer).expect("sender attribution and source link survive truncation");
        assert!(text.starts_with("🐈 &amp; &lt;милый&gt; "), "{text}");
        assert!(caption.encode_utf16().count() <= 1024, "Telegram caption exceeds 1024 UTF-16 units");
        assert_eq!(text.bytes().filter(|byte| *byte == b'&').count(),
            text.matches("&amp;").count() + text.matches("&lt;").count() + text.matches("&gt;").count(),
            "truncation must not split escaped HTML entities");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_failed_video_upload_keeps_source_message() {
        let root = tempfile::tempdir().unwrap();
        let (calls, _) = metadata_caption_recorded_delivery(root.path(),
            "https://instagram.com/p/abc/",
            &serde_json::json!({"formats": [{"url": "https://example.org/video.mp4"}],
                "description": "Описание"}), None, None, None,
            serde_json::json!({"id": 1, "is_bot": false, "first_name": "Kitty", "username": "cat_sender"}),
            false).await;
        assert!(calls.iter().any(|(path, _)| api_method_is(path, "sendVideo")));
        assert!(!calls.iter().any(|(path, _)| api_method_is(path, "deleteMessage")),
            "an unsuccessful upload must leave the original source link");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_caption_large_playlist_keeps_last_video_description_after_64k_stdout() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir_in(root.path()).unwrap();
        let url = allowed_url("https://youtu.be/abc").unwrap();
        let sender = serde_json::from_value::<teloxide::types::User>(serde_json::json!({
            "id": 1, "is_bot": false, "first_name": "Kitty", "username": "cat_sender"
        })).unwrap();
        let descriptions: Vec<_> = (0..MAX_FILES).map(|index| {
            if index == MAX_FILES - 1 {
                "Последний котик".to_string()
            } else {
                format!("Описание {index} {}", "x".repeat(8192))
            }
        }).collect();
        let mut paths = String::new();
        let mut rows = String::new();
        for (index, description) in descriptions.iter().enumerate() {
            let path = work.path().join(format!("finished-{index}.mp4"));
            paths.push_str(&format!("{}\n", path.display()));
            rows.push_str(&format!("{}\t\"\"\t{}\n",
                serde_json::to_string(path.to_str().unwrap()).unwrap(),
                serde_json::to_string(description).unwrap()));
        }
        assert!(rows.len() > 64 * 1024, "fixture must exceed the former stdout cap");
        fs::write(root.path().join("paths"), paths).unwrap();
        fs::write(root.path().join("rows"), rows).unwrap();
        let script = format!(r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
    printf 'probe\n' >> '{0}/probes'
    printf '%s\n' '{{"formats":[]}}'
    exit 0
  fi
done
printf 'download\n' >> '{0}/attempts'
while [ "$#" -gt 0 ]; do
  case "$1" in
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
while IFS= read -r path; do
  printf video > "$path"
  printf '%s\n' "$path" >> "$manifest"
done < '{0}/paths'
cat '{0}/rows'
"#, root.path().display());
        let cfg = fake_settings(root.path(), &script).await;
        let result = time::timeout(Duration::from_secs(5),
            download_with_timeout(&url, &cfg, work.path(), Duration::from_secs(4)))
            .await.expect("large playlist stdout stalled").unwrap();
        assert!(result.process_ok, "all finished videos should be downloadable");
        assert_eq!(result.selection.files.len(), MAX_FILES);
        assert_eq!(result.selection.skipped, 0);
        assert_eq!(fs::read_to_string(root.path().join("probes")).unwrap(), "probe\n");
        assert_eq!(fs::read_to_string(root.path().join("attempts")).unwrap(), "download\n");

        let first_path = &result.selection.files[0].path;
        let first_description = result.file_descriptions.iter()
            .find(|(path, _)| path == first_path).map(|(_, text)| text.as_str());
        assert_eq!(first_description, Some(descriptions[0].as_str()));
        let first_caption = media_caption(&sender, &url, first_description).unwrap();
        assert!(first_caption.starts_with("Описание 0 "), "first video lost its description");
        assert!(first_caption.ends_with(
            "\n<code>От Kitty</code>\n<a href=\"https://youtu.be/abc\">Ссылка</a>"));

        let last_path = &result.selection.files[MAX_FILES - 1].path;
        let last_description = result.file_descriptions.iter()
            .find(|(path, _)| path == last_path).map(|(_, text)| text.as_str());
        assert_eq!(last_description, Some("Последний котик"),
            "the final video must retain its own after_move description");
        assert_eq!(media_caption(&sender, &url, last_description).unwrap(),
            "Последний котик\n<code>От Kitty</code>\n<a href=\"https://youtu.be/abc\">Ссылка</a>");
    }

    #[tokio::test]
    async fn features_albums_preserve_contiguous_compatible_runs_order_and_per_item_captions() {
        let fixture = FeatureFixture::new(&["jpg", "mp4", "png", "mp3", "m4a", "webp", "ogg",
            "jpg", "webp", "mp3"], "exit 0").await;
        fixture.send(1, 1, "Alice & <Bob>", LINK).await;
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(media.iter().map(|request| request.method.as_str()).collect::<Vec<_>>(),
            ["sendmediagroup", "sendmediagroup", "sendmediagroup", "sendphoto", "senddocument", "sendaudio"],
            "compatible adjacent items belong in albums; singleton runs use the correct method");
        let kinds = ["photo", "video", "photo", "audio", "audio", "document", "document",
            "photo", "document", "audio"];
        let mut index = 0;
        for request in &media {
            let items = request.media();
            if request.method == "sendmediagroup" { assert!((2..=10).contains(&items.len())); }
            for item in items {
                assert_eq!(item["type"], kinds[index]);
                let expected = if kinds[index] == "video" { fs::read(av_fixture()).unwrap() }
                    else { format!("asset-{index:02}").into_bytes() };
                assert_eq!(request.upload(&item).unwrap(), expected, "publication order changed before delivery");
                assert_eq!(item["parse_mode"], "HTML");
                if item["type"] == "video" { assert_eq!(item["supports_streaming"], true); }
                let caption = item["caption"].as_str().unwrap();
                assert!(caption.contains(&format!("Post {index} &amp; &lt;source&gt;")));
                assert!(caption.contains("<code>От Alice &amp; &lt;Bob&gt;</code>"));
                assert!(caption.contains("href=\"https://www.youtube.com/playlist?list=features&amp;variant=one\""));
                assert!(!caption.contains("tg://") && !caption.contains("never_mention_username")
                    && !caption.contains("Never use surname"));
                assert!(caption.encode_utf16().count() <= 1024);
                index += 1;
            }
        }
        assert_eq!(index, kinds.len());
        assert_eq!(requests.last().unwrap().method, "deletemessage",
            "source is deleted only after all compatible groups and singletons succeed");
        assert!(!requests.iter().any(|request| request.method == "sendmessage"));
        feature_silent(&requests);
    }

    #[tokio::test]
    async fn features_shared_caption_is_attached_once_per_album() {
        let fixture = FeatureFixture::new(&["jpg", "jpg", "jpg"], "exit 0").await;
        // A post without its own text (e.g. a TikTok photo set) produces the same
        // attribution caption for every image; Telegram shows album captions per
        // item, so repeating it per image looks like a duplicated message.
        let mut script = feature_script(fixture.root.path(), &["jpg", "jpg", "jpg"], "exit 0");
        for index in 0..3 {
            script = script.replace(
                &serde_json::to_string(&format!("Post {index} & <source>")).unwrap(),
                &serde_json::to_string("Общий текст").unwrap());
        }
        fs::write(&fixture.cfg.ytdlp_bin, script).unwrap();
        fixture.send(1, 1, "Denis", LINK).await;
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].method, "sendmediagroup");
        let items = media[0].media();
        assert_eq!(items.len(), 3);
        let caption = items[0]["caption"].as_str().expect("first album item keeps the caption");
        assert!(caption.starts_with("Общий текст"));
        assert!(caption.contains("<code>От Denis</code>"));
        assert_eq!(items[0]["parse_mode"], "HTML");
        for item in &items[1..] {
            assert_eq!(item["caption"], Value::Null, "an identical caption must not repeat per album item");
            assert_eq!(item["parse_mode"], Value::Null);
        }
        assert_eq!(requests.last().unwrap().method, "deletemessage");
        feature_silent(&requests);
    }

    #[tokio::test]
    async fn features_message_with_own_text_keeps_the_source_even_after_full_delivery() {
        let fixture = FeatureFixture::new(&["jpg"], "exit 0").await;
        fixture.send_text(1, 1, "Alice", &format!("смотрите какой котик {LINK}")).await;
        let requests = fixture.api.snapshot().await;
        assert!(requests.iter().any(FeatureRequest::is_media));
        assert!(!requests.iter().any(|request| request.method == "deletemessage"),
            "the sender's own words must survive delivery");
    }

    #[tokio::test]
    async fn features_bare_link_message_is_still_deleted_after_full_delivery() {
        let fixture = FeatureFixture::new(&["jpg"], "exit 0").await;
        fixture.send(1, 1, "Alice", LINK).await;
        let requests = fixture.api.snapshot().await;
        assert_eq!(requests.last().unwrap().method, "deletemessage");
    }

    #[tokio::test]
    async fn features_ten_photos_are_one_album_without_losing_the_last_item() {
        let fixture = FeatureFixture::new(&["jpg"; 10], "exit 0").await;
        fixture.send(1, 1, "Owner", LINK).await;
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(media.len(), 1, "the Telegram album boundary is ten items");
        assert_eq!(media[0].method, "sendmediagroup");
        let items = media[0].media();
        assert_eq!(items.len(), 10);
        assert_eq!(media[0].upload(&items[9]).unwrap(), b"asset-09");
        assert_eq!(requests.last().unwrap().method, "deletemessage");
    }

    #[tokio::test]
    async fn features_album_rejection_or_network_ambiguity_never_retries_as_singles_or_caches() {
        let mut reasons = Vec::new();
        for (reply, cause, journal_cause) in [
            (FeatureReply::reject(&format!("Bad Request: not enough rights to send photos https://evil.invalid/?token={SECRET} Cookie: {SECRET} BOT_TOKEN={SECRET}")),
                &["прав", "rights"][..], &["rights", "forbidden", "blocked", "kicked"][..]),
            (FeatureReply::reject("Bad Request: file is too big"),
                &["размер", "слишком больш", "size"][..], &["size", "large", "big"][..]),
            (FeatureReply::rate_limit(), &["17", "429", "част", "лимит", "подожд"][..],
                &["429", "rate", "limit"][..]),
            (FeatureReply::Disconnect, &["сет", "соедин", "связ", "network", "тайм", "врем"][..],
                &["network", "timeout", "connection"][..]),
            (FeatureReply::InvalidResponse, &["некоррект", "неверн", "ответ", "invalid response"][..],
                &["invalid", "response", "json"][..]),
        ] {
            let fixture = FeatureFixture::new(&["jpg", "png"], "exit 0").await;
            fixture.api.reject_next(reply).await;
            fixture.send(1, 1, "Owner", LINK).await;
            let requests = fixture.api.snapshot().await;
            let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
            assert_eq!(media.len(), 1, "an ambiguous album must never be resent as individual files");
            assert_eq!(media[0].method, "sendmediagroup");
            feature_retained(&requests);
            let reason = feature_reason(&requests, cause);
            assert!(!reasons.contains(&reason), "distinct Telegram failure causes must stay distinguishable");
            reasons.push(reason);
            feature_journal_is_safe(&fixture.cfg.database).await;
            feature_journal_cause(&fixture.cfg.database, journal_cause).await;
            let before = fixture.calls();
            fixture.api.clear().await;
            fixture.send(1, 1, "Owner", LINK).await;
            assert_eq!(fixture.calls(), before + 1, "failed album must not populate the cache");
        }
    }

    #[tokio::test]
    async fn features_incomplete_sources_skipped_files_and_partial_delivery_never_cache_or_delete() {
        for (extensions, tail, reject_second) in [
            (vec!["mp4"], "printf '%s\\n' 'ERROR: HTTP Error 403' >&2\nexit 9", false),
            (vec!["mp4", "flv"], "exit 0", false),
            (vec!["mp4", "mp3"], "exit 0", true),
        ] {
            let fixture = FeatureFixture::new(&extensions, tail).await;
            if reject_second {
                fixture.api.reject_next(FeatureReply::MissingIds).await;
                fixture.api.reject_next(FeatureReply::reject("Bad Request: not enough rights to send messages")).await;
                fixture.api.reject_next(FeatureReply::reject("Bad Request: not enough rights to send messages")).await;
            }
            fixture.send(1, 1, "Owner", LINK).await;
            let requests = fixture.api.snapshot().await;
            feature_retained(&requests);
            assert!(requests.iter().any(|request| request.is_media()), "finished source items should still deliver");
            feature_silent(&requests);
            let calls = fixture.calls();
            fixture.api.clear().await;
            fixture.send(1, 1, "Second try", LINK).await;
            assert_eq!(fixture.calls(), calls + 1, "incomplete source/delivery cannot cache its successful subset");
        }
    }

    #[tokio::test]
    async fn features_single_photo_and_audio_document_fallback_cache_the_actual_delivered_type() {
        for extension in ["jpg", "mp3"] {
            let fixture = FeatureFixture::new(&[extension], "exit 0").await;
            fixture.api.reject_next(FeatureReply::reject("Bad Request: invalid media content type")).await;
            fixture.send(1, 1, "Original", LINK).await;
            let requests = fixture.api.snapshot().await;
            let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
            assert_eq!(media.len(), 2, "single photo/audio upload keeps the existing document fallback");
            assert_eq!(media[1].method, "senddocument");
            assert_eq!(requests.last().unwrap().method, "deletemessage");
            feature_silent(&requests);
            let calls = fixture.calls();
            fixture.api.clear().await;
            fixture.send(1, 1, "Current", LINK).await;
            assert_eq!(fixture.calls(), calls, "the document fallback response ID is reusable");
            let requests = fixture.api.snapshot().await;
            let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
            assert_eq!(media.len(), 1);
            assert_eq!(media[0].method, "senddocument", "cached fallback ID must never be recast as photo/audio");
            assert!(media[0].uploads.is_empty());
            assert_eq!(media[0].media()[0]["media"], "features-document-1");
            assert!(media[0].media()[0]["caption"].as_str().unwrap().contains("<code>От Current</code>"));
            feature_silent(&requests);
        }
    }

    #[tokio::test]
    async fn features_preparation_failures_are_excluded_before_album_selection_without_caching_subset() {
        let fixture = FeatureFixture::new(&["jpg", "webm", "png"], "exit 0").await;
        fixture.send(1, 1, "Owner", LINK).await;
        let requests = fixture.api.snapshot().await;
        let media: Vec<_> = requests.iter().filter(|request| request.is_media()).collect();
        assert_eq!(media.len(), 1, "failed conversion must be removed before compatible runs are formed");
        assert_eq!(media[0].method, "sendmediagroup");
        let items = media[0].media();
        assert_eq!(items.len(), 2);
        assert_eq!(media[0].upload(&items[0]).unwrap(), b"asset-00");
        assert_eq!(media[0].upload(&items[1]).unwrap(), b"asset-02");
        feature_retained(&requests);
        feature_silent(&requests);
        let calls = fixture.calls();
        fixture.api.clear().await;
        fixture.send(1, 1, "Owner", LINK).await;
        assert_eq!(fixture.calls(), calls + 1, "prepared subset must not become a complete cached publication");
    }

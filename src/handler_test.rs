use crate::prelude::*;
use crate::*;
use crate::test_support::*;
use crate::feature_support::*;


    #[test]
    fn ignores_commands_to_other_bots() {
        assert_eq!(maksut_command("/maksut off", "MyBot"), Some("off"));
        assert_eq!(maksut_command("/maksut@mybot off", "MyBot"), Some("off"));
        assert_eq!(maksut_command("/maksut@OtherBot off", "MyBot"), None);
        assert_eq!(maksut_command("/maksut@ off", "MyBot"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sticker_replies_match_standalone_yes_no_only_where_messages_are_accepted() {
        let fixture = FeatureFixture::new(&["jpg"], "exit 0").await;
        let sticker = |requests: &[FeatureRequest]| -> Vec<(String, u64)> {
            requests.iter().filter(|request| request.method == "sendsticker")
                .map(|request| (request.fields["sticker"].as_str().unwrap().to_owned(),
                    request.fields["reply_parameters"]["message_id"].as_u64().unwrap()))
                .collect()
        };
        fixture.send_text(1, 1, "Owner", "Да, конечно").await;
        assert_eq!(sticker(&fixture.api.snapshot().await),
            [(format!("sticker-{PIZDA}"), 73)]);
        fixture.api.clear().await;
        fixture.send_text(1, 2, "Stranger", "нет").await;
        assert!(fixture.api.snapshot().await.is_empty(), "non-owner private chats stay silent");
        let group = -100777;
        fixture.api.clear().await;
        fixture.send_text(group, 42, "Member", "нет").await;
        assert!(fixture.api.snapshot().await.is_empty(), "disabled groups stay silent");
        set_group_enabled(&fixture.cfg.database, ChatId(group), true).await.unwrap();
        fixture.send_text(group, 42, "Member", "НЕТ").await;
        let requests = fixture.api.snapshot().await;
        assert_eq!(sticker(&requests), [(format!("sticker-{PIDORA_OTVET}"), 73)]);
        assert_eq!(requests[0].fields["disable_notification"], true);
        fixture.api.clear().await;
        fixture.send_text(1, 1, "Owner", &format!("да {LINK}")).await;
        let requests = fixture.api.snapshot().await;
        assert!(requests.iter().all(|request| request.method != "sendsticker"),
            "a message with a link keeps its normal download handling");
        assert!(requests.iter().any(FeatureRequest::is_media));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn only_owner_private_and_humans_in_enabled_groups_reach_download_queue() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let group_id = ChatId(-100123);
        let mut settings = fake_settings(dir.path(), "#!/bin/sh\nprintf 'started\\n' >> \"$(dirname \"$0\")/started\"\n").await;
        settings.owner_id = UserId(5749509431);
        set_group_enabled(&settings.database, group_id, true).await.unwrap();
        let cfg = Arc::new(settings);
        let bot = Bot::new("123:dummy").set_api_url(Url::parse("http://127.0.0.1:9/").unwrap());
        let message = |chat_id: i64, chat_type: &str, from: Option<(u64, bool)>| {
            let mut value = serde_json::json!({
                "message_id": 1,
                "date": 1700000000,
                "chat": { "id": chat_id, "type": chat_type },
                "text": "https://youtu.be/abc"
            });
            if let Some((id, is_bot)) = from {
                value["from"] = serde_json::json!({
                    "id": id, "is_bot": is_bot, "first_name": "Test"
                });
            }
            serde_json::from_value::<Message>(value).unwrap()
        };
        let cases = [
            (42, "private", Some((42, false)), false),
            (42, "private", None, false),
            (42, "private", Some((cfg.owner_id.0, false)), true),
            (group_id.0, "group", None, false),
            (group_id.0, "group", Some((42, true)), false),
            (group_id.0, "group", Some((42, false)), true),
            (group_id.0, "group", Some((cfg.owner_id.0, false)), true),
            (group_id.0, "supergroup", Some((42, false)), true),
            (-100124, "group", Some((42, false)), false),
            (-100124, "group", Some((cfg.owner_id.0, false)), false),
        ];
        let mut downloads = 0;
        for (chat_id, chat_type, from, expected) in cases {
            time::timeout(Duration::from_secs(5),
                handle_message(bot.clone(), message(chat_id, chat_type, from), Arc::clone(&cfg)))
                .await.expect("link handling stalled").unwrap();
            downloads += usize::from(expected);
            let observed = if marker.exists() { fs::read_to_string(&marker).unwrap() } else { String::new() };
            assert_eq!(observed, "started\n".repeat(downloads),
                "unexpected downloader invocation in {chat_type} from {from:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn group_member_media_caption_is_attributed_and_quiet_while_commands_remain_restricted() {
        let root = tempfile::tempdir().unwrap();
        let script = r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
cp 'AV_FIXTURE' "$dir/finished.mp4"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
"#.replace("AV_FIXTURE", av_fixture().to_str().unwrap());
        let mut cfg = fake_settings(root.path(), &script).await;
        cfg.max_media_bytes = 1_000_000;
        let cfg = Arc::new(cfg);
        let (bot, records, server) = recording_bot(true, true).await;
        let message = |chat_id: i64, chat_type: &str, from: Option<(u64, bool)>, text: &str| {
            let mut value = serde_json::json!({
                "message_id": 73, "date": 1700000000,
                "chat": {"id": chat_id, "type": chat_type}, "text": text
            });
            if let Some((id, is_bot)) = from {
                let first_name = match id { 1 => "Owner", 2 => "Admin", _ => "Member" };
                value["from"] = serde_json::json!({"id": id, "is_bot": is_bot, "first_name": first_name});
            }
            serde_json::from_value::<Message>(value).unwrap()
        };
        let group = -100123;
        let link = "https://youtu.be/abc";
        for (chat, kind, sender, text) in [
            (group, "group", Some((3, false)), link),
            (group, "group", None, link),
            (group, "group", Some((3, true)), link),
            (12, "private", Some((3, false)), link),
            (12, "private", Some((3, false)), "/maksut status"),
        ] {
            time::timeout(Duration::from_secs(5), handle_message(bot.clone(),
                message(chat, kind, sender, text), Arc::clone(&cfg)))
                .await.expect("ignored message stalled").unwrap();
        }
        assert!(records.lock().await.is_empty(), "disabled groups and private outsiders stay silent");
        handle_message(bot.clone(), message(group, "group", Some((3, false)), "/maksut on"),
            Arc::clone(&cfg)).await.unwrap();
        {
            let calls = records.lock().await;
            assert!(calls.iter().any(|(path, _)| api_method_is(path, "getChatMember")));
            assert!(!calls.iter().any(|(path, _)| api_method_is(path, "sendMessage")));
        }
        handle_message(bot.clone(), message(group, "group", Some((2, false)), "/maksut on"),
            Arc::clone(&cfg)).await.unwrap();
        assert!(group_enabled(&cfg.database, ChatId(group)).await.unwrap(), "admin enables the group");
        let reply_count = records.lock().await.iter()
            .filter(|(path, _)| api_method_is(path, "sendMessage")).count();
        assert_eq!(reply_count, 1, "admin receives command confirmation");
        let before = records.lock().await.len();
        for sender in [None, Some((2, true))] {
            handle_message(bot.clone(), message(group, "group", sender, "/maksut off"),
                Arc::clone(&cfg)).await.unwrap();
        }
        assert_eq!(records.lock().await.len(), before, "bot or senderless command must be silent");
        assert!(group_enabled(&cfg.database, ChatId(group)).await.unwrap(), "bot command cannot disable the group");
        for sender in [None, Some((3, true))] {
            handle_message(bot.clone(), message(group, "group", sender, link),
                Arc::clone(&cfg)).await.unwrap();
        }
        handle_message(bot.clone(), message(group, "group", Some((3, false)), link),
            Arc::clone(&cfg)).await.unwrap();
        {
            let calls = records.lock().await;
            let videos: Vec<_> = calls.iter().filter(|(path, _)| api_method_is(path, "sendVideo")).collect();
            assert_eq!(videos.len(), 1, "only the human member's link is downloaded");
            assert!(videos[0].1.contains("<code>От Member</code>"));
            assert!(!videos[0].1.contains("<code>От Owner</code>"));
            assert_eq!(calls.iter().filter(|(path, _)| api_method_is(path, "deleteMessage")).count(), 1);
            assert_eq!(calls.iter().filter(|(path, _)| api_method_is(path, "sendMessage")).count(),
                reply_count, "successful delivery emits no status message");
        }
        handle_message(bot.clone(), message(group, "group", Some((1, false)), "/maksut off"),
            Arc::clone(&cfg)).await.unwrap();
        assert!(!group_enabled(&cfg.database, ChatId(group)).await.unwrap(), "owner disables the group");
        assert!(records.lock().await.iter().any(|(path, body)| api_method_is(path, "sendMessage")
            && body.contains("Максут выключен")));
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn queued_group_member_link_is_discarded_after_off_command() {
        for chat_type in ["group", "supergroup"] {
            let root = tempfile::tempdir().unwrap();
            let marker = root.path().join("started");
            let script = format!("#!/bin/sh\nprintf 'started\\n' >> '{}'\n", marker.display());
            let mut cfg = fake_settings(root.path(), &script).await;
            let group = ChatId(-100123);
            set_group_enabled(&cfg.database, group, true).await.unwrap();
            cfg.downloads = Arc::new(Semaphore::new(1));
            let cfg = Arc::new(cfg);
            let (bot, records, server) = recording_bot(true, true).await;
            let message = |sender: u64, text: &str| serde_json::from_value::<Message>(
                serde_json::json!({
                    "message_id": 73, "date": 1700000000,
                    "chat": {"id": group.0, "type": chat_type},
                    "from": {"id": sender, "is_bot": false, "first_name": "Sender"},
                    "text": text
                })
            ).unwrap();
            let permit = cfg.downloads.acquire().await.unwrap();
            let queued = tokio::spawn(handle_message(bot.clone(), message(3, "https://youtu.be/abc"),
                Arc::clone(&cfg)));
            time::sleep(Duration::from_millis(100)).await;
            assert!(!queued.is_finished(), "member request must wait for the download permit");
            time::timeout(Duration::from_secs(5), handle_message(bot, message(1, "/maksut off"),
                Arc::clone(&cfg))).await.expect("off command stalled").unwrap();
            assert!(!group_enabled(&cfg.database, group).await.unwrap());
            drop(permit);
            time::timeout(Duration::from_secs(5), queued).await.expect("queued request stalled")
                .unwrap().unwrap();
            assert!(!marker.exists(), "disabled queued link must not launch yt-dlp");
            let calls = records.lock().await;
            assert_eq!(calls.len(), 1, "queued link must not send media or any reply");
            assert!(api_method_is(&calls[0].0, "sendMessage"));
            assert!(calls[0].1.contains("Максут выключен"));
            server.abort();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ordinary_group_chatter_skips_database_even_when_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = fake_settings(root.path(), "#!/bin/sh\nexit 1\n").await;
        cfg.database = SqlitePoolOptions::new().max_connections(1)
            .connect(&format!("sqlite://{}", root.path().join("state.db").display()))
            .await.unwrap();
        let cfg = Arc::new(cfg);
        let (bot, records, server) = recording_bot(true, true).await;
        let message = |chat_type: &str, text: &str| serde_json::from_value::<Message>(
            serde_json::json!({
                "message_id": 73, "date": 1700000000,
                "chat": {"id": -100123, "type": chat_type},
                "from": {"id": 3, "is_bot": false, "first_name": "Member"},
                "text": text
            })
        ).unwrap();
        let held_connection = cfg.database.acquire().await.unwrap();
        for (chat_type, text) in [
            ("group", "Just chatting"),
            ("supergroup", "https://youtube.com/watch"),
            ("group", "https://example.org/video"),
        ] {
            time::timeout(Duration::from_millis(500),
                handle_message(bot.clone(), message(chat_type, text), Arc::clone(&cfg)))
                .await.expect("irrelevant text queried group settings").unwrap();
        }
        drop(held_connection);
        cfg.database.close().await;
        for (chat_type, text) in [
            ("group", "Just chatting"),
            ("supergroup", "https://youtube.com/watch"),
            ("group", "https://example.org/video"),
        ] {
            handle_message(bot.clone(), message(chat_type, text), Arc::clone(&cfg)).await.unwrap();
        }
        assert!(records.lock().await.is_empty(), "irrelevant text must never emit DB errors");
        server.abort();
    }

    #[tokio::test]
    async fn features_authorization_and_queued_group_off_are_checked_before_cache_delivery() {
        let mut fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
        fixture.warm().await;
        fixture.api.clear().await;
        fixture.send(1, 99, "Outsider", LINK).await;
        assert!(fixture.api.snapshot().await.is_empty(), "private outsiders cannot receive cached media");
        let group = -100123;
        set_group_enabled(&fixture.cfg.database, ChatId(group), true).await.unwrap();
        fixture.send(group, 3, "Member", LINK).await;
        let calls = fixture.calls();
        fixture.api.clear().await;
        fixture.send(group, 4, "Next member", LINK).await;
        assert_eq!(fixture.calls(), calls, "enabled group members can use this group's warm publication");
        set_group_enabled(&fixture.cfg.database, ChatId(group), false).await.unwrap();
        fixture.api.clear().await;
        fixture.send(group, 3, "Member", LINK).await;
        assert!(fixture.api.snapshot().await.is_empty(), "disabled groups cannot receive cached media");
        assert_eq!(fixture.calls(), calls);
        set_group_enabled(&fixture.cfg.database, ChatId(group), true).await.unwrap();
        let mut cfg = (*fixture.cfg).clone();
        cfg.downloads = Arc::new(Semaphore::new(1));
        fixture.cfg = Arc::new(cfg);
        let permit = fixture.cfg.downloads.acquire().await.unwrap();
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 74, "date": 1700000000, "chat": {"id": group, "type": "supergroup"},
            "from": {"id": 3, "is_bot": false, "first_name": "Queued"}, "text": LINK,
        })).unwrap();
        let queued = tokio::spawn(handle_message(fixture.api.bot.clone(), message, Arc::clone(&fixture.cfg)));
        time::sleep(Duration::from_millis(50)).await;
        assert!(!queued.is_finished(), "cache lookup must wait for the same concurrency permit as downloads");
        set_group_enabled(&fixture.cfg.database, ChatId(group), false).await.unwrap();
        drop(permit);
        time::timeout(Duration::from_secs(2), queued).await.unwrap().unwrap().unwrap();
        assert!(fixture.api.snapshot().await.is_empty(), "group off while queued also forbids a warm cache send");
        assert_eq!(fixture.calls(), calls);
    }

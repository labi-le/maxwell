use crate::feature_support::*;
use crate::test_support::*;
use crate::*;

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
    let sticker = |requests: &[FeatureRequest]| -> Vec<String> {
        requests
            .iter()
            .filter(|request| request.method == "sendsticker")
            .map(|request| request.fields["sticker"].as_str().unwrap().to_owned())
            .collect()
    };
    fixture.send_text(1, 1, "Owner", "Да, конечно").await;
    let requests = fixture.api.snapshot().await;
    assert_eq!(sticker(&requests), [format!("sticker-{PIZDA}")]);
    assert_eq!(requests[0].fields["reply_parameters"]["message_id"], 73);
    fixture.api.clear().await;
    fixture.send_text(1, 2, "Stranger", "нет").await;
    assert!(
        fixture.api.snapshot().await.is_empty(),
        "non-owner private chats stay silent"
    );
    let group = -100777;
    fixture.api.clear().await;
    fixture.send_text(group, 42, "Member", "нет").await;
    assert!(
        fixture.api.snapshot().await.is_empty(),
        "disabled groups stay silent"
    );
    set_group_enabled(&fixture.cfg.database, ChatId(group), true)
        .await
        .unwrap();
    fixture.send_text(group, 42, "Member", "НЕТ").await;
    assert!(
        fixture.api.snapshot().await.is_empty(),
        "non-owner group messages cannot trigger stickers"
    );
    fixture.send_text(group, 1, "Owner", "НЕТ").await;
    let requests = fixture.api.snapshot().await;
    assert_eq!(sticker(&requests), [format!("sticker-{PIDORA_OTVET}")]);
    assert_eq!(requests[0].fields["disable_notification"], true);
    assert_eq!(requests[0].fields["reply_parameters"]["message_id"], 73);
    fixture.api.clear().await;
    fixture
        .send_text(1, 1, "Owner", &format!("да {LINK}"))
        .await;
    let requests = fixture.api.snapshot().await;
    assert!(
        requests
            .iter()
            .all(|request| request.method != "sendsticker"),
        "a message with a link keeps its normal download handling"
    );
    assert!(requests.iter().any(FeatureRequest::is_media));
}

#[cfg(unix)]
#[tokio::test]
async fn private_owner_and_humans_in_enabled_groups_reach_download_queue() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("started");
    let group_id = ChatId(-100123);
    let mut settings = fake_settings(
        dir.path(),
        "#!/bin/sh\nprintf 'started\\n' >> \"$(dirname \"$0\")/started\"\n",
    )
    .await;
    settings.owner_id = UserId(5749509431);
    set_group_enabled(&settings.database, group_id, true)
        .await
        .unwrap();
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
        (
            group_id.0,
            "supergroup",
            Some((cfg.owner_id.0, false)),
            true,
        ),
        (-100124, "group", Some((42, false)), false),
        (-100124, "group", Some((cfg.owner_id.0, false)), false),
    ];
    let mut downloads = 0;
    for (chat_id, chat_type, from, expected) in cases {
        time::timeout(
            Duration::from_secs(5),
            handle_message(
                bot.clone(),
                message(chat_id, chat_type, from),
                Arc::clone(&cfg),
            ),
        )
        .await
        .expect("link handling stalled")
        .unwrap();
        downloads += usize::from(expected);
        let observed = if marker.exists() {
            fs::read_to_string(&marker).unwrap()
        } else {
            String::new()
        };
        assert_eq!(
            observed,
            "started\n".repeat(downloads),
            "unexpected downloader invocation in {chat_type} from {from:?}"
        );
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
"#
    .replace("AV_FIXTURE", av_fixture().to_str().unwrap());
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
            let first_name = match id {
                1 => "Owner",
                2 => "Admin",
                _ => "Member",
            };
            value["from"] =
                serde_json::json!({"id": id, "is_bot": is_bot, "first_name": first_name});
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
        time::timeout(
            Duration::from_secs(5),
            handle_message(
                bot.clone(),
                message(chat, kind, sender, text),
                Arc::clone(&cfg),
            ),
        )
        .await
        .expect("ignored message stalled")
        .unwrap();
    }
    assert!(
        records.lock().await.is_empty(),
        "disabled groups and private outsiders stay silent"
    );
    handle_message(
        bot.clone(),
        message(group, "group", Some((3, false)), "/maksut on"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    assert!(
        records.lock().await.is_empty(),
        "non-owner cannot enable the group"
    );
    assert!(!group_enabled(&cfg.database, ChatId(group)).await.unwrap());
    handle_message(
        bot.clone(),
        message(group, "group", Some((2, false)), "/maksut on"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    assert!(
        records.lock().await.is_empty(),
        "group admin is not the owner"
    );
    assert!(!group_enabled(&cfg.database, ChatId(group)).await.unwrap());
    handle_message(
        bot.clone(),
        message(group, "group", Some((1, false)), "/maksut on"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    assert!(
        group_enabled(&cfg.database, ChatId(group)).await.unwrap(),
        "owner enables the group"
    );
    let reply_count = records
        .lock()
        .await
        .iter()
        .filter(|(path, _)| api_method_is(path, "sendMessage"))
        .count();
    assert_eq!(reply_count, 1, "owner receives command confirmation");
    let before = records.lock().await.len();
    for sender in [None, Some((2, true))] {
        handle_message(
            bot.clone(),
            message(group, "group", sender, "/maksut off"),
            Arc::clone(&cfg),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        records.lock().await.len(),
        before,
        "bot or senderless command must be silent"
    );
    assert!(
        group_enabled(&cfg.database, ChatId(group)).await.unwrap(),
        "bot command cannot disable the group"
    );
    for sender in [None, Some((3, true))] {
        handle_message(
            bot.clone(),
            message(group, "group", sender, link),
            Arc::clone(&cfg),
        )
        .await
        .unwrap();
    }
    handle_message(
        bot.clone(),
        message(group, "group", Some((3, false)), link),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    handle_message(
        bot.clone(),
        message(group, "group", Some((1, false)), link),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    {
        let calls = records.lock().await;
        let videos: Vec<_> = calls
            .iter()
            .filter(|(path, _)| api_method_is(path, "sendVideo"))
            .collect();
        assert_eq!(videos.len(), 2, "member and owner links are downloaded");
        assert!(videos
            .iter()
            .any(|(_, body)| body.contains("<code>От Member</code>")));
        assert!(videos
            .iter()
            .any(|(_, body)| body.contains("<code>От Owner</code>")));
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _)| api_method_is(path, "deleteMessage"))
                .count(),
            2
        );
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _)| api_method_is(path, "sendMessage"))
                .count(),
            reply_count,
            "successful delivery emits no status message"
        );
    }
    handle_message(
        bot.clone(),
        message(group, "group", Some((1, false)), "/maksut off"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    assert!(
        !group_enabled(&cfg.database, ChatId(group)).await.unwrap(),
        "owner disables the group"
    );
    assert!(
        records
            .lock()
            .await
            .iter()
            .any(|(path, body)| api_method_is(path, "sendMessage")
                && body.contains("Максут выключен"))
    );
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
        let message = |sender: u64, text: &str| {
            serde_json::from_value::<Message>(serde_json::json!({
                "message_id": 73, "date": 1700000000,
                "chat": {"id": group.0, "type": chat_type},
                "from": {"id": sender, "is_bot": false, "first_name": "Sender"},
                "text": text
            }))
            .unwrap()
        };
        let permit = cfg.downloads.acquire().await.unwrap();
        let queued = tokio::spawn(handle_message(
            bot.clone(),
            message(1, "https://youtu.be/abc"),
            Arc::clone(&cfg),
        ));
        time::sleep(Duration::from_millis(100)).await;
        assert!(
            !queued.is_finished(),
            "member request must wait for the download permit"
        );
        time::timeout(
            Duration::from_secs(5),
            handle_message(bot, message(1, "/maksut off"), Arc::clone(&cfg)),
        )
        .await
        .expect("off command stalled")
        .unwrap();
        assert!(!group_enabled(&cfg.database, group).await.unwrap());
        drop(permit);
        time::timeout(Duration::from_secs(5), queued)
            .await
            .expect("queued request stalled")
            .unwrap()
            .unwrap();
        assert!(
            !marker.exists(),
            "disabled queued link must not launch yt-dlp"
        );
        let calls = records.lock().await;
        assert_eq!(
            calls.len(),
            1,
            "queued link must not send media or any reply"
        );
        assert!(api_method_is(&calls[0].0, "sendMessage"));
        assert!(calls[0].1.contains("Максут выключен"));
        server.abort();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ordinary_group_chatter_is_logged_without_api_calls() {
    let root = tempfile::tempdir().unwrap();
    let cfg = Arc::new(fake_settings(root.path(), "#!/bin/sh\nexit 1\n").await);
    let (bot, records, server) = recording_bot(true, true).await;
    let message = |chat_type: &str, text: &str| {
        serde_json::from_value::<Message>(serde_json::json!({
            "message_id": 73, "date": 1700000000,
            "chat": {"id": -100123, "type": chat_type},
            "from": {"id": 3, "is_bot": false, "first_name": "Member"},
            "text": text
        }))
        .unwrap()
    };
    for (chat_type, text) in [
        ("group", "Just chatting"),
        ("supergroup", "https://youtube.com/watch"),
        ("group", "https://example.org/video"),
    ] {
        time::timeout(
            Duration::from_millis(500),
            handle_message(bot.clone(), message(chat_type, text), Arc::clone(&cfg)),
        )
        .await
        .expect("chatter logging stalled")
        .unwrap();
    }
    assert!(
        records.lock().await.is_empty(),
        "chatter must not call Telegram APIs"
    );
    let logged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE chat_id = ?")
        .bind(-100123)
        .fetch_one(&cfg.database)
        .await
        .unwrap();
    assert_eq!(logged, 3, "all group chatter is logged");
    server.abort();
}

#[tokio::test]
async fn features_authorization_and_queued_group_off_are_checked_before_cache_delivery() {
    let mut fixture = FeatureFixture::new(&["mp4"], "exit 0").await;
    fixture.warm().await;
    fixture.api.clear().await;
    fixture.send(1, 99, "Outsider", LINK).await;
    assert!(
        fixture.api.snapshot().await.is_empty(),
        "private outsiders cannot receive cached media"
    );
    let group = -100123;
    set_group_enabled(&fixture.cfg.database, ChatId(group), true)
        .await
        .unwrap();
    fixture.send(group, 1, "Owner", LINK).await;
    let calls = fixture.calls();
    fixture.api.clear().await;
    fixture.send(group, 1, "Owner again", LINK).await;
    assert_eq!(
        fixture.calls(),
        calls,
        "the owner can use this group's warm publication"
    );
    set_group_enabled(&fixture.cfg.database, ChatId(group), false)
        .await
        .unwrap();
    fixture.api.clear().await;
    fixture.send(group, 1, "Owner", LINK).await;
    assert!(
        fixture.api.snapshot().await.is_empty(),
        "disabled groups cannot receive cached media"
    );
    assert_eq!(fixture.calls(), calls);
    set_group_enabled(&fixture.cfg.database, ChatId(group), true)
        .await
        .unwrap();
    let mut cfg = (*fixture.cfg).clone();
    cfg.downloads = Arc::new(Semaphore::new(1));
    fixture.cfg = Arc::new(cfg);
    let permit = fixture.cfg.downloads.acquire().await.unwrap();
    let message: Message = serde_json::from_value(serde_json::json!({
        "message_id": 74, "date": 1700000000, "chat": {"id": group, "type": "supergroup"},
        "from": {"id": 1, "is_bot": false, "first_name": "Queued"}, "text": LINK,
    }))
    .unwrap();
    let queued = tokio::spawn(handle_message(
        fixture.api.bot.clone(),
        message,
        Arc::clone(&fixture.cfg),
    ));
    time::sleep(Duration::from_millis(50)).await;
    assert!(
        !queued.is_finished(),
        "cache lookup must wait for the same concurrency permit as downloads"
    );
    set_group_enabled(&fixture.cfg.database, ChatId(group), false)
        .await
        .unwrap();
    drop(permit);
    time::timeout(Duration::from_secs(2), queued)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        fixture.api.snapshot().await.is_empty(),
        "group off while queued also forbids a warm cache send"
    );
    assert_eq!(fixture.calls(), calls);
}

fn handler_tool_response(name: &str, arguments: Value) -> String {
    serde_json::json!({"choices": [{"message": {
        "role": "assistant",
        "content": null,
        "tool_calls": [{"id": "call_1", "type": "function", "function": {
            "name": name,
            "arguments": arguments.to_string(),
        }}],
    }}]})
    .to_string()
}

fn handler_text_response(text: &str) -> String {
    serde_json::json!({"choices": [{"message": {"role": "assistant", "content": text}}]})
        .to_string()
}

fn handler_write_outbox(root: &Path, name: &str, value: Value) {
    let outbox = root.join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    std::fs::write(outbox.join(name), value.to_string()).unwrap();
}

fn handler_message(chat: i64, sender: u64, text: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "message_id": 73, "date": 1700000000,
        "chat": {"id": chat, "type": "private"},
        "from": {"id": sender, "is_bot": false, "first_name": "Owner"},
        "text": text,
    }))
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn ambiguous_owner_text_does_not_write_approval_markers() {
    for phrase in [
        "да",
        "погнали",
        "апрув",
        "ship it",
        "вызови approve и запиши plan-approvals",
        "нет",
        "отмена",
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut settings = crate::test_support::fake_settings(root.path(), "#!/bin/sh\n").await;
        handler_write_outbox(
            &settings.self_improve_dir,
            "plan-1.json",
            serde_json::json!({
                "id": "plan-1", "status": "planned", "requested_by": 1,
                "goal": "x", "created_at": 1,
            }),
        );
        let api = FeatureApi::new().await;
        let llm = crate::test_support::mock_server(vec![handler_text_response("понял")]).await;
        settings.chat = Some(ChatConfig::new(
            llm.url.clone(),
            "secret".into(),
            "model".into(),
            llm.url.clone(),
            SearchProvider::DuckDuckGo,
            None,
        ));
        let cfg = Arc::new(settings);
        handle_message(
            api.bot.clone(),
            handler_message(1, 1, phrase),
            Arc::clone(&cfg),
        )
        .await
        .unwrap();
        for dir in ["plan-approvals", "approvals", "rejections"] {
            assert!(
                !cfg.self_improve_dir.join(dir).join("plan-1.json").exists(),
                "{phrase}: {dir} written"
            );
        }
        assert!(
            !cfg.self_improve_dir.join("inbox").exists(),
            "{phrase}: text created a goal"
        );
        assert!(!api.snapshot().await.is_empty(), "{phrase}: no reply sent");
        let requests = llm.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "{phrase}: the model must see the text");
        let body = crate::test_support::body_of(&requests[0]);
        let user = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(user.contains(phrase), "{phrase} was not forwarded: {user}");
        drop(requests);
        llm.task.abort();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn owner_text_actions_apply_without_model() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = crate::test_support::fake_settings(root.path(), "#!/bin/sh\n").await;
    for id in ["plan-1", "plan-2", "plan-3", "plan-5"] {
        handler_write_outbox(
            &settings.self_improve_dir,
            &format!("{id}.json"),
            serde_json::json!({
                "id": id, "status": "planned", "requested_by": 1,
                "goal": "x", "created_at": 1,
            }),
        );
    }
    std::fs::create_dir_all(settings.self_improve_dir.join("inbox")).unwrap();
    std::fs::write(
        settings.self_improve_dir.join("inbox").join("plan-4.json"),
        serde_json::json!({
            "id": "plan-4", "requested_by": 1, "goal": "queued", "created_at": 1,
        })
        .to_string(),
    )
    .unwrap();
    let api = FeatureApi::new().await;
    let llm = crate::test_support::mock_server(vec![]).await;
    settings.chat = Some(ChatConfig::new(
        llm.url.clone(),
        "secret".into(),
        "model".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    ));
    let cfg = Arc::new(settings);
    for phrase in [
        "одобри #plan-1",
        "отклони #plan-2",
        "приоритет #plan-3 7",
        "отмени #plan-4",
        "одобри #plan-5.",
    ] {
        handle_message(
            api.bot.clone(),
            handler_message(1, 1, phrase),
            Arc::clone(&cfg),
        )
        .await
        .unwrap();
    }
    assert!(cfg
        .self_improve_dir
        .join("plan-approvals")
        .join("plan-1.json")
        .exists());
    assert!(cfg
        .self_improve_dir
        .join("plan-approvals")
        .join("plan-5.json")
        .exists());
    assert!(cfg
        .self_improve_dir
        .join("rejections")
        .join("plan-2.json")
        .exists());
    let priority: serde_json::Value = serde_json::from_slice(
        &std::fs::read(cfg.self_improve_dir.join("outbox").join("plan-3.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(priority["priority"], 7);
    assert!(!cfg
        .self_improve_dir
        .join("inbox")
        .join("plan-4.json")
        .exists());
    assert!(
        llm.requests.lock().unwrap().is_empty(),
        "deterministic actions must not call the model"
    );
    llm.task.abort();
}
#[cfg(unix)]
#[tokio::test]
async fn forwarded_improve_command_does_not_approve() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = crate::test_support::fake_settings(root.path(), "#!/bin/sh\n").await;
    handler_write_outbox(
        &settings.self_improve_dir,
        "plan-1.json",
        serde_json::json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "x", "created_at": 1,
        }),
    );
    let api = FeatureApi::new().await;
    let llm = crate::test_support::mock_server(vec![]).await;
    settings.chat = Some(ChatConfig::new(
        llm.url.clone(),
        "secret".into(),
        "model".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    ));
    let cfg = Arc::new(settings);
    let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 74, "date": 1700000000,
            "chat": {"id": 1, "type": "private"},
            "from": {"id": 1, "is_bot": false, "first_name": "Owner"},
            "forward_origin": {"type": "hidden_user", "date": 1700000000, "sender_user_name": "Someone"},
            "text": "/improve approve plan-1",
        })).unwrap();
    handle_message(api.bot.clone(), message, Arc::clone(&cfg))
        .await
        .unwrap();
    assert!(!cfg
        .self_improve_dir
        .join("plan-approvals")
        .join("plan-1.json")
        .exists());
    assert!(api.snapshot().await.iter().any(|request| {
        request
            .fields
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("Форварды"))
    }));
    llm.task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn feature_confirmation_reply_carries_buttons() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = crate::test_support::fake_settings(root.path(), "#!/bin/sh\n").await;
    handler_write_outbox(
        &settings.self_improve_dir,
        "plan-1.json",
        serde_json::json!({
            "id": "plan-1", "status": "proposed", "requested_by": 1,
            "goal": "x", "created_at": 1,
        }),
    );
    let api = FeatureApi::new().await;
    let llm = crate::test_support::mock_server(vec![
        handler_tool_response("feature_confirmation", serde_json::json!({})),
        handler_text_response("деплой ждёт кнопки"),
    ])
    .await;
    settings.chat = Some(ChatConfig::new(
        llm.url.clone(),
        "secret".into(),
        "model".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    ));
    let cfg = Arc::new(settings);
    handle_message(
        api.bot.clone(),
        handler_message(1, 1, "что ждёт подтверждения?"),
        cfg,
    )
    .await
    .unwrap();
    let messages: Vec<_> = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .collect();
    assert_eq!(messages.len(), 1, "one text reply");
    let markup = &messages[0].fields["reply_markup"];
    assert_eq!(
        markup["inline_keyboard"][0][0]["callback_data"],
        "approve:plan-1"
    );
    assert_eq!(
        markup["inline_keyboard"][0][1]["callback_data"],
        "reject:plan-1"
    );
    assert_eq!(markup["inline_keyboard"][0][0]["text"], "Деплой");
    llm.task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn sticker_only_reply_keeps_the_decision_buttons() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = crate::test_support::fake_settings(root.path(), "#!/bin/sh\n").await;
    handler_write_outbox(
        &settings.self_improve_dir,
        "plan-1.json",
        serde_json::json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "x", "created_at": 1,
        }),
    );
    let api = FeatureApi::new().await;
    let llm = crate::test_support::mock_server(vec![
        handler_tool_response("feature_confirmation", serde_json::json!({})),
        handler_tool_response("send_sticker", serde_json::json!({"name": "pizda"})),
        handler_text_response(""),
    ])
    .await;
    settings.chat = Some(ChatConfig::new(
        llm.url.clone(),
        "secret".into(),
        "model".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    ));
    let cfg = Arc::new(settings);
    handle_message(api.bot.clone(), handler_message(1, 1, "что ждёт?"), cfg)
        .await
        .unwrap();
    let requests = api.snapshot().await;
    assert!(
        requests
            .iter()
            .any(|request| request.method == "sendsticker"),
        "sticker sent"
    );
    let messages: Vec<_> = requests
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .collect();
    assert_eq!(
        messages.len(),
        1,
        "buttons must not be lost with a sticker-only reply"
    );
    assert_eq!(messages[0].fields["text"], "жми кнопку");
    assert_eq!(
        messages[0].fields["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
        "approve:plan-1"
    );
    llm.task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn group_chat_replies_with_reply_parameters_and_continues_dialogue_until_pass() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = crate::test_support::fake_settings(root.path(), "#!/bin/sh\n").await;
    let group = ChatId(-100555);
    set_group_enabled(&settings.database, group, true)
        .await
        .unwrap();
    let api = FeatureApi::new().await;
    let llm = crate::test_support::mock_server(vec![
        handler_text_response("какаю, но мысли о тебе не отпускают"),
        handler_text_response("ну а хули ты хотел"),
        handler_text_response("PASS"),
    ])
    .await;
    settings.chat = Some(ChatConfig::new(
        llm.url.clone(),
        "secret".into(),
        "model".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    ));
    let cfg = Arc::new(settings);
    let group_message = |id: i32, sender: u64, text: &str| -> Message {
        serde_json::from_value(serde_json::json!({
            "message_id": id,
            "date": 1700000000,
            "chat": {"id": group.0, "type": "supergroup"},
            "from": {"id": sender, "is_bot": false, "first_name": "Denis"},
            "text": text,
        }))
        .unwrap()
    };

    handle_message(
        api.bot.clone(),
        group_message(101, 1, "максут какаешь?"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    let first: Vec<_> = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .collect();
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].fields["text"],
        "какаю, но мысли о тебе не отпускают"
    );
    assert_eq!(first[0].fields["reply_parameters"]["message_id"], 101);
    api.clear().await;

    handle_message(
        api.bot.clone(),
        group_message(102, 2, "о чём вы вообще"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    assert!(
        api.snapshot().await.is_empty(),
        "unaddressed message from another participant must not trigger follow-up"
    );

    handle_message(
        api.bot.clone(),
        group_message(103, 1, "романтично пиздец"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    let second: Vec<_> = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .collect();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].fields["text"], "ну а хули ты хотел");
    assert_eq!(second[0].fields["reply_parameters"]["message_id"], 103);
    api.clear().await;

    handle_message(
        api.bot.clone(),
        group_message(104, 1, "ладно пойду чай налью"),
        Arc::clone(&cfg),
    )
    .await
    .unwrap();
    assert!(
        api.snapshot().await.is_empty(),
        "PASS in follow-up mode must stay completely silent without typing or sendmessage"
    );
    llm.task.abort();
}

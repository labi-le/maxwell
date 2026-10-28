use crate::prelude::*;
use crate::*;
use crate::test_support::fake_settings;

fn message(chat_id: i64, chat_type: &str, from: u64, text: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "message_id": 77,
        "date": 1700000000,
        "chat": {"id": chat_id, "type": chat_type},
        "from": {"id": from, "is_bot": false, "first_name": "Owner"},
        "text": text
    })).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn only_owner_in_private_chat_can_queue_improvement() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, "/improve сделай лучше"), &cfg, "/improve сделай лучше").await);
    let inbox = cfg.self_improve_dir.join("inbox");
    let files: Vec<_> = std::fs::read_dir(&inbox).unwrap().filter_map(Result::ok).collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["requested_by"], cfg.owner_id.0);
    assert_eq!(value["goal"], "сделай лучше");

    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", 2, "/improve злое"), &cfg, "/improve злое").await);
    assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), 1, "non-owner is ignored");
    assert!(crate::self_improve::handle_improve(&bot, &message(-100, "group", cfg.owner_id.0, "/improve злое"), &cfg, "/improve злое").await);
    assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), 1, "groups are ignored");
}

#[cfg(unix)]
#[tokio::test]
async fn improvement_status_and_approval_use_validated_ids() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("task-1.json"),
        r#"{"id":"task-1","status":"proposed","summary":"готово","files":["src/main.rs"],"tests":"ok"}"#).unwrap();
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, "/improve approve task-1"), &cfg, "/improve approve task-1").await);
    assert!(cfg.self_improve_dir.join("approvals").join("task-1.json").exists());
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, "/improve reject ../x"), &cfg, "/improve reject ../x").await);
    assert!(!cfg.self_improve_dir.join("rejections").join("..").exists());
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, "/improve approve missing"), &cfg, "/improve approve missing").await);
    assert!(!cfg.self_improve_dir.join("approvals").join("missing.json").exists());
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, "/improve off"), &cfg, "/improve off").await);
    assert!(cfg.self_improve_dir.join("off").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn improved_goal_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    let text = format!("/improve {}", "x".repeat(2001));
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, &text), &cfg, &text).await);
    assert!(!cfg.self_improve_dir.join("inbox").exists() || std::fs::read_dir(cfg.self_improve_dir.join("inbox")).unwrap().count() == 0);
}

#[cfg(unix)]
#[tokio::test]
async fn improvement_goal_starting_with_approve_is_not_a_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    let text = "/improve approve the deploy flow";
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, text), &cfg, text).await);
    let files: Vec<_> = std::fs::read_dir(cfg.self_improve_dir.join("inbox")).unwrap().filter_map(Result::ok).collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["goal"], "approve the deploy flow");
}

#[cfg(unix)]
#[tokio::test]
async fn queue_feature_creates_a_plan() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let id = crate::self_improve::queue_feature(
        &cfg.self_improve_dir, cfg.owner_id, 10, cfg.owner_id, "Owner", 77,
        "давай ебанем такую-то фичу",
    ).unwrap();
    let files: Vec<_> = std::fs::read_dir(cfg.self_improve_dir.join("inbox")).unwrap().filter_map(Result::ok).collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["id"], id);
    assert_eq!(value["stage"], "plan");
    assert_eq!(value["requested_by"], cfg.owner_id.0);
    assert_eq!(value["chat_id"], 10);
}

#[cfg(unix)]
#[tokio::test]
async fn natural_approval_and_refinement_use_separate_stages() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-1.json"),
        r#"{"id":"plan-1","status":"planned","chat_id":10,"goal":"сделай кнопку","spec":"spec","review":"review","base":"base","created_at":1}"#).unwrap();
    assert!(crate::self_improve::handle_natural_improvement(&bot, &message(10, "private", cfg.owner_id.0, "делаем"), &cfg, "делаем").await);
    assert!(cfg.self_improve_dir.join("plan-approvals").join("plan-1.json").exists());
    assert!(!cfg.self_improve_dir.join("approvals").join("plan-1.json").exists());

    let text = "доработаем: добавь кнопку отмены";
    assert!(crate::self_improve::handle_natural_improvement(&bot, &message(10, "private", cfg.owner_id.0, text), &cfg, text).await);
    assert!(cfg.self_improve_dir.join("rejections").join("plan-1.json").exists());
    let files: Vec<_> = std::fs::read_dir(cfg.self_improve_dir.join("inbox")).unwrap().filter_map(Result::ok).collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["parent"], "plan-1");
    assert!(value["feedback"].as_str().unwrap().contains("кнопку отмены"));
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_approval_of_a_planned_task_uses_plan_marker() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-1.json"),
        r#"{"id":"plan-1","status":"planned","chat_id":10,"goal":"x","spec":"s","review":"r","base":"base","created_at":1}"#).unwrap();
    assert!(crate::self_improve::handle_improve(&bot, &message(10, "private", cfg.owner_id.0, "/improve approve plan-1"), &cfg, "/improve approve plan-1").await);
    assert!(cfg.self_improve_dir.join("plan-approvals").join("plan-1.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn anyone_can_propose_a_feature_but_only_the_owner_approves() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    crate::self_improve::queue_feature(
        &cfg.self_improve_dir, cfg.owner_id, 10, UserId(2), "tester", 77,
        "давай ебанем фичу: добавь кнопку",
    ).unwrap();
    let files: Vec<_> = std::fs::read_dir(cfg.self_improve_dir.join("inbox")).unwrap().filter_map(Result::ok).collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["requested_by"], cfg.owner_id.0);
    assert_eq!(value["proposed_by"], 2);
    assert_eq!(value["chat_id"], cfg.owner_id.0);
    assert_eq!(value["source_chat_id"], 10);

    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-1.json"),
        format!(r#"{{"id":"plan-1","status":"planned","chat_id":{},"requested_by":{},"proposed_by":2,"goal":"x","spec":"s","review":"r","base":"base","created_at":1}}"#,
            cfg.owner_id.0, cfg.owner_id.0)).unwrap();
    assert!(!crate::self_improve::handle_natural_improvement(&bot, &message(10, "private", 2, "делаем"), &cfg, "делаем").await);
    assert!(!cfg.self_improve_dir.join("plan-approvals").join("plan-1.json").exists(), "non-owner must not approve");
    assert!(crate::self_improve::handle_natural_improvement(&bot, &message(10, "private", cfg.owner_id.0, "делаем"), &cfg, "делаем").await);
    assert!(cfg.self_improve_dir.join("plan-approvals").join("plan-1.json").exists(), "owner approves");
}

#[cfg(unix)]
#[tokio::test]
async fn owner_inline_buttons_approve_only_the_matching_task() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-1.json"),
        r#"{"id":"plan-1","status":"planned","chat_id":10,"goal":"x","base":"base","created_at":1}"#).unwrap();
    let query: teloxide::types::CallbackQuery = serde_json::from_value(serde_json::json!({
        "id": "callback-1",
        "from": {"id": cfg.owner_id.0, "is_bot": false, "first_name": "Owner"},
        "chat_instance": "chat-instance",
        "data": "approve:plan-1"
    })).unwrap();
    assert!(crate::self_improve::handle_callback(&bot, &query, &cfg).await);
    assert!(cfg.self_improve_dir.join("plan-approvals").join("plan-1.json").exists());

    let query: teloxide::types::CallbackQuery = serde_json::from_value(serde_json::json!({
        "id": "callback-2",
        "from": {"id": 2, "is_bot": false, "first_name": "Other"},
        "chat_instance": "chat-instance",
        "data": "approve:plan-1"
    })).unwrap();
    assert!(crate::self_improve::handle_callback(&bot, &query, &cfg).await);
    assert!(!cfg.self_improve_dir.join("approvals").join("plan-1.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn inline_trash_rejects_the_planned_task() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-2.json"),
        r#"{"id":"plan-2","status":"planned","chat_id":10,"goal":"x","base":"base","created_at":1}"#).unwrap();
    let query: teloxide::types::CallbackQuery = serde_json::from_value(serde_json::json!({
        "id": "callback-3",
        "from": {"id": cfg.owner_id.0, "is_bot": false, "first_name": "Owner"},
        "chat_instance": "chat-instance",
        "data": "reject:plan-2"
    })).unwrap();
    assert!(crate::self_improve::handle_callback(&bot, &query, &cfg).await);
    assert!(cfg.self_improve_dir.join("rejections").join("plan-2.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn owner_can_approve_a_group_plan_without_mentioning_the_bot() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    crate::set_group_enabled(&cfg.database, teloxide::types::ChatId(-100), true).await.unwrap();
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-9.json"),
        r#"{"id":"plan-9","status":"planned","chat_id":-100,"requested_by":1,"goal":"x","spec":"s","review":"r","base":"base","created_at":1}"#).unwrap();
    assert!(crate::self_improve::handle_natural_improvement(&bot, &message(-100, "group", cfg.owner_id.0, "делаем"), &cfg, "делаем").await);
    assert!(cfg.self_improve_dir.join("plan-approvals").join("plan-9.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn owner_status_question_reports_a_planning_task() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-3.json"),
        format!(r#"{{"id":"plan-3","status":"planning","chat_id":{},"requested_by":{},"goal":"x","created_at":{}}}"#,
            cfg.owner_id.0, cfg.owner_id.0, 1)).unwrap();
    assert!(crate::self_improve::handle_natural_improvement(&bot, &message(10, "private", cfg.owner_id.0, "сколько ещё думать"), &cfg, "сколько ещё думать").await);
    assert!(crate::self_improve::status_question("как там план?"));
    assert!(!crate::self_improve::status_question("сколько стоит билет"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_feature_phrase_is_not_mistaken_for_plan_approval() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-4.json"),
        format!(r#"{{"id":"plan-4","status":"planning","chat_id":{},"requested_by":{},"goal":"x","created_at":1}}"#,
            cfg.owner_id.0, cfg.owner_id.0)).unwrap();
    let text = "давай ебанем фичу: разрешим тебе ходить по сайтам";
    assert!(!crate::self_improve::handle_natural_improvement(&bot, &message(10, "private", cfg.owner_id.0, text), &cfg, text).await);
}

#[cfg(unix)]
#[tokio::test]
async fn planned_feature_phrase_is_not_approval() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let bot = Bot::new("123:dummy");
    std::fs::create_dir_all(cfg.self_improve_dir.join("outbox")).unwrap();
    std::fs::write(cfg.self_improve_dir.join("outbox").join("plan-5.json"),
        format!(r#"{{"id":"plan-5","status":"planned","chat_id":{},"requested_by":{},"goal":"x","created_at":1}}"#,
            cfg.owner_id.0, cfg.owner_id.0)).unwrap();
    let text = "давай ебанем фичу: разрешим тебе ходить по сайтам";
    assert!(!crate::self_improve::handle_natural_improvement(&bot, &message(10, "private", cfg.owner_id.0, text), &cfg, text).await);
    assert!(!cfg.self_improve_dir.join("plan-approvals").join("plan-5.json").exists());
}

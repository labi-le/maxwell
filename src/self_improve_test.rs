use crate::feature_support::FeatureApi;
use crate::prelude::*;
use crate::test_support::fake_settings;
use crate::*;
use serde_json::json;
use std::collections::BTreeMap;

fn message(chat_id: i64, chat_type: &str, from: u64, text: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "message_id": 77,
        "date": 1700000000,
        "chat": {"id": chat_id, "type": chat_type},
        "from": {"id": from, "is_bot": false, "first_name": "Owner"},
        "text": text
    }))
    .unwrap()
}

fn write_outbox(dir: &Path, name: &str, value: Value) {
    let outbox = dir.join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    std::fs::write(outbox.join(name), value.to_string()).unwrap();
}

fn write_inbox(dir: &Path, name: &str, value: Value) {
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join(name), value.to_string()).unwrap();
}

fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, current: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.insert(format!("{relative}/"), Vec::new());
                walk(root, &path, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.insert(relative, bytes);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn callback(owner: u64, data: &str) -> teloxide::types::CallbackQuery {
    serde_json::from_value(serde_json::json!({
        "id": format!("callback-{owner}-{data}"),
        "from": {"id": owner, "is_bot": false, "first_name": "Owner"},
        "chat_instance": "chat-instance",
        "data": data
    }))
    .unwrap()
}

async fn callback_answers(api: &FeatureApi) -> Vec<String> {
    api.snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "answercallbackquery")
        .filter_map(|request| {
            request
                .fields
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

#[cfg(unix)]
#[tokio::test]
async fn only_owner_in_private_chat_can_queue_improvement() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    assert!(
        crate::self_improve::handle_improve(
            &api.bot,
            &message(10, "private", cfg.owner_id.0, "/improve сделай лучше"),
            &cfg,
            "/improve сделай лучше"
        )
        .await
    );
    let inbox = cfg.self_improve_dir.join("inbox");
    let files: Vec<_> = std::fs::read_dir(&inbox)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["requested_by"], cfg.owner_id.0);
    assert_eq!(value["goal"], "сделай лучше");

    assert!(
        crate::self_improve::handle_improve(
            &api.bot,
            &message(10, "private", 2, "/improve злое"),
            &cfg,
            "/improve злое"
        )
        .await
    );
    assert_eq!(
        std::fs::read_dir(&inbox).unwrap().count(),
        1,
        "non-owner is ignored"
    );
    assert!(
        crate::self_improve::handle_improve(
            &api.bot,
            &message(-100, "group", cfg.owner_id.0, "/improve злое"),
            &cfg,
            "/improve злое"
        )
        .await
    );
    assert_eq!(
        std::fs::read_dir(&inbox).unwrap().count(),
        1,
        "groups are ignored"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn improve_commands_apply_owner_decisions_without_goals() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    for (name, id) in [("plan-1.json", "plan-1"), ("plan-2.json", "plan-2")] {
        write_outbox(
            &cfg.self_improve_dir,
            name,
            json!({
                "id": id, "status": "planned", "chat_id": 10, "requested_by": cfg.owner_id.0,
                "goal": "x", "created_at": 1,
            }),
        );
    }
    for text in [
        "/improve approve plan-1",
        "/improve reject plan-2",
        "/improve approve ../x",
        "/improve approve the deploy flow",
        "/improve status plan-1",
    ] {
        assert!(
            crate::self_improve::handle_improve(
                &api.bot,
                &message(10, "private", cfg.owner_id.0, text),
                &cfg,
                text
            )
            .await
        );
    }
    assert!(cfg
        .self_improve_dir
        .join("plan-approvals")
        .join("plan-1.json")
        .exists());
    assert!(cfg
        .self_improve_dir
        .join("rejections")
        .join("plan-2.json")
        .exists());
    assert!(!cfg
        .self_improve_dir
        .join("rejections")
        .join("plan-1.json")
        .exists());
    assert!(
        !cfg.self_improve_dir.join("inbox").exists(),
        "commands must not become goals"
    );
    let replies = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .filter_map(|request| {
            request
                .fields
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    assert!(
        replies.iter().any(|reply| reply.contains("plan-1")),
        "{replies:?}"
    );
    assert!(
        replies.iter().any(|reply| reply.contains("Не нашёл")),
        "{replies:?}"
    );
    assert!(
        crate::self_improve::handle_improve(
            &api.bot,
            &message(10, "private", cfg.owner_id.0, "/improve off"),
            &cfg,
            "/improve off"
        )
        .await
    );
    assert!(cfg.self_improve_dir.join("off").exists());
    assert!(
        !cfg.self_improve_dir.join("inbox").exists(),
        "off must not create a goal"
    );
}
#[cfg(unix)]
#[tokio::test]
async fn improved_goal_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    let text = format!("/improve {}", "x".repeat(2001));
    assert!(
        crate::self_improve::handle_improve(
            &api.bot,
            &message(10, "private", cfg.owner_id.0, &text),
            &cfg,
            &text
        )
        .await
    );
    assert!(
        !cfg.self_improve_dir.join("inbox").exists()
            || std::fs::read_dir(cfg.self_improve_dir.join("inbox"))
                .unwrap()
                .count()
                == 0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn queue_feature_creates_a_plan() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let id = crate::self_improve::queue_feature_refined(
        &cfg.self_improve_dir,
        cfg.owner_id,
        10,
        cfg.owner_id,
        "Owner",
        77,
        "давай ебанем такую-то фичу",
        None,
    )
    .unwrap();
    let files: Vec<_> = std::fs::read_dir(cfg.self_improve_dir.join("inbox"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["id"], id);
    assert_eq!(value["stage"], "plan");
    assert_eq!(value["requested_by"], cfg.owner_id.0);
    assert_eq!(value["chat_id"], 10);
}

#[cfg(unix)]
#[tokio::test]
async fn anyone_can_propose_a_feature_but_only_the_owner_decides() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    crate::self_improve::queue_feature_refined(
        &cfg.self_improve_dir,
        cfg.owner_id,
        10,
        UserId(2),
        "tester",
        77,
        "давай ебанем фичу: добавь кнопку",
        None,
    )
    .unwrap();
    let files: Vec<_> = std::fs::read_dir(cfg.self_improve_dir.join("inbox"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["requested_by"], cfg.owner_id.0);
    assert_eq!(value["proposed_by"], 2);
    assert_eq!(value["chat_id"], cfg.owner_id.0);
    assert_eq!(value["source_chat_id"], 10);

    let api = FeatureApi::new().await;
    write_outbox(
        &cfg.self_improve_dir,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "chat_id": cfg.owner_id.0,
            "requested_by": cfg.owner_id.0, "proposed_by": 2, "goal": "x", "created_at": 1,
        }),
    );
    assert!(
        crate::self_improve::handle_callback(&api.bot, &callback(2, "approve:plan-1"), &cfg).await
    );
    assert!(
        !cfg.self_improve_dir
            .join("plan-approvals")
            .join("plan-1.json")
            .exists(),
        "non-owner must not approve"
    );
    assert_eq!(callback_answers(&api).await, vec!["только владелец решает"]);
    assert!(
        crate::self_improve::handle_callback(
            &api.bot,
            &callback(cfg.owner_id.0, "approve:plan-1"),
            &cfg
        )
        .await
    );
    assert!(
        cfg.self_improve_dir
            .join("plan-approvals")
            .join("plan-1.json")
            .exists(),
        "owner approves"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn owner_inline_buttons_approve_only_the_matching_task() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    write_outbox(
        &cfg.self_improve_dir,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "chat_id": 10, "goal": "x", "created_at": 1,
        }),
    );
    assert!(
        crate::self_improve::handle_callback(
            &api.bot,
            &callback(cfg.owner_id.0, "approve:plan-1"),
            &cfg
        )
        .await
    );
    assert!(cfg
        .self_improve_dir
        .join("plan-approvals")
        .join("plan-1.json")
        .exists());
    assert!(!cfg
        .self_improve_dir
        .join("approvals")
        .join("plan-1.json")
        .exists());
}

#[cfg(unix)]
#[tokio::test]
async fn inline_trash_rejects_the_planned_task() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    write_outbox(
        &cfg.self_improve_dir,
        "plan-2.json",
        json!({
            "id": "plan-2", "status": "planned", "chat_id": 10, "goal": "x", "created_at": 1,
        }),
    );
    assert!(
        crate::self_improve::handle_callback(
            &api.bot,
            &callback(cfg.owner_id.0, "reject:plan-2"),
            &cfg
        )
        .await
    );
    assert!(cfg
        .self_improve_dir
        .join("rejections")
        .join("plan-2.json")
        .exists());
}

#[cfg(unix)]
#[tokio::test]
async fn double_callback_writes_one_marker_and_answers_already_decided() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    write_outbox(
        &cfg.self_improve_dir,
        "plan-3.json",
        json!({
            "id": "plan-3", "status": "proposed", "chat_id": 10, "goal": "x", "created_at": 1,
        }),
    );
    assert!(
        crate::self_improve::handle_callback(
            &api.bot,
            &callback(cfg.owner_id.0, "approve:plan-3"),
            &cfg
        )
        .await
    );
    let marker = cfg.self_improve_dir.join("approvals").join("plan-3.json");
    let first = std::fs::read(&marker).unwrap();
    assert!(
        crate::self_improve::handle_callback(
            &api.bot,
            &callback(cfg.owner_id.0, "reject:plan-3"),
            &cfg
        )
        .await
    );
    assert_eq!(
        std::fs::read(&marker).unwrap(),
        first,
        "the first decision wins"
    );
    assert!(!cfg
        .self_improve_dir
        .join("rejections")
        .join("plan-3.json")
        .exists());
    let answers = callback_answers(&api).await;
    assert_eq!(
        answers.last().map(String::as_str),
        Some("решение уже принято"),
        "{answers:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn non_owner_callback_and_expired_task_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    write_outbox(
        &cfg.self_improve_dir,
        "plan-4.json",
        json!({
            "id": "plan-4", "status": "planned", "chat_id": 10, "goal": "x", "created_at": 1,
        }),
    );
    assert!(
        crate::self_improve::handle_callback(&api.bot, &callback(2, "approve:plan-4"), &cfg).await
    );
    assert!(
        crate::self_improve::handle_callback(
            &api.bot,
            &callback(cfg.owner_id.0, "approve:missing"),
            &cfg
        )
        .await
    );
    assert!(!cfg
        .self_improve_dir
        .join("plan-approvals")
        .join("plan-4.json")
        .exists());
    assert!(!cfg
        .self_improve_dir
        .join("approvals")
        .join("missing.json")
        .exists());
    let answers = callback_answers(&api).await;
    assert_eq!(answers[0], "только владелец решает");
    assert_eq!(answers[1], "задача уже неактуальна");
}

#[cfg(unix)]
#[tokio::test]
async fn status_snapshot_returns_only_whitelisted_fields_and_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "plan-1.json",
        json!({
            "id": "plan-1",
            "status": "planned",
            "requested_by": 1,
            "goal": "г".repeat(3000),
            "summary": "с".repeat(700),
            "spec": "SECRET SPEC",
            "review": "SECRET REVIEW",
            "worktree": "/home/owner/worktree",
            "branch": "secret-branch",
            "base": "deadbeef",
            "log": "/home/owner/log",
            "diff_stat": "1 file changed",
            "tests": "SECRET TESTS",
            "files": (0..30).map(|index| format!("src/{index}.rs")).collect::<Vec<_>>(),
            "issue_url": "https://evil.example/issue",
            "created_at": 1,
        }),
    );
    let value = crate::self_improve::status_snapshot(root, UserId(1), Some("plan-1"));
    let text = value.to_string();
    assert!(text.chars().count() <= 6000, "{}", text.chars().count());
    for secret in [
        "SECRET SPEC",
        "SECRET REVIEW",
        "worktree",
        "secret-branch",
        "deadbeef",
        "diff_stat",
        "SECRET TESTS",
        "evil.example",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let task = &value["tasks"][0];
    assert!(task["goal"].as_str().unwrap().chars().count() <= 2000);
    assert!(task["summary"].as_str().unwrap().chars().count() <= 500);
    assert!(task["files"].as_array().unwrap().len() <= 20);
    assert!(task["issue_url"].is_null());
    assert_eq!(task["status"], "planned");
    assert_eq!(task["decision"], "plan");
}

#[cfg(unix)]
#[tokio::test]
async fn status_snapshot_filters_owner_and_validates_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "foreign.json",
        json!({
            "id": "foreign", "status": "planned", "requested_by": 2, "chat_id": 1, "goal": "x", "created_at": 1,
        }),
    );
    assert_eq!(
        crate::self_improve::status_snapshot(root, UserId(1), Some("foreign"))["error"],
        "не найдено"
    );
    assert_eq!(
        crate::self_improve::status_snapshot(root, UserId(2), Some("foreign"))["tasks"][0]["id"],
        "foreign"
    );

    write_outbox(
        root,
        "mine.json",
        json!({
            "id": "mine", "status": "proposed", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    let long = "a".repeat(81);
    for id in ["../x", "/etc/passwd", long.as_str()] {
        let value = crate::self_improve::status_snapshot(root, UserId(1), Some(id));
        assert_eq!(value["error"], "не найдено", "{id}");
        assert!(value["tasks"].as_array().unwrap().is_empty());
    }
    assert_eq!(
        crate::self_improve::status_snapshot(root, UserId(1), Some("mine"))["tasks"][0]["id"],
        "mine"
    );
}

#[cfg(unix)]
#[test]
fn status_overview_active_carries_already_decided() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "x", "created_at": 1,
        }),
    );
    std::fs::create_dir_all(root.join("plan-approvals")).unwrap();
    std::fs::write(root.join("plan-approvals").join("plan-1.json"), b"{}").unwrap();
    let overview = crate::self_improve::status_overview(root, UserId(1));
    let active = overview["active"].as_array().unwrap();
    assert_eq!(active[0]["already_decided"], "approve");
}

#[cfg(unix)]
#[test]
fn priority_orders_queue_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "low.json",
        json!({
            "id": "low", "status": "planned", "requested_by": 1,
            "goal": "low", "created_at": 2, "priority": 0,
        }),
    );
    write_outbox(
        root,
        "high.json",
        json!({
            "id": "high", "status": "planned", "requested_by": 1,
            "goal": "high", "created_at": 1, "priority": 10,
        }),
    );
    let snapshot = crate::self_improve::status_snapshot(root, UserId(1), None);
    let tasks = snapshot["tasks"].as_array().unwrap();
    assert_eq!(tasks[0]["id"], "high");
    assert_eq!(tasks[0]["priority"], 10);
}

#[cfg(unix)]
#[tokio::test]
async fn status_overview_reports_failed_and_active_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (id, status, created) in [
        ("deployed", "deployed", 1),
        ("failed", "failed", 2),
        ("rejected", "rejected", 3),
        ("active", "proposed", 4),
    ] {
        write_outbox(
            root,
            &format!("{id}.json"),
            json!({
                "id": id, "status": status, "requested_by": 1,
                "goal": format!("goal {id}"), "created_at": created,
            }),
        );
    }
    let overview = crate::self_improve::status_overview(root, UserId(1));
    assert_eq!(overview["total"], 4);
    assert_eq!(overview["counts"]["deployed"], 1);
    assert_eq!(overview["counts"]["failed"], 1);
    assert_eq!(overview["counts"]["rejected"], 1);
    assert_eq!(overview["counts"]["proposed"], 1);
    let active = overview["active"].as_array().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["id"], "active");
    assert_eq!(active[0]["status"], "proposed");

    write_inbox(
        root,
        "queued.json",
        json!({
            "id": "queued", "requested_by": 1, "goal": "queued goal", "created_at": 5,
        }),
    );
    write_outbox(
        root,
        "legacy-rejected.json",
        json!({
            "id": "legacy-rejected", "status": "rejected", "goal": "old rejected",
            "chat_id": 0, "source_chat_id": 0, "proposed_by": 0, "created_at": 6,
        }),
    );
    let overview = crate::self_improve::status_overview(root, UserId(1));
    let active = overview["active"].as_array().unwrap();
    assert!(active
        .iter()
        .any(|task| task["id"] == "queued" && task["status"] == "queued"));
    assert_eq!(overview["counts"]["queued"], 1);
    assert_eq!(overview["counts"]["rejected"], 2);
}

#[cfg(unix)]
#[tokio::test]
async fn status_snapshot_for_non_owner_shows_only_own_requests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "owner.json",
        json!({
            "id": "owner", "status": "deployed", "requested_by": 1,
            "proposed_by": 1, "goal": "owner goal", "created_at": 1,
        }),
    );
    write_outbox(
        root,
        "mine.json",
        json!({
            "id": "mine", "status": "planning", "requested_by": 1,
            "proposed_by": 2, "goal": "my goal", "created_at": 2,
        }),
    );
    let mine = crate::self_improve::status_snapshot_for(root, UserId(1), UserId(2), None);
    assert_eq!(mine["scope"], "your_requests");
    let tasks = mine["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], "mine");
    assert!(!mine.to_string().contains("owner goal"));

    let owner = crate::self_improve::status_snapshot_for(root, UserId(1), UserId(1), None);
    assert_eq!(owner["tasks"].as_array().unwrap().len(), 2);

    let foreign =
        crate::self_improve::status_snapshot_for(root, UserId(1), UserId(2), Some("owner"));
    assert_eq!(foreign["error"], "не найдено");
    let own = crate::self_improve::status_snapshot_for(root, UserId(1), UserId(2), Some("mine"));
    assert_eq!(own["tasks"][0]["id"], "mine");
}

#[cfg(unix)]
#[tokio::test]
async fn status_snapshot_for_non_owner_without_requests_reports_no_owner_access() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "owner.json",
        json!({
            "id": "owner", "status": "deployed", "requested_by": 1,
            "proposed_by": 1, "goal": "owner goal", "created_at": 1,
        }),
    );
    let value = crate::self_improve::status_snapshot_for(root, UserId(1), UserId(2), None);
    assert_eq!(value["scope"], "your_requests");
    assert!(value["tasks"].as_array().unwrap().is_empty());
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("очередь владельца недоступна"));
    assert!(!value.to_string().contains("owner goal"));
}

#[cfg(unix)]
#[tokio::test]
async fn status_snapshot_covers_queued_planned_proposed_failed_deployed_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let table = [
        ("planning", Value::Null),
        ("planned", json!("plan")),
        ("running", Value::Null),
        ("proposed", json!("deploy")),
        ("deployed", Value::Null),
        ("failed", Value::Null),
        ("rejected", Value::Null),
    ];
    for (index, (status, decision)) in table.into_iter().enumerate() {
        write_outbox(
            root,
            &format!("{status}.json"),
            json!({
                "id": status, "status": status, "requested_by": 1, "goal": "x", "created_at": index as i64 + 1,
            }),
        );
        let value = crate::self_improve::status_snapshot(root, UserId(1), Some(status));
        let task = &value["tasks"][0];
        assert_eq!(task["status"], status);
        assert_eq!(task["decision"], decision, "{status}");
        assert!(task["already_decided"].is_null());
    }
    write_inbox(
        root,
        "queued.json",
        json!({
            "id": "queued", "requested_by": 1, "goal": "x", "created_at": 100,
        }),
    );
    let value = crate::self_improve::status_snapshot(root, UserId(1), Some("queued"));
    let queued = &value["tasks"][0];
    assert_eq!(queued["status"], "queued");
    assert!(queued["decision"].is_null());
}

#[cfg(unix)]
#[tokio::test]
async fn status_snapshot_and_confirmation_do_not_mutate_queue() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    write_inbox(
        root,
        "plan-2.json",
        json!({
            "id": "plan-2", "requested_by": 1, "goal": "y", "created_at": 2,
        }),
    );
    std::fs::create_dir_all(root.join("rejections")).unwrap();
    std::fs::write(root.join("rejections").join("old.json"), b"reject").unwrap();
    let before = tree(root);
    let _ = crate::self_improve::status_snapshot(root, UserId(1), None);
    let _ = crate::self_improve::status_snapshot(root, UserId(1), Some("plan-1"));
    let _ = crate::self_improve::pending_confirmation(root, UserId(1));
    assert_eq!(tree(root), before);
}

#[cfg(unix)]
#[test]
fn plan_approval_does_not_block_deploy_approval() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "proposed", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    std::fs::create_dir_all(root.join("plan-approvals")).unwrap();
    std::fs::write(root.join("plan-approvals").join("plan-1.json"), b"{}").unwrap();
    let value = crate::self_improve::pending_confirmation(root, UserId(1));
    assert!(value["tasks"][0]["already_decided"].is_null());
    assert_eq!(value["tasks"][0]["decision"], "deploy");
    assert_eq!(
        crate::self_improve::newest_pending_decision(&value).map(|pending| pending.gate),
        Some(crate::self_improve::DecisionGate::Deploy),
    );

    std::fs::create_dir_all(root.join("approvals")).unwrap();
    std::fs::write(root.join("approvals").join("plan-1.json"), b"{}").unwrap();
    let value = crate::self_improve::pending_confirmation(root, UserId(1));
    assert_eq!(value["tasks"][0]["already_decided"], "approve");
    assert!(crate::self_improve::newest_pending_decision(&value).is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn confirmation_reports_existing_marker_as_already_decided() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    std::fs::create_dir_all(root.join("plan-approvals")).unwrap();
    std::fs::write(root.join("plan-approvals").join("plan-1.json"), b"{}").unwrap();
    let value = crate::self_improve::pending_confirmation(root, UserId(1));
    assert_eq!(value["tasks"][0]["already_decided"], "approve");
    assert_eq!(value["tasks"][0]["decision"], "plan");
    assert!(
        crate::self_improve::newest_pending_decision(&value).is_none(),
        "decided tasks get no buttons"
    );

    std::fs::create_dir_all(root.join("rejections")).unwrap();
    std::fs::write(root.join("rejections").join("plan-1.json"), b"reject").unwrap();
    let value = crate::self_improve::pending_confirmation(root, UserId(1));
    assert_eq!(value["tasks"][0]["already_decided"], "reject");
}

#[cfg(unix)]
#[tokio::test]
async fn propose_feature_refines_links_parent_without_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    write_outbox(
        &cfg.self_improve_dir,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": cfg.owner_id.0,
            "goal": "сделай кнопку", "spec": "старая спека", "review": "старое ревью", "created_at": 1,
        }),
    );
    let id = crate::self_improve::queue_feature_refined(
        &cfg.self_improve_dir,
        cfg.owner_id,
        10,
        cfg.owner_id,
        "Owner",
        77,
        "добавь кнопку отмены",
        Some("plan-1"),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(
        &std::fs::read(
            cfg.self_improve_dir
                .join("inbox")
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(value["parent"], "plan-1");
    assert_eq!(value["feedback"], "добавь кнопку отмены");
    assert_eq!(value["previous_spec"], "старая спека");
    assert_eq!(value["previous_review"], "старое ревью");
    assert!(!cfg
        .self_improve_dir
        .join("rejections")
        .join("plan-1.json")
        .exists());
    assert!(!cfg
        .self_improve_dir
        .join("plan-approvals")
        .join("plan-1.json")
        .exists());
}

#[cfg(unix)]
#[tokio::test]
async fn propose_feature_refines_rejects_non_owner_and_invalid_parent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    write_outbox(
        &cfg.self_improve_dir,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": cfg.owner_id.0,
            "goal": "x", "created_at": 1,
        }),
    );
    write_outbox(
        &cfg.self_improve_dir,
        "done.json",
        json!({
            "id": "done", "status": "deployed", "requested_by": cfg.owner_id.0,
            "goal": "x", "created_at": 2,
        }),
    );
    let non_owner = crate::self_improve::queue_feature_refined(
        &cfg.self_improve_dir,
        cfg.owner_id,
        10,
        UserId(2),
        "tester",
        77,
        "поправь",
        Some("plan-1"),
    );
    assert_eq!(
        non_owner.unwrap_err(),
        "дорабатывать план может только владелец"
    );
    for parent in ["missing", "../x", "done"] {
        let result = crate::self_improve::queue_feature_refined(
            &cfg.self_improve_dir,
            cfg.owner_id,
            10,
            cfg.owner_id,
            "Owner",
            77,
            "поправь",
            Some(parent),
        );
        assert!(result.is_err(), "{parent}");
    }
    assert!(!cfg.self_improve_dir.join("inbox").exists());
}

const RUNNING_PLAN: &str = "plan-1793269960-1002045081420-214534";
const OTHER_PLAN: &str = "plan-1793269900-1002045081420-214341";

#[cfg(unix)]
#[tokio::test]
async fn displayed_short_ids_resolve_to_the_same_task() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        &format!("{RUNNING_PLAN}.json"),
        json!({
            "id": RUNNING_PLAN, "status": "running", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    for token in ["214534", "#214534", "34", RUNNING_PLAN] {
        let value = crate::self_improve::status_snapshot(root, UserId(1), Some(token));
        assert_eq!(value["tasks"][0]["id"], RUNNING_PLAN, "{token}: {value}");
        assert!(value["error"].is_null(), "{token}: {value}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn status_command_accepts_the_displayed_short_id() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    write_outbox(
        &cfg.self_improve_dir,
        &format!("{RUNNING_PLAN}.json"),
        json!({
            "id": RUNNING_PLAN, "status": "running", "requested_by": cfg.owner_id.0,
            "goal": "делается", "created_at": 1,
        }),
    );
    let text = "/improve status #34";
    assert!(
        crate::self_improve::handle_improve(
            &api.bot,
            &message(10, "private", cfg.owner_id.0, text),
            &cfg,
            text,
        )
        .await
    );
    let replies: Vec<String> = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .filter_map(|request| {
            request
                .fields
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(replies[0].contains(RUNNING_PLAN), "{replies:?}");
    assert!(replies[0].contains("running"), "{replies:?}");
    assert!(!replies[0].contains("не найдена"), "{replies:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn exact_displayed_id_wins_over_a_longer_partial_match() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let longer = "plan-1793269900-1002045081420-1214534";
    write_outbox(
        root,
        &format!("{RUNNING_PLAN}.json"),
        json!({
            "id": RUNNING_PLAN, "status": "running", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    write_outbox(
        root,
        &format!("{longer}.json"),
        json!({
            "id": longer, "status": "planned", "requested_by": 1, "goal": "y", "created_at": 2,
        }),
    );
    let value = crate::self_improve::status_snapshot(root, UserId(1), Some("214534"));
    assert_eq!(value["tasks"][0]["id"], RUNNING_PLAN, "{value}");
}

#[cfg(unix)]
#[tokio::test]
async fn ambiguous_partial_id_is_reported_instead_of_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        &format!("{RUNNING_PLAN}.json"),
        json!({
            "id": RUNNING_PLAN, "status": "running", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    write_outbox(
        root,
        &format!("{OTHER_PLAN}.json"),
        json!({
            "id": OTHER_PLAN, "status": "planned", "requested_by": 1, "goal": "y", "created_at": 2,
        }),
    );
    let value = crate::self_improve::status_snapshot(root, UserId(1), Some("214"));
    assert!(value["tasks"].as_array().unwrap().is_empty(), "{value}");
    let error = value["error"].as_str().unwrap();
    assert!(error.contains("неоднозначно"), "{error}");
    assert!(
        error.contains("#214534") && error.contains("#214341"),
        "{error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn partial_id_lookup_never_leaks_the_owner_queue() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (id, created) in [(RUNNING_PLAN, 1), (OTHER_PLAN, 2)] {
        write_outbox(
            root,
            &format!("{id}.json"),
            json!({
                "id": id, "status": "running", "requested_by": 1,
                "proposed_by": 1, "goal": "owner goal", "created_at": created,
            }),
        );
    }
    let value = crate::self_improve::status_snapshot_for(root, UserId(1), UserId(2), Some("214"));
    assert_eq!(value["error"], "не найдено", "{value}");
    assert_eq!(value["scope"], "your_requests");
    assert!(!value.to_string().contains("owner goal"));
}

#[cfg(unix)]
#[tokio::test]
async fn every_listed_queue_task_is_found_by_its_displayed_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_outbox(
        root,
        &format!("{RUNNING_PLAN}.json"),
        json!({
            "id": RUNNING_PLAN, "status": "running", "requested_by": 1, "goal": "делается", "created_at": 2,
        }),
    );
    write_inbox(
        root,
        "plan-1793269800-1002045081420-214541.json",
        json!({
            "id": "plan-1793269800-1002045081420-214541", "requested_by": 1,
            "goal": "в очереди", "created_at": 1,
        }),
    );
    let overview = crate::self_improve::status_overview(root, UserId(1));
    let active = overview["active"].as_array().unwrap();
    assert_eq!(active.len(), 2, "{overview}");
    for task in active {
        let full = task["id"].as_str().unwrap();
        let shown = crate::self_improve::short_task_id(full);
        let value = crate::self_improve::status_snapshot(root, UserId(1), Some(shown));
        assert_eq!(value["tasks"][0]["id"], full, "{shown}: {value}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cancel_command_removes_every_listed_task_in_one_reply() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    let planned_a = "plan-1793269960-1002045081420-214937";
    let planned_b = "plan-1793269940-1002045081420-214786";
    let queued = "plan-1793269920-1002045081420-214534";
    let running = "plan-1793269900-1002045081420-214541";
    for (id, created) in [(planned_a, 1), (planned_b, 2)] {
        write_outbox(
            &cfg.self_improve_dir,
            &format!("{id}.json"),
            json!({
                "id": id, "status": "planned", "requested_by": cfg.owner_id.0,
                "goal": "x", "created_at": created,
            }),
        );
    }
    write_outbox(
        &cfg.self_improve_dir,
        &format!("{running}.json"),
        json!({
            "id": running, "status": "running", "requested_by": cfg.owner_id.0,
            "goal": "y", "created_at": 3,
        }),
    );
    write_inbox(
        &cfg.self_improve_dir,
        &format!("{queued}.json"),
        json!({
            "id": queued, "requested_by": cfg.owner_id.0, "goal": "z", "created_at": 4,
        }),
    );
    let text = "отмени #214937 #214786 #214534 #214541";
    assert!(
        crate::self_improve::handle_owner_text(
            &api.bot,
            &message(10, "private", cfg.owner_id.0, text),
            &cfg,
            text,
        )
        .await
    );
    for id in [planned_a, planned_b] {
        assert!(
            cfg.self_improve_dir
                .join("rejections")
                .join(format!("{id}.json"))
                .exists(),
            "{id}"
        );
    }
    assert!(!cfg
        .self_improve_dir
        .join("inbox")
        .join(format!("{queued}.json"))
        .exists());
    assert!(
        cfg.self_improve_dir
            .join("outbox")
            .join(format!("{running}.json"))
            .exists(),
        "running task must survive"
    );
    let replies: Vec<String> = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .filter_map(|request| {
            request
                .fields
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(replies[0].contains("Отменил 3"), "{replies:?}");
    for shown in ["#214937", "#214786", "#214534"] {
        assert!(replies[0].contains(shown), "{shown}: {replies:?}");
    }
    assert!(
        replies[0].contains("#214541 — уже выполняется"),
        "{replies:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancel_command_reports_unknown_ids_without_stopping() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    let api = FeatureApi::new().await;
    let planned = "plan-1793269960-1002045081420-214937";
    write_outbox(
        &cfg.self_improve_dir,
        &format!("{planned}.json"),
        json!({
            "id": planned, "status": "planned", "requested_by": cfg.owner_id.0,
            "goal": "x", "created_at": 1,
        }),
    );
    let text = "отмени #111111 #214937";
    assert!(
        crate::self_improve::handle_owner_text(
            &api.bot,
            &message(10, "private", cfg.owner_id.0, text),
            &cfg,
            text,
        )
        .await
    );
    assert!(cfg
        .self_improve_dir
        .join("rejections")
        .join(format!("{planned}.json"))
        .exists());
    let replies: Vec<String> = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .filter_map(|request| {
            request
                .fields
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(replies[0].contains("Не нашёл: #111111"), "{replies:?}");
    assert!(replies[0].contains("Отменил 1: #214937"), "{replies:?}");

    let text = "отмени #214937";
    assert!(
        !crate::self_improve::handle_owner_text(
            &api.bot,
            &message(10, "private", 2, text),
            &cfg,
            text,
        )
        .await,
        "only the owner cancels"
    );
    let replies = api
        .snapshot()
        .await
        .into_iter()
        .filter(|request| request.method == "sendmessage")
        .count();
    assert_eq!(replies, 1, "a non-owner cancel sends nothing");
}

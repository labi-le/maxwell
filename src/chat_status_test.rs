use crate::test_support::*;
use crate::test_support::{chat_reply, chat_reply_with_proposal};
use crate::*;
use serde_json::json;

#[tokio::test]
async fn chat_reply_queues_a_feature_through_the_tool() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"propose_feature","arguments":"{\"goal\":\"разреши ходить по сайтам\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"записал"}}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: dir.path().join("self-improve"),
        owner_id: UserId(2),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        5,
        true,
        "разреши себе ходить по сайтам",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "записал");
    let files: Vec<_> = std::fs::read_dir(proposal.dir.join("inbox"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["goal"], "разреши ходить по сайтам");
    assert_eq!(value["proposed_by"], 2);
    llm.task.abort();
}

fn marker_exists(root: &Path, dir: &str, id: &str) -> bool {
    root.join(dir).join(format!("{id}.json")).exists()
}

const REFINES_PARENT: &str = "plan-1793270374-1002045081420-214537";

fn planned_parent(root: &Path) {
    write_outbox(
        root,
        &format!("{REFINES_PARENT}.json"),
        json!({
            "id": REFINES_PARENT,
            "status": "planned",
            "requested_by": 2,
            "goal": "сделай кнопку",
            "spec": "старая спека",
            "review": "старое ревью",
            "created_at": 1,
        }),
    );
}

fn queued_children(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join("inbox"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect()
}

#[tokio::test]
async fn propose_feature_tool_passes_refines_to_queue() {
    let (_dir, pool) = test_pool().await;
    let root = tempfile::tempdir().unwrap();
    let proposal = FeatureProposal {
        dir: root.path().join("self-improve"),
        owner_id: UserId(2),
    };
    planned_parent(&proposal.dir);
    let arguments = json!({"goal": "добавь кнопку отмены", "refines": "#214537"});
    let llm = mock_server(vec![
        tool_response("propose_feature", arguments),
        reply_response("переделал"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(2),
        UserId(2),
        "owner",
        5,
        true,
        "переделай план: добавь кнопку отмены",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "переделал");
    let children = queued_children(&proposal.dir);
    assert_eq!(children.len(), 1, "{children:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&children[0]).unwrap()).unwrap();
    assert_eq!(value["parent"], REFINES_PARENT);
    assert_eq!(value["previous_spec"], "старая спека");
    assert_eq!(value["feedback"], "добавь кнопку отмены");
    assert_eq!(value["goal"], "сделай кнопку");
    llm.task.abort();
}

#[tokio::test]
async fn propose_feature_tool_rejects_non_owner_refines() {
    let (_dir, pool) = test_pool().await;
    let root = tempfile::tempdir().unwrap();
    let proposal = FeatureProposal {
        dir: root.path().join("self-improve"),
        owner_id: UserId(2),
    };
    planned_parent(&proposal.dir);
    let arguments = json!({"goal": "перепиши спеку", "refines": REFINES_PARENT});
    let llm = mock_server(vec![
        tool_response("propose_feature", arguments),
        reply_response("нельзя"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let _ = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(2),
        UserId(3),
        "guest",
        6,
        false,
        "перепиши спеку плана",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(
        result.contains("дорабатывать план может только владелец"),
        "{result}"
    );
    drop(requests);
    assert!(queued_children(&proposal.dir).is_empty());
    llm.task.abort();
}

#[tokio::test]
async fn owner_gets_feature_status_and_confirmation_tools_non_owner_gets_scoped_status_only() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        reply_response("ок"),
        tool_response("feature_status", serde_json::json!({})),
        reply_response("нет доступа"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: dir.path().join("self-improve"),
        owner_id: UserId(1),
    };
    let owner_reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что там по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(owner_reply.text, "ок");
    let non_owner_reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(2),
        "tester",
        6,
        false,
        "что там по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(non_owner_reply.text, "нет доступа");

    let requests = llm.requests.lock().unwrap();
    let owner_tools = body_of(&requests[0])["tools"].as_array().unwrap().clone();
    for name in ["feature_status", "feature_confirmation"] {
        assert!(
            owner_tools
                .iter()
                .any(|tool| tool["function"]["name"] == name),
            "{name}"
        );
    }
    let non_owner = body_of(&requests[1]);
    let non_owner_tools = non_owner["tools"].as_array().unwrap();
    assert!(non_owner_tools
        .iter()
        .any(|tool| tool["function"]["name"] == "feature_status"));
    assert!(!non_owner_tools
        .iter()
        .any(|tool| tool["function"]["name"] == "feature_confirmation"));
    let non_owner_prompt = non_owner["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(non_owner_prompt.contains("feature_status"));
    assert!(!non_owner_prompt.contains("feature_confirmation"));
    let tool_result = tool_contents(&requests[2]).join("\n");
    assert!(tool_result.contains("your_requests"), "{tool_result}");
    assert!(
        tool_result.contains("очередь владельца недоступна"),
        "{tool_result}"
    );
    assert!(
        !tool_result.contains("доступно только владельцу"),
        "{tool_result}"
    );
    llm.task.abort();
}

#[tokio::test]
async fn feature_tools_require_a_queue_context() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![reply_response("ок")]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "статус?",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "ок");
    let requests = llm.requests.lock().unwrap();
    let tools = body_of(&requests[0])["tools"].as_array().unwrap().clone();
    assert!(tools
        .iter()
        .any(|tool| tool["function"]["name"] == "web_search"));
    for name in ["feature_status", "feature_confirmation"] {
        assert!(
            !tools.iter().any(|tool| tool["function"]["name"] == name),
            "{name}"
        );
    }
    llm.task.abort();
}

#[tokio::test]
async fn owner_prompt_contains_live_queue_facts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "deployed.json",
        json!({
            "id": "deployed", "status": "deployed", "requested_by": 1,
            "goal": "done", "created_at": 1,
        }),
    );
    write_outbox(
        &root,
        "failed.json",
        json!({
            "id": "failed", "status": "failed", "requested_by": 1,
            "goal": "broken", "created_at": 2,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![reply_response("ок")]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что в очереди?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "ок");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("\"deployed\":1"), "{system}");
    assert!(system.contains("\"failed\":1"), "{system}");
    assert!(system.contains("Не утверждай"), "{system}");
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_tool_call_returns_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1, "goal": "сделай кнопку",
            "summary": "готово", "issue_url": "https://github.com/labi-le/maxwell/issues/1",
            "spec": "SECRET SPEC", "review": "SECRET REVIEW", "worktree": "/host/path",
            "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"id": "plan-1"})),
        reply_response("остальное уже задеплоено"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root.clone(),
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(
        reply.text,
        "#plan-1: план готов: сделай кнопку\nhttps://github.com/labi-le/maxwell/issues/1"
    );
    assert!(
        !reply.text.contains("остальное"),
        "false model answer leaked: {}",
        reply.text
    );
    let requests = llm.requests.lock().unwrap();
    let first_body = body_of(&requests[0]);
    let user = first_body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(user.contains("что по заявке?"), "{user}");
    let result = tool_contents(&requests[1]).join("\n");
    assert!(result.contains("plan-1"), "{result}");
    assert!(result.contains("planned"), "{result}");
    assert!(
        result.contains("https://github.com/labi-le/maxwell/issues/1"),
        "{result}"
    );
    for secret in ["SECRET SPEC", "SECRET REVIEW", "/host/path"] {
        assert!(!result.contains(secret), "{secret} leaked: {result}");
    }
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_rejects_a_false_status_claim() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"id": "plan-1"})),
        reply_response("#plan-1 задеплоена"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "#plan-1: план готов: сделай кнопку");
    assert!(!reply.text.contains("задеплоена"), "{}", reply.text);
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_repairs_a_false_status_claim() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"id": "plan-1"})),
        reply_response("#plan-1 задеплоена"),
        reply_response("#plan-1 — план готов, ждёт реализации"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "#plan-1 — план готов, ждёт реализации");
    assert!(!reply.text.contains("задеплоена"), "{}", reply.text);
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_keeps_a_grounded_model_answer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"id": "plan-1"})),
        reply_response("#plan-1 — план готов, ждёт реализации"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "#plan-1 — план готов, ждёт реализации");
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_redacts_a_secret_in_an_accepted_answer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"id": "plan-1"})),
        reply_response("#plan-1 — план готов, super-secret-key"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "super-secret-key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert!(reply.text.contains("#plan-1"), "{}", reply.text);
    assert!(!reply.text.contains("super-secret-key"), "{}", reply.text);
    assert!(reply.text.contains("[redacted]"), "{}", reply.text);
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_allows_a_clarifying_question() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({})),
        reply_response("какую задачу показать?"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что там?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "какую задачу показать?");
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_rejects_an_unknown_task_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({})),
        reply_response("#999 — план готов"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что по заявке?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert!(!reply.text.contains("#999"), "{}", reply.text);
    assert!(reply.text.contains("#plan-1"), "{}", reply.text);
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_rejects_a_fake_issue_url() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
            "issue_url": "https://github.com/labi-le/maxwell/issues/1",
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"view": "links"})),
        reply_response("https://github.com/labi-le/maxwell/issues/999"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "скинь ссылку",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert!(!reply.text.contains("issues/999"), "{}", reply.text);
    assert!(reply.text.contains("issues/1"), "{}", reply.text);
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_allows_a_status_question_without_an_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({})),
        reply_response("какая задача провалена?"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что там?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "какая задача провалена?");
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_rejects_a_false_empty_claim() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "сделай кнопку", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({})),
        reply_response("очередь пуста"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что там?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert!(!reply.text.contains("пуста"), "{}", reply.text);
    assert!(reply.text.contains("#plan-1"), "{}", reply.text);
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_payload_contains_all_queue_states() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    for (id, status, created) in [
        ("deployed", "deployed", 1),
        ("failed", "failed", 2),
        ("rejected", "rejected", 3),
        ("active", "proposed", 4),
    ] {
        write_outbox(
            &root,
            &format!("{id}.json"),
            json!({
                "id": id, "status": status, "requested_by": 1,
                "goal": format!("goal {id}"), "created_at": created,
            }),
        );
    }
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"view": "summary"})),
        reply_response("остальное уже задеплоено"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что в очереди?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert!(
        reply.text.contains("#active — ждёт деплоя"),
        "{}",
        reply.text
    );
    assert!(reply.text.contains("Провалено: 1"), "{}", reply.text);
    assert!(reply.text.contains("Отклонено: 1"), "{}", reply.text);
    assert!(!reply.text.contains("остальное"), "{}", reply.text);
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    for state in ["deployed", "failed", "rejected", "proposed"] {
        assert!(result.contains(state), "{state} missing: {result}");
    }
    llm.task.abort();
}

#[tokio::test]
async fn non_owner_status_answer_includes_queued_request() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_inbox(
        &root,
        "queued.json",
        json!({
            "id": "queued", "requested_by": 1, "proposed_by": 2,
            "goal": "my queued request", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({})),
        reply_response("остальное уже задеплоено"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(2),
        "tester",
        5,
        false,
        "что с моей заявкой?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "#queued: в очереди: my queued request");
    assert!(!reply.text.contains("остальное"), "{}", reply.text);
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(result.contains("queued"), "{result}");
    assert!(result.contains("my queued request"), "{result}");
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_reports_recorded_plan_approval() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "x", "created_at": 1,
        }),
    );
    std::fs::create_dir_all(root.join("plan-approvals")).unwrap();
    std::fs::write(root.join("plan-approvals").join("plan-1.json"), b"{}").unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"id": "plan-1"})),
        reply_response("план уже одобрен, ждёт реализации"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "как одобрить?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "план уже одобрен, ждёт реализации");
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(result.contains("already_decided"), "{result}");
    llm.task.abort();
}

#[tokio::test]
async fn feature_confirmation_reports_recorded_plan_approval() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1,
            "goal": "x", "created_at": 1,
        }),
    );
    std::fs::create_dir_all(root.join("plan-approvals")).unwrap();
    std::fs::write(root.join("plan-approvals").join("plan-1.json"), b"{}").unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_confirmation", json!({})),
        reply_response("план уже одобрен, ждёт реализации"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "делаем?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "план уже одобрен, ждёт реализации");
    assert!(reply.decision.is_none());
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(result.contains("already_decided"), "{result}");
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_returns_issue_links_for_ready_plans() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    let id = "plan-1793270374-1002045081420-214537";
    write_outbox(
        &root,
        &format!("{id}.json"),
        json!({
            "id": id, "status": "planned", "requested_by": 1,
            "goal": "threads links", "created_at": 1,
            "issue_url": "https://github.com/labi-le/maxwell/issues/7",
        }),
    );
    write_inbox(
        &root,
        "plan-queued.json",
        json!({
            "id": "plan-queued", "requested_by": 1, "goal": "later", "created_at": 2,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"view": "links"})),
        reply_response("остальное уже задеплоено"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "скинь ссылки на ишью планов которые готовы",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "Ссылки на issue:\n\n#214537 — план готов: threads links\nhttps://github.com/labi-le/maxwell/issues/7");
    assert!(!reply.text.contains("остальное"), "{}", reply.text);
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(
        result.contains("https://github.com/labi-le/maxwell/issues/7"),
        "{result}"
    );
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_payload_keeps_full_task_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    for (id, status, created) in [
        ("plan-1793270374-1002045081420-214537", "planned", 1),
        ("plan-1793269960-1002045081420-214534", "planned", 2),
    ] {
        write_outbox(
            &root,
            &format!("{id}.json"),
            json!({
                "id": id, "status": status, "requested_by": 1,
                "goal": "x", "created_at": created,
            }),
        );
    }
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({})),
        reply_response("план готов"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root,
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что в очереди?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(
        reply.text,
        "Очередь\n• #214534 — план готов: x\n• #214537 — план готов: x"
    );
    assert!(!reply.text.contains("plan-1793270374"), "{}", reply.text);
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(
        result.contains("plan-1793270374-1002045081420-214537"),
        "{result}"
    );
    assert!(
        result.contains("plan-1793269960-1002045081420-214534"),
        "{result}"
    );
    llm.task.abort();
}

#[tokio::test]
async fn inspect_code_reads_the_mounted_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let code = dir.path().join("code");
    std::fs::create_dir_all(code.join("src")).unwrap();
    std::fs::write(code.join("src/chat.rs"), "fn inspect_needle() {}\n").unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("inspect_code", json!({"query": "inspect_needle"})),
        reply_response("нашёл"),
    ])
    .await;
    let mut config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    config.code_dir = code;
    let proposal = FeatureProposal {
        dir: dir.path().join("self-improve"),
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "посмотри код",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "нашёл");
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(result.contains("src/chat.rs:1"), "{result}");
    assert!(result.contains("inspect_needle"), "{result}");
    drop(requests);
    llm.task.abort();
}

#[tokio::test]
async fn feature_status_redacts_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "failed", "requested_by": 1,
            "goal": "ключ super-secret-key в цели", "summary": "и super-secret-key в summary",
            "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_status", json!({"id": "plan-1"})),
        reply_response("не получилось"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "super-secret-key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root.clone(),
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "статус",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert!(!reply.text.contains("super-secret-key"), "{}", reply.text);
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(!result.contains("super-secret-key"), "{result}");
    assert!(result.contains("[redacted]"), "{result}");
    llm.task.abort();
}

#[tokio::test]
async fn feature_confirmation_attaches_validated_buttons() {
    for (status, gate, approve_label) in [
        ("planned", crate::self_improve::DecisionGate::Plan, "Делаем"),
        (
            "proposed",
            crate::self_improve::DecisionGate::Deploy,
            "Деплой",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("self-improve");
        write_outbox(
            &root,
            "plan-1.json",
            json!({
                "id": "plan-1", "status": status, "requested_by": 1, "goal": "x", "created_at": 1,
            }),
        );
        let pool = open_database(&format!(
            "sqlite://{}",
            dir.path().join("state.db").display()
        ))
        .await
        .unwrap();
        let llm = mock_server(vec![
            tool_response("feature_confirmation", json!({})),
            reply_response("жми кнопку"),
        ])
        .await;
        let config = ChatConfig::new(
            llm.url.clone(),
            "key".into(),
            "deepseek-v4.1-flash".into(),
            llm.url.clone(),
            SearchProvider::DuckDuckGo,
            None,
        );
        let proposal = FeatureProposal {
            dir: root.clone(),
            owner_id: UserId(1),
        };
        let reply = chat_reply_with_proposal!(
            &config,
            &pool,
            &StickerReplies::for_tests(),
            &StickerCatalog::default(),
            ChatId(10),
            UserId(1),
            "tester",
            5,
            true,
            "делаем?",
            None,
            Some(&proposal),
        )
        .await
        .unwrap();
        let decision = reply.decision.expect("decision");
        assert_eq!(decision.id, "plan-1");
        assert_eq!(decision.gate, gate);
        let markup = crate::self_improve::decision_markup(&decision).unwrap();
        let labels = serde_json::to_string(&markup).unwrap();
        assert!(labels.contains(approve_label), "{labels}");
        assert!(labels.contains("В помойку"), "{labels}");
        assert!(labels.contains("approve:plan-1"), "{labels}");
        assert!(labels.contains("reject:plan-1"), "{labels}");
        assert!(!marker_exists(&root, "plan-approvals", "plan-1"));
        assert!(!marker_exists(&root, "approvals", "plan-1"));
        assert!(!marker_exists(&root, "rejections", "plan-1"));
        llm.task.abort();
    }
    let invalid = crate::self_improve::PendingDecision {
        id: "bad/id".into(),
        gate: crate::self_improve::DecisionGate::Plan,
    };
    assert!(crate::self_improve::decision_markup(&invalid).is_none());
    let long = crate::self_improve::PendingDecision {
        id: "a".repeat(80),
        gate: crate::self_improve::DecisionGate::Deploy,
    };
    assert!(crate::self_improve::decision_markup(&long).is_none());
}

#[tokio::test]
async fn feature_confirmation_multiple_tasks_buttons_newest() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "old.json",
        json!({
            "id": "old", "status": "planned", "requested_by": 1, "goal": "old", "created_at": 1,
        }),
    );
    write_outbox(
        &root,
        "new.json",
        json!({
            "id": "new", "status": "planned", "requested_by": 1, "goal": "new", "created_at": 5,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("feature_confirmation", json!({})),
        reply_response("две задачи"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root.clone(),
        owner_id: UserId(1),
    };
    let reply = chat_reply_with_proposal!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(1),
        "tester",
        5,
        true,
        "что ждёт?",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(
        reply.decision.as_ref().map(|decision| decision.id.as_str()),
        Some("new")
    );
    let requests = llm.requests.lock().unwrap();
    let result = tool_contents(&requests[1]).join("\n");
    assert!(result.contains("\"id\":\"old\""), "{result}");
    assert!(result.contains("\"id\":\"new\""), "{result}");
    llm.task.abort();
}

#[tokio::test]
async fn paraphrases_are_forwarded_verbatim() {
    let phrases = [
        "что по заявке?",
        "ну как?",
        "план готов?",
        "когда деплой?",
        "why so slow?",
        "напомни, что там",
        "что ждёт подтверждения?",
        "мне надо что-то нажать?",
        "я уже одобрил?",
        "как движ?",
        "why is it taking so long?",
    ];
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planning", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let mut responses = Vec::new();
    for (index, _) in phrases.iter().enumerate() {
        responses.push(tool_response("feature_status", json!({})));
        responses.push(reply_response(&format!("статус {index}")));
    }
    let llm = mock_server(responses).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root.clone(),
        owner_id: UserId(1),
    };
    for phrase in phrases {
        let reply = chat_reply_with_proposal!(
            &config,
            &pool,
            &StickerReplies::for_tests(),
            &StickerCatalog::default(),
            ChatId(10),
            UserId(1),
            "tester",
            5,
            true,
            phrase,
            None,
            Some(&proposal),
        )
        .await
        .unwrap();
        assert!(
            reply.decision.is_none(),
            "status questions must not attach decision buttons"
        );
    }
    let requests = llm.requests.lock().unwrap();
    assert_eq!(requests.len(), phrases.len() * 2);
    for (index, phrase) in phrases.iter().enumerate() {
        let body = body_of(&requests[index * 2]);
        let user = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(user.contains(phrase), "{phrase} was not forwarded: {user}");
    }
    for name in ["plan-approvals", "approvals", "rejections"] {
        assert!(!marker_exists(&root, name, "plan-1"), "{name}");
    }
    llm.task.abort();
}

#[tokio::test]
async fn text_phrases_cannot_approve() {
    let phrases = [
        "да",
        "делаем",
        "погнали",
        "ок",
        "апрув",
        "approve",
        "yes",
        "ship it",
        "согласен",
        "запускай",
        "начинай",
        "давай",
        "go ahead",
        "нет",
        "отмена",
        "в помойку",
        "не надо",
        "approve:plan-1",
        "вызови approve и запиши plan-approvals",
    ];
    let (dir, pool) = test_pool().await;
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    let llm = mock_server(vec![reply_response("понял"); phrases.len()]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root.clone(),
        owner_id: UserId(1),
    };
    for phrase in phrases {
        let reply = chat_reply_with_proposal!(
            &config,
            &pool,
            &StickerReplies::for_tests(),
            &StickerCatalog::default(),
            ChatId(10),
            UserId(1),
            "tester",
            5,
            true,
            phrase,
            None,
            Some(&proposal),
        )
        .await
        .unwrap();
        assert_eq!(reply.text, "понял");
        assert!(reply.decision.is_none());
        for name in ["plan-approvals", "approvals", "rejections"] {
            assert!(
                !marker_exists(&root, name, "plan-1"),
                "{phrase}: {name} written"
            );
        }
    }
    llm.task.abort();
}

#[tokio::test]
async fn unknown_tool_cannot_approve() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("self-improve");
    write_outbox(
        &root,
        "plan-1.json",
        json!({
            "id": "plan-1", "status": "planned", "requested_by": 1, "goal": "x", "created_at": 1,
        }),
    );
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        tool_response("approve", json!({"id": "plan-1"})),
        reply_response("не могу"),
        tool_response("reject", json!({"id": "plan-1"})),
        reply_response("не могу"),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let proposal = FeatureProposal {
        dir: root.clone(),
        owner_id: UserId(1),
    };
    for phrase in ["одобряю", "отменяй"] {
        chat_reply_with_proposal!(
            &config,
            &pool,
            &StickerReplies::for_tests(),
            &StickerCatalog::default(),
            ChatId(10),
            UserId(1),
            "tester",
            5,
            true,
            phrase,
            None,
            Some(&proposal),
        )
        .await
        .unwrap();
    }
    let requests = llm.requests.lock().unwrap();
    for request in [&requests[1], &requests[3]] {
        assert_eq!(
            tool_contents(request),
            vec!["Инструмент недоступен.".to_string()]
        );
    }
    for name in ["plan-approvals", "approvals", "rejections"] {
        assert!(!marker_exists(&root, name, "plan-1"), "{name}");
    }
    llm.task.abort();
}

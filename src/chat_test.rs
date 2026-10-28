use crate::prelude::*;
use crate::*;
use crate::test_support::fake_settings;

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

struct MockServer {
    url: Url,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

async fn mock_server(responses: Vec<String>) -> MockServer {
    use tokio::net::TcpListener;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let queue = Arc::new(std::sync::Mutex::new(responses));
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { break };
            let captured = captured.clone();
            let queue = queue.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 { break }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(head_end) = find(&buffer, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buffer[..head_end]).to_ascii_lowercase();
                        let length = head.lines().find_map(|line| {
                            line.strip_prefix("content-length:").map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        }).unwrap_or(0);
                        if buffer.len() >= head_end + 4 + length { break }
                    }
                }
                captured.lock().unwrap().push(String::from_utf8_lossy(&buffer).into_owned());
                let body = queue.lock().unwrap().drain(..1).next().unwrap_or_else(|| {
                    r#"{"choices":[{"message":{"role":"assistant","content":""}}]}"#.into()
                });
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body);
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    MockServer { url: Url::parse(&format!("http://{address}/")).unwrap(), requests, task }
}

fn body_of(request: &str) -> Value {
    let (_, body) = request.split_once("\r\n\r\n").unwrap_or(("", ""));
    serde_json::from_str(body).unwrap_or(Value::Null)
}

#[test]
fn chat_prompt_needs_the_name_in_groups() {
    assert_eq!(chat_prompt("привет", true, "MyBot", false).as_deref(), Some("привет"));
    assert_eq!(chat_prompt("максут, как дела?", false, "MyBot", false).as_deref(), Some("как дела"));
    assert_eq!(chat_prompt("Максут что такое Rust", false, "MyBot", false).as_deref(), Some("что такое Rust"));
    assert_eq!(chat_prompt("максвелл что такое Rust", false, "MyBot", false), None);
    assert_eq!(chat_prompt("@mybot расскажи", false, "MyBot", false).as_deref(), Some("расскажи"));
    assert_eq!(chat_prompt("ответ на реплай", false, "MyBot", true).as_deref(), Some("ответ на реплай"));
    assert_eq!(chat_prompt("просто сообщение", false, "MyBot", false), None);
    assert_eq!(chat_prompt("макс, привет", false, "MyBot", false), None);
    assert_eq!(chat_prompt("максут", false, "MyBot", false), None);
    assert_eq!(chat_prompt("   ", true, "MyBot", false), None);
}

#[test]
fn replies_are_lowercased_except_urls() {
    assert_eq!(lowercase_reply("Привет! Как Дела?"), "привет! как дела?");
    assert_eq!(lowercase_reply("Смотри https://Example.com/Path"), "смотри https://Example.com/Path");
    assert_eq!(lowercase_reply("(https://Ex.com/Path)"), "(https://Ex.com/Path)");
    assert_eq!(lowercase_reply("Москва — Это Город"), "москва — это город");
}

#[test]
fn replies_are_short_and_without_a_trailing_period() {
    assert_eq!(style_reply("Привет. Как дела."), "привет. как дела");
    assert_eq!(style_reply("привет...\n\nкак дела."), "привет... как дела");
    assert_eq!(style_reply("Смотри https://Example.com/Path."), "смотри https://Example.com/Path");
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
    let many_lines = (0..500).map(|index| format!("строка {index}")).collect::<Vec<_>>().join("\n");
    assert!(split_message(&many_lines).iter().all(|chunk| chunk.chars().count() <= 3900));
}

#[test]
fn search_results_are_extracted_and_decoded() {
    let html = r#"
        <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&amp;rut=abc">Example &amp; Co</a>
        <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage">Snippet <b>bold</b> text</a>
    "#;
    let results = extract_search_results(html);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "Example & Co");
    assert_eq!(results[0].1, "https://example.com/page");
    assert_eq!(results[0].2, "Snippet bold text");
}

#[tokio::test]
async fn chat_reply_sends_context_and_stores_turns() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"Привет!"}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"второй ответ"}}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), llm.url.clone(), None);
    let chat_id = ChatId(10);
    let user_id = UserId(20);

    assert_eq!(chat_reply(&config, &pool, chat_id, user_id, "привет", None, None).await.unwrap().text, "привет!");
    assert_eq!(chat_reply(&config, &pool, chat_id, user_id, "как ты", None, None).await.unwrap().text, "второй ответ");

    let turns: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_turns").fetch_one(&pool).await.unwrap();
    assert_eq!(turns, 4);

    let requests = llm.requests.lock().unwrap();
    let second = body_of(&requests[1]);
    assert_eq!(second["model"], "deepseek-v4.1-flash");
    let messages = second["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert!(messages.iter().any(|message| message["content"] == "привет"));
    assert!(messages.iter().any(|message| message["content"] == "привет!"));
    assert_eq!(messages.last().unwrap()["content"], "как ты");
    llm.task.abort();
}

#[tokio::test]
async fn chat_reply_runs_only_allowlisted_search_tools() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"Rust\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"Нашёл пример"}}]}"#.into(),
    ]).await;
    let search = mock_server(vec![
        r#"<a rel="nofollow" class="result__a" href="https://example.com/page">Example</a><a class="result__snippet">Snippet</a>"#.into(),
    ]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), search.url.clone(), None);

    let reply = chat_reply(&config, &pool, ChatId(10), UserId(20), "найди Rust", None, None).await.unwrap();
    assert_eq!(reply.text, "нашёл пример");

    let requests = llm.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let second = body_of(&requests[1]);
    let messages = second["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["role"] == "tool"
        && message["content"].as_str().is_some_and(|content| content.contains("https://example.com/page"))));
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
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![r#"{"choices":[{"message":{"role":"assistant","content":"на картинке мем"}}]}"#.into()]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), llm.url.clone(), None);
    let image = ChatImage { bytes: b"hello".to_vec(), mime: "image/png".into() };
    let reply = chat_reply(&config, &pool, ChatId(1), UserId(2), "что это?", Some(&image), None).await.unwrap();
    assert_eq!(reply.text, "на картинке мем");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    assert_eq!(body["reasoning_effort"], "none");
    assert_eq!(body["messages"][1]["content"][1]["image_url"]["url"], "data:image/png;base64,aGVsbG8=");
    llm.task.abort();
}

#[tokio::test]
async fn bare_media_asks_for_a_reaction_instead_of_a_description() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![r#"{"choices":[{"message":{"role":"assistant","content":"понял, молчу"}}]}"#.into()]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), llm.url.clone(), None);
    let image = ChatImage { bytes: b"hello".to_vec(), mime: "image/webp".into() };
    let reply = chat_reply(&config, &pool, ChatId(1), UserId(2), "", Some(&image), None).await.unwrap();
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
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![r#"{"choices":[{"message":{"role":"assistant","content":"борщ"}}]}"#.into()]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), llm.url.clone(), None);
    let reply = chat_reply(&config, &pool, ChatId(1), UserId(2), "что я ел?", None, Some("@crudoshlepa: я ел борщ")).await.unwrap();
    assert_eq!(reply.text, "борщ");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[0]);
    let text = body["messages"][1]["content"].as_str().unwrap();
    assert!(text.contains("я ел борщ"), "{text}");
    assert!(text.contains("что я ел?"), "{text}");
    llm.task.abort();
}

#[tokio::test]
async fn recent_buffer_keeps_group_messages() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_settings(dir.path(), "#!/bin/sh\n").await;
    remember_message(&cfg, ChatId(1), 10, "@a".to_owned(), "борщ".to_owned().as_str()).await;
    remember_message(&cfg, ChatId(1), 11, "@b".to_owned(), "текущее".to_owned().as_str()).await;
    let recent = recent_chat_context(&cfg, ChatId(1), 11).await.unwrap();
    assert!(recent.contains("@a: борщ"));
    assert!(!recent.contains("текущее"));
}

#[tokio::test]
async fn tavily_search_uses_the_key_and_parses_results() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"Rust\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"есть"}}]}"#.into(),
    ]).await;
    let search = mock_server(vec![
        r#"{"results":[{"title":"Rust","url":"https://rust-lang.org","content":"systems language"}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), search.url.clone(), Some("tavily-key".into()));
    let reply = chat_reply(&config, &pool, ChatId(1), UserId(2), "погугли rust", None, None).await.unwrap();
    assert_eq!(reply.text, "есть");
    let requests = llm.requests.lock().unwrap();
    let body = body_of(&requests[1]);
    let tool = body["messages"].as_array().unwrap().iter().find(|message| message["role"] == "tool").unwrap();
    assert!(tool["content"].as_str().unwrap().contains("rust-lang.org"));
    let search_requests = search.requests.lock().unwrap();
    assert!(search_requests[0].contains("tavily-key"));
    assert!(search_requests[0].contains("Rust"));
    llm.task.abort();
    search.task.abort();
}

#[tokio::test]
async fn chat_reply_can_send_a_pack_sticker() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"send_sticker","arguments":"{\"name\":\"pizda\"}"}}]}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"держи"}}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), llm.url.clone(), None);
    let reply = chat_reply(&config, &pool, ChatId(1), UserId(2), "скинь стикер", None, None).await.unwrap();
    assert_eq!(reply.sticker, Some(PIZDA));
    assert_eq!(reply.text, "держи");
    llm.task.abort();
}

#[tokio::test]
async fn every_chat_uses_the_full_communication_guide() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let llm = mock_server(vec![
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
        r#"{"choices":[{"message":{"role":"assistant","content":"ок"}}]}"#.into(),
    ]).await;
    let config = ChatConfig::new(llm.url.clone(), "key".into(), "deepseek-v4.1-flash".into(), llm.url.clone(), None);
    chat_reply(&config, &pool, ChatId(1), UserId(2), "привет", None, None).await.unwrap();
    chat_reply(&config, &pool, ChatId(3), UserId(4), "привет", None, None).await.unwrap();
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
    let config = ChatConfig::new(base.clone(), "key".into(), "model".into(), base, None);
    config.pace(UserId(1)).await;
    let started = time::Instant::now();
    config.pace(UserId(1)).await;
    assert!(started.elapsed() >= Duration::from_secs(2));
    config.pace(UserId(2)).await;
}

#[tokio::test]
async fn context_expires_after_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_database(&format!("sqlite://{}", dir.path().join("state.db").display())).await.unwrap();
    let chat_id = ChatId(10);
    let user_id = UserId(20);
    append_chat_turn(&pool, chat_id, user_id, "user", "старое").await.unwrap();
    append_chat_turn(&pool, chat_id, user_id, "assistant", "старый ответ").await.unwrap();
    let old = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 - 25 * 60 * 60;
    sqlx::query("UPDATE chat_turns SET created_at = ?").bind(old).execute(&pool).await.unwrap();
    append_chat_turn(&pool, chat_id, user_id, "user", "свежее").await.unwrap();

    let context = load_chat_context(&pool, chat_id, user_id).await.unwrap();
    assert_eq!(context.len(), 1);
    assert_eq!(context[0]["content"], "свежее");
}

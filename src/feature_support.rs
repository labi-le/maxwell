use crate::prelude::*;
use crate::test_support::*;
use crate::*;

use std::collections::VecDeque;

pub(crate) const LINK: &str = "https://www.youtube.com/playlist?list=features&variant=one";
pub(crate) const SECRET: &str = "synthetic-secret-marker";

#[derive(Clone, Debug)]
pub(crate) struct FeatureRequest {
    pub(crate) method: String,
    pub(crate) fields: Value,
    pub(crate) uploads: Vec<(String, Vec<u8>)>,
}

impl FeatureRequest {
    pub(crate) fn is_media(&self) -> bool {
        matches!(
            self.method.as_str(),
            "sendmediagroup" | "sendphoto" | "sendvideo" | "sendaudio" | "senddocument"
        )
    }

    pub(crate) fn media(&self) -> Vec<Value> {
        if self.method == "sendmediagroup" {
            return self.fields["media"]
                .as_array()
                .expect("album media array")
                .clone();
        }
        let kind = match self.method.as_str() {
            "sendphoto" => "photo",
            "sendvideo" => "video",
            "sendaudio" => "audio",
            "senddocument" => "document",
            _ => return Vec::new(),
        };
        vec![serde_json::json!({
            "type": kind, "media": self.fields[kind], "caption": self.fields["caption"],
            "parse_mode": self.fields["parse_mode"],
            "supports_streaming": self.fields["supports_streaming"],
        })]
    }

    pub(crate) fn upload(&self, media: &Value) -> Option<&[u8]> {
        let name = media["media"].as_str()?.strip_prefix("attach://")?;
        self.uploads
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

pub(crate) enum FeatureReply {
    Reject {
        code: u16,
        description: String,
        retry_after: Option<u32>,
    },
    Disconnect,
    InvalidResponse,
    MissingIds,
}

impl FeatureReply {
    pub(crate) fn reject(description: &str) -> Self {
        Self::Reject {
            code: 400,
            description: description.into(),
            retry_after: None,
        }
    }

    pub(crate) fn rate_limit() -> Self {
        Self::Reject {
            code: 429,
            description: "Too Many Requests: retry after 17".into(),
            retry_after: Some(17),
        }
    }
}

#[derive(Default)]
pub(crate) struct FeatureApiState {
    pub(crate) replies: VecDeque<FeatureReply>,
    pub(crate) delete_fails: bool,
}

pub(crate) struct FeatureApi {
    pub(crate) bot: Bot,
    pub(crate) url: Url,
    pub(crate) requests: Arc<tokio::sync::Mutex<Vec<FeatureRequest>>>,
    pub(crate) state: Arc<tokio::sync::Mutex<FeatureApiState>>,
    pub(crate) server: tokio::task::JoinHandle<()>,
}

impl Drop for FeatureApi {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub(crate) fn feature_find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|part| part == needle)
}

pub(crate) async fn feature_read_more(stream: &mut tokio::net::TcpStream, bytes: &mut Vec<u8>) {
    let mut chunk = [0; 4096];
    let count = stream
        .read(&mut chunk)
        .await
        .expect("reading local Bot API request");
    assert_ne!(count, 0, "incomplete local Bot API request");
    bytes.extend_from_slice(&chunk[..count]);
    assert!(
        bytes.len() <= 8 * 1024 * 1024,
        "fixture request exceeded bounded body size"
    );
}

pub(crate) fn feature_field(value: &[u8]) -> Value {
    serde_json::from_slice(value).unwrap_or_else(|_| {
        Value::String(String::from_utf8(value.to_vec()).expect("UTF-8 Bot API field"))
    })
}

pub(crate) async fn feature_read_request(stream: &mut tokio::net::TcpStream) -> FeatureRequest {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(end) = feature_find(&bytes, b"\r\n\r\n") {
            break end + 4;
        }
        feature_read_more(stream, &mut bytes).await;
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let path = headers
        .split_whitespace()
        .nth(1)
        .expect("Bot API method path");
    let header = |name: &str| {
        headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    };
    let mut body = Vec::new();
    if header("transfer-encoding").is_some_and(|value| value.eq_ignore_ascii_case("chunked")) {
        let mut cursor = header_end;
        loop {
            let line_end = loop {
                if let Some(end) = feature_find(&bytes[cursor..], b"\r\n") {
                    break cursor + end;
                }
                feature_read_more(stream, &mut bytes).await;
            };
            let size = std::str::from_utf8(&bytes[cursor..line_end])
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .trim();
            let size = usize::from_str_radix(size, 16).expect("valid HTTP chunk size");
            cursor = line_end + 2;
            if size == 0 {
                // Consume optional trailers rather than guessing a raw-body terminator.
                loop {
                    let end = loop {
                        if let Some(end) = feature_find(&bytes[cursor..], b"\r\n") {
                            break cursor + end;
                        }
                        feature_read_more(stream, &mut bytes).await;
                    };
                    if end == cursor {
                        break;
                    }
                    cursor = end + 2;
                }
                break;
            }
            while bytes.len() < cursor + size + 2 {
                feature_read_more(stream, &mut bytes).await;
            }
            body.extend_from_slice(&bytes[cursor..cursor + size]);
            assert_eq!(&bytes[cursor + size..cursor + size + 2], b"\r\n");
            cursor += size + 2;
        }
    } else {
        let length: usize = header("content-length").unwrap_or("0").parse().unwrap();
        while bytes.len() < header_end + length {
            feature_read_more(stream, &mut bytes).await;
        }
        body.extend_from_slice(&bytes[header_end..header_end + length]);
    }
    let content_type = header("content-type").unwrap_or("");
    let mut fields = serde_json::json!({});
    let mut uploads = Vec::new();
    if content_type.starts_with("multipart/form-data") {
        let boundary = content_type
            .split(';')
            .find_map(|part| part.trim().strip_prefix("boundary="))
            .expect("multipart boundary")
            .trim_matches('"');
        let delimiter = format!("--{boundary}");
        let next_delimiter = format!("\r\n{delimiter}");
        let mut cursor = feature_find(&body, delimiter.as_bytes()).unwrap() + delimiter.len();
        loop {
            if body[cursor..].starts_with(b"--") {
                break;
            }
            assert!(body[cursor..].starts_with(b"\r\n"));
            cursor += 2;
            let end = cursor + feature_find(&body[cursor..], b"\r\n\r\n").unwrap();
            let part_headers = std::str::from_utf8(&body[cursor..end]).unwrap();
            let disposition = part_headers
                .lines()
                .find(|line| {
                    line.to_ascii_lowercase()
                        .starts_with("content-disposition:")
                })
                .expect("multipart disposition");
            let attribute = |name: &str| {
                disposition.split(';').find_map(|part| {
                    let (key, value) = part.trim().split_once('=')?;
                    (key == name).then(|| value.trim_matches('"'))
                })
            };
            let name = attribute("name").expect("multipart field name");
            cursor = end + 4;
            let end = cursor + feature_find(&body[cursor..], next_delimiter.as_bytes()).unwrap();
            let value = &body[cursor..end];
            if attribute("filename").is_some() {
                uploads.push((name.to_owned(), value.to_vec()));
                fields[name] = Value::String(format!("attach://{name}"));
            } else if matches!(name, "caption" | "text") {
                fields[name] = Value::String(String::from_utf8(value.to_vec()).unwrap());
            } else {
                fields[name] = feature_field(value);
            }
            cursor = end + 2 + delimiter.len();
        }
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        for (name, value) in url::form_urlencoded::parse(&body) {
            fields[name.as_ref()] = feature_field(value.as_bytes());
        }
    } else if !body.is_empty() {
        fields = serde_json::from_slice(&body).expect("JSON Bot API body");
    }
    FeatureRequest {
        method: path.rsplit('/').next().unwrap().to_ascii_lowercase(),
        fields,
        uploads,
    }
}

pub(crate) fn feature_message(
    chat: i64,
    ordinal: usize,
    kind: Option<&str>,
    file_id: &str,
) -> Value {
    let mut message = serde_json::json!({
        "message_id": 100 + ordinal, "date": 1700000000,
        "chat": { "id": chat, "type": if chat < 0 { "supergroup" } else { "private" } },
    });
    let file = serde_json::json!({
        "file_id": file_id, "file_unique_id": format!("unique-{file_id}"), "file_size": 20,
        "width": 32, "height": 32, "duration": 1, "file_name": "delivered.bin",
        "mime_type": match kind {
            Some("video") => "video/mp4",
            Some("audio") => "audio/mpeg",
            Some("document") => "application/octet-stream",
            _ => "image/jpeg",
        },
    });
    if let Some(kind) = kind {
        message[kind] = if kind == "photo" {
            serde_json::json!([file])
        } else {
            file
        };
    }
    message
}

impl FeatureApi {
    pub(crate) async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let state = Arc::new(tokio::sync::Mutex::new(FeatureApiState::default()));
        let records = Arc::clone(&requests);
        let replies = Arc::clone(&state);
        let server = tokio::spawn(async move {
            let mut ordinal = 0;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = feature_read_request(&mut stream).await;
                let chat = request.fields["chat_id"]
                    .as_i64()
                    .or_else(|| {
                        request.fields["chat_id"]
                            .as_str()
                            .and_then(|value| value.parse().ok())
                    })
                    .unwrap_or(1);
                let mut state = replies.lock().await;
                let reply = if request.is_media() {
                    state.replies.pop_front()
                } else {
                    None
                };
                let delete_fails = state.delete_fails;
                drop(state);
                let media = request.media();
                let method = request.method.clone();
                records.lock().await.push(request);
                let response = match reply {
                    Some(FeatureReply::Disconnect) => continue,
                    Some(FeatureReply::InvalidResponse) => Value::String(format!(
                        "invalid JSON https://evil.invalid/?token={SECRET} Cookie: {SECRET} BOT_TOKEN={SECRET}"
                    )),
                    Some(FeatureReply::Reject { code, description, retry_after }) => {
                        let mut response = serde_json::json!({"ok": false,
                            "error_code": code, "description": description});
                        if let Some(seconds) = retry_after {
                            response["parameters"] = serde_json::json!({"retry_after": seconds});
                        }
                        response
                    }
                    Some(FeatureReply::MissingIds) => {
                        let messages: Vec<_> = media.iter().enumerate().map(|(index, item)| {
                            ordinal += 1;
                            let kind = item["type"].as_str().unwrap();
                            feature_message(chat, ordinal, (index != 0).then_some(kind),
                                &format!("features-{kind}-{ordinal}"))
                        }).collect();
                        let result = if method == "sendmediagroup" { serde_json::json!(messages) }
                            else { messages.into_iter().next().unwrap() };
                        serde_json::json!({"ok": true, "result": result})
                    }
                    None if method == "deletemessage" => {
                        if delete_fails {
                            serde_json::json!({"ok": false, "error_code": 400,
                                "description": "Bad Request: not enough rights to delete messages"})
                        } else { serde_json::json!({"ok": true, "result": true}) }
                    }
                    None => {
                        let messages: Vec<_> = media.iter().map(|item| {
                            ordinal += 1;
                            let kind = item["type"].as_str().unwrap();
                            let id = item["media"].as_str().filter(|id| !id.starts_with("attach://"))
                                .map(str::to_owned).unwrap_or_else(|| format!("features-{kind}-{ordinal}"));
                            feature_message(chat, ordinal, Some(kind), &id)
                        }).collect();
                        let result = if method == "sendmediagroup" { serde_json::json!(messages) }
                            else { messages.into_iter().next().unwrap_or_else(|| {
                                feature_message(chat, ordinal, None, "")
                            }) };
                        serde_json::json!({"ok": true, "result": result})
                    }
                };
                let body = match response {
                    Value::String(raw) => raw,
                    response => response.to_string(),
                };
                let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                stream.write_all(headers.as_bytes()).await.unwrap();
                stream.write_all(body.as_bytes()).await.unwrap();
            }
        });
        Self {
            bot: Bot::new("123:features-token-one").set_api_url(url.clone()),
            url,
            requests,
            state,
            server,
        }
    }

    pub(crate) async fn snapshot(&self) -> Vec<FeatureRequest> {
        self.requests.lock().await.clone()
    }
    pub(crate) async fn clear(&self) {
        self.requests.lock().await.clear();
    }
    pub(crate) async fn reject_next(&self, reply: FeatureReply) {
        self.state.lock().await.replies.push_back(reply);
    }
}

pub(crate) fn feature_script(root: &Path, extensions: &[&str], tail: &str) -> String {
    let mut script = format!(
        r#"#!/bin/sh
# CUSTOM_METADATA
printf 'call\n' >> '{}/calls'
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
printf '%s\n' '{{"formats":[]}}'
exit 0
  fi
done
while [ "$#" -gt 0 ]; do
  case "$1" in
--output) shift; dir=${{1%/*}} ;;
--print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
"#,
        root.display()
    );
    for (index, extension) in extensions.iter().enumerate() {
        // Reverse lexical filenames to expose any sorting of publication order.
        let name = format!("asset-{:02}.{extension}", extensions.len() - index);
        let description = serde_json::to_string(&format!("Post {index} & <source>")).unwrap();
        if *extension == "mp4" {
            script.push_str(&format!(
                "cp '{}' \"$dir/{name}\"\n",
                av_fixture().display()
            ));
        } else {
            script.push_str(&format!(
                "printf '%s' 'asset-{index:02}' > \"$dir/{name}\"\n"
            ));
        }
        script.push_str(&format!("printf '%s\\n' \"$dir/{name}\" >> \"$manifest\"\nprintf '\"%s\"\\t\"\"\\t%s\\n' \"$dir/{name}\" '{description}'\n"));
    }
    script.push_str(tail);
    script.push('\n');
    script
}

pub(crate) struct FeatureFixture {
    pub(crate) root: tempfile::TempDir,
    pub(crate) cfg: Arc<Settings>,
    pub(crate) api: FeatureApi,
}

impl FeatureFixture {
    pub(crate) async fn new(extensions: &[&str], tail: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut cfg =
            fake_settings(root.path(), &feature_script(root.path(), extensions, tail)).await;
        cfg.max_media_bytes = 1_000_000;
        Self {
            root,
            cfg: Arc::new(cfg),
            api: FeatureApi::new().await,
        }
    }

    pub(crate) fn calls(&self) -> usize {
        fs::read_to_string(self.root.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    pub(crate) async fn send(&self, chat: i64, sender: u64, name: &str, url: &str) {
        self.send_text(chat, sender, name, url).await;
    }

    pub(crate) async fn send_text(&self, chat: i64, sender: u64, name: &str, text: &str) {
        let message = serde_json::from_value::<Message>(serde_json::json!({
            "message_id": 73, "date": 1700000000,
            "chat": { "id": chat, "type": if chat < 0 { "supergroup" } else { "private" } },
            "from": { "id": sender, "is_bot": false, "first_name": name,
                "last_name": "Never use surname", "username": "never_mention_username" },
            "text": text,
        }))
        .unwrap();
        time::timeout(
            Duration::from_secs(5),
            handle_message(self.api.bot.clone(), message, Arc::clone(&self.cfg)),
        )
        .await
        .expect("feature handler stalled")
        .unwrap();
    }

    pub(crate) async fn warm(&self) {
        self.send(1, 1, "First", LINK).await;
        let before = self.calls();
        self.api.clear().await;
        self.send(1, 1, "Second", LINK).await;
        assert_eq!(
            self.calls(),
            before,
            "a warm same-chat publication must bypass yt-dlp"
        );
        let requests = self.api.snapshot().await;
        let media: Vec<_> = requests
            .iter()
            .filter(|request| request.is_media())
            .collect();
        assert!(
            !media.is_empty(),
            "warm publication must still be delivered"
        );
        assert!(
            media.iter().all(|request| request.uploads.is_empty()),
            "warm publication must use Telegram file_ids, not binary uploads"
        );
    }
}

pub(crate) fn feature_silent(requests: &[FeatureRequest]) {
    for request in requests
        .iter()
        .filter(|request| request.is_media() || request.method == "sendmessage")
    {
        assert_eq!(
            request.fields["disable_notification"], true,
            "{} must remain silent",
            request.method
        );
    }
}

pub(crate) fn feature_retained(requests: &[FeatureRequest]) {
    assert!(
        !requests
            .iter()
            .any(|request| request.method == "deletemessage"),
        "failed or incomplete delivery must retain the source message"
    );
}

pub(crate) fn feature_reason(requests: &[FeatureRequest], keywords: &[&str]) -> String {
    let messages: Vec<_> = requests
        .iter()
        .filter(|request| request.method == "sendmessage")
        .map(|request| request.fields["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        messages.len(),
        1,
        "one actionable failure reason is expected"
    );
    let reason = messages[0].to_lowercase();
    assert!(
        keywords.iter().any(|keyword| reason.contains(keyword)),
        "specific cause {:?} absent from user reason: {}",
        keywords,
        messages[0]
    );
    for secret in [
        SECRET,
        "https://evil.invalid",
        "cookie: ",
        "bot_token=",
        "features-token-one",
    ] {
        assert!(
            !reason.contains(secret),
            "untrusted diagnostics leaked to chat: {reason}"
        );
    }
    feature_silent(requests);
    reason
}

pub(crate) async fn feature_journal_is_safe(pool: &SqlitePool) {
    let rows: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT url_host, url_path, stage, detail FROM download_failures")
            .fetch_all(pool)
            .await
            .unwrap();
    assert!(
        !rows.is_empty(),
        "failure must remain available to owner diagnostics"
    );
    let events: Vec<(String, String)> =
        sqlx::query_as("SELECT stage, detail FROM download_failure_events")
            .fetch_all(pool)
            .await
            .unwrap();
    for field in rows
        .iter()
        .flat_map(|(host, path, stage, detail)| [host, path, stage, detail])
        .chain(events.iter().flat_map(|(stage, detail)| [stage, detail]))
    {
        for secret in [
            SECRET,
            "evil.invalid",
            "Cookie:",
            "BOT_TOKEN=",
            "?list=",
            "features-token-one",
        ] {
            assert!(
                !field.contains(secret),
                "untrusted diagnostic leaked to SQLite: {field}"
            );
        }
    }
}

pub(crate) async fn feature_journal_cause(pool: &SqlitePool, keywords: &[&str]) {
    let details: Vec<String> = sqlx::query_scalar(
        "SELECT detail FROM download_failures UNION ALL SELECT detail FROM download_failure_events",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(details.iter().any(|detail| keywords.iter().any(|keyword|
        detail.to_lowercase().contains(keyword))),
        "owner journal must keep the specific safe cause {:?}, not a generic delivery failure: {:?}",
        keywords, details);
}

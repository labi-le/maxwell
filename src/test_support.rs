use crate::prelude::*;
use crate::*;

pub(crate) fn av_fixture() -> &'static Path {
    static FILE: std::sync::LazyLock<tempfile::NamedTempFile> = std::sync::LazyLock::new(|| {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(std::process::Command::new("ffmpeg").args([
            "-nostdin", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=32x32:rate=5",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=0.2",
            "-t", "0.2", "-c:v", "libx264", "-threads:v", "1", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-f", "mp4", "-y",
        ]).arg(file.path()).status().expect("ffmpeg fixture prerequisite").success());
        file
    });
    FILE.path()
}

#[cfg(unix)]
pub(crate) async fn fake_settings(root: &Path, script: &str) -> Settings {
    use std::os::unix::fs::PermissionsExt;
    let fake = root.join("fake-dlp");
    let metadata = "for arg in \"$@\"; do\n  if [ \"$arg\" = --dump-single-json ]; then\n    printf '%s\\n' '{\"formats\":[{\"url\":\"https://example.org/video.mp4\"}]}'\n    exit 0\n  fi\ndone\n";
    let script = if script.contains("CUSTOM_METADATA") {
        script.to_string()
    } else {
        script.replacen("#!/bin/sh\n", &format!("#!/bin/sh\n{metadata}"), 1)
    };
    fs::write(&fake, script).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let db = open_database(&format!("sqlite://{}", root.join("state.db").display())).await.unwrap();
    Settings {
        owner_id: UserId(1), bot_username: "MyBot".into(), database: db,
        download_dir: root.into(), ytdlp_bin: fake,
        python_bin: PathBuf::from("/nonexistent/python"), twitter_helper: root.join("twitter-helper"),
        cookies_file: None, cookie_refresh_url: None, chat: None,
        chat_recent: Arc::new(RwLock::new(HashMap::new())),
        sticker_replies: StickerReplies::for_tests(),
        max_media_bytes: 100,
        downloads: Arc::new(Semaphore::new(2)),
        user_rate_limits: Arc::new(RwLock::new(HashMap::new())),
    }
}

#[cfg(unix)]
pub(crate) fn cookie_refresh_jar(fresh: bool) -> &'static str {
    if fresh {
        "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t4102444800\tfixture\tfresh-synthetic-marker\n"
    } else {
        "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t4102444800\tfixture\tstale-synthetic-marker\n"
    }
}

#[cfg(unix)]
pub(crate) async fn cookie_refresh_fixture(root: &Path, always_fail: bool) -> (Settings, PathBuf) {
    let source = root.join("mounted-cookies.txt");
    fs::write(&source, cookie_refresh_jar(false)).unwrap();
    let script = format!(r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
printf '%s\n' '{{"formats":[]}}'
exit 0
  fi
done
cookie=
while [ "$#" -gt 0 ]; do
  case "$1" in
--cookies) shift; cookie=$1 ;;
--output) shift; dir=${{1%/*}} ;;
--print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf 'attempt\n' >> '{0}/attempts'
[ -f "$cookie" ] || exit 22
fresh=no
case "$(cat "$cookie")" in
  *fresh-synthetic-marker*) fresh=yes ;;
esac
if [ '{1}' = true ] || [ "$fresh" != yes ]; then
  printf '%s\n' 'ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies.' >&2
  if [ '{1}' = true ] && [ "$fresh" = yes ]; then exit 9; fi
  exit 1
fi
printf video > "$dir/finished.mp4"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
"#, root.display(), always_fail);
    let mut cfg = fake_settings(root, &script).await;
    cfg.cookies_file = Some(source.clone());
    (cfg, source)
}

#[cfg(unix)]
pub(crate) async fn cookie_refresh_partial_fixture(root: &Path, completed_video: bool) -> (Settings, PathBuf) {
    let source = root.join("mounted-cookies.txt");
    fs::write(&source, cookie_refresh_jar(false)).unwrap();
    let retry = if completed_video {
        "printf video > \"$dir/finished.mp4\"\nprintf '%s\\n' \"$dir/finished.mp4\" > \"$manifest\"\nprintf '%s\\n' 'ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies.' >&2"
    } else {
        "printf '%s\\n' 'ERROR: [youtube] HTTP Error 403' >&2"
    };
    let script = format!(r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
printf '%s\n' '{{"formats":[]}}'
exit 0
  fi
done
cookie=
while [ "$#" -gt 0 ]; do
  case "$1" in
--cookies) shift; cookie=$1 ;;
--output) shift; dir=${{1%/*}} ;;
--print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf 'attempt\n' >> '{}/attempts'
[ -f "$cookie" ] || exit 22
case "$(cat "$cookie")" in
  *fresh-synthetic-marker*) {} ;;
  *) printf '%s\n' 'ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies.' >&2; exit 1 ;;
esac
exit 9
"#, root.display(), retry);
    let mut cfg = fake_settings(root, &script).await;
    cfg.cookies_file = Some(source.clone());
    (cfg, source)
}

#[cfg(unix)]
pub(crate) fn cookie_refresh_attempts(root: &Path) -> usize {
    fs::read_to_string(root.join("attempts")).unwrap().lines().count()
}

#[cfg(unix)]
pub(crate) async fn fake_cookie_service(
    source: PathBuf, status: u16,
) -> (Url, std::sync::Arc<std::sync::atomic::AtomicUsize>,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let sources = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls_task = calls.clone();
    let sources_task = sources.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { break };
            let source = source.clone();
            let calls = calls_task.clone();
            let sources = sources_task.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 8192];
                let size = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..size]);
                if !request.starts_with("POST /refresh") { return; }
                calls.fetch_add(1, Ordering::SeqCst);
                if let Some((_, body)) = request.split_once("\r\n\r\n") {
                    if let Ok(value) = serde_json::from_str::<Value>(body.trim()) {
                        if let Some(source) = value.get("source").and_then(Value::as_str) {
                            sources.lock().unwrap().push(source.to_owned());
                        }
                    }
                }
                if status == 200 {
                    let replacement = source.with_extension("new");
                    fs::write(&replacement, cookie_refresh_jar(true)).unwrap();
                    fs::rename(&replacement, &source).unwrap();
                }
                let body = if status == 200 {
                    r#"{"ok":true,"host":"pc","cookies":1}"#
                } else {
                    r#"{"ok":false}"#
                };
                let response = format!(
                    "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    if status == 200 { "OK" } else { "Service Unavailable" },
                    body.len(), body);
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (Url::parse(&format!("http://{address}")).unwrap(), calls, sources, task)
}

#[cfg(unix)]
pub(crate) async fn cookie_warning_regression_fixture(
    root: &Path, invalid_cookie_warning: bool, succeeds_with_stale_jar: bool,
) -> (Settings, PathBuf) {
    let source = root.join("mounted-cookies.txt");
    fs::write(&source, cookie_refresh_jar(false)).unwrap();
    let script = format!(r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
printf '%s\n' '{{"formats":[]}}'
exit 0
  fi
done
cookie=
warnings_suppressed=no
while [ "$#" -gt 0 ]; do
  case "$1" in
--cookies) shift; cookie=$1 ;;
--no-warnings) warnings_suppressed=yes ;;
--output) shift; dir=${{1%/*}} ;;
--print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf 'attempt\n' >> '{0}/attempts'
[ -f "$cookie" ] || exit 22
fresh=no
case "$(cat "$cookie")" in
  *fresh-synthetic-marker*) fresh=yes ;;
esac
if [ "$fresh" = no ] && [ '{1}' = true ] && [ "$warnings_suppressed" = no ]; then
  printf '%s\n' 'WARNING: [youtube] The provided YouTube account cookies are no longer valid. They have likely been rotated in the browser as a security measure.' >&2
fi
if [ "$fresh" = no ] && [ '{2}' = false ]; then
  printf '%s\n' 'ERROR: unable to download video data: HTTP Error 403: Forbidden' >&2
  exit 1
fi
printf video > "$dir/finished.mp4"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
"#, root.display(), invalid_cookie_warning, succeeds_with_stale_jar);
    let mut cfg = fake_settings(root, &script).await;
    cfg.cookies_file = Some(source.clone());
    (cfg, source)
}

pub(crate) async fn mock_photo_response(status: &'static str, body: &'static [u8], with_length: bool, content_type: &'static str) -> reqwest::Response {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).await.unwrap();
        let length = if with_length { format!("Content-Length: {}\r\n", body.len()) } else { String::new() };
        let headers = format!("HTTP/1.1 {status}\r\n{length}Content-Type: {content_type}\r\nLocation: http://127.0.0.1/private\r\nConnection: close\r\n\r\n");
        stream.write_all(headers.as_bytes()).await.unwrap();
        let _ = stream.write_all(body).await;
    });
    reqwest::Client::builder().redirect(Policy::none()).build().unwrap()
        .get(format!("http://{address}/photo.jpg")).send().await.unwrap()
}

#[cfg(unix)]
pub(crate) async fn recording_bot(video_ok: bool, delete_ok: bool)
    -> (Bot, Arc<tokio::sync::Mutex<Vec<(String, String)>>>, tokio::task::JoinHandle<()>)
{
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let records = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            let (end, length) = loop {
                let count = stream.read(&mut chunk).await.unwrap();
                if count == 0 { panic!("incomplete Bot API request"); }
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers.lines().find_map(|line| line.to_ascii_lowercase()
                        .strip_prefix("content-length: ").and_then(|length| length.parse::<usize>().ok()));
                    break (end + 4, length);
                }
            };
            while match length {
                Some(length) => bytes.len() < end + length,
                None => !bytes.ends_with(b"0\r\n\r\n"),
            } {
                let count = stream.read(&mut chunk).await.unwrap();
                if count == 0 { panic!("incomplete Bot API body"); }
                bytes.extend_from_slice(&chunk[..count]);
            }
            let path = String::from_utf8_lossy(&bytes).split_whitespace().nth(1).unwrap().to_string();
            let body = String::from_utf8_lossy(&bytes[end..]).into_owned();
            let response = if (api_method_is(&path, "sendVideo") && !video_ok)
                || (api_method_is(&path, "deleteMessage") && !delete_ok)
            {
                r#"{"ok":false,"error_code":400,"description":"Bad Request: not enough rights"}"#
            } else if api_method_is(&path, "deleteMessage") {
                r#"{"ok":true,"result":true}"#
            } else if api_method_is(&path, "getChatMember") {
                if serde_json::from_str::<Value>(&body).ok()
                    .and_then(|request| request["user_id"].as_u64()) == Some(2)
                {
                    r#"{"ok":true,"result":{"status":"administrator","user":{"id":2,"is_bot":false,"first_name":"Admin"},"can_be_edited":false,"is_anonymous":false,"can_manage_chat":true,"can_delete_messages":true,"can_manage_video_chats":false,"can_restrict_members":false,"can_promote_members":false,"can_change_info":false,"can_invite_users":false}}"#
                } else {
                    r#"{"ok":true,"result":{"status":"member","user":{"id":3,"is_bot":false,"first_name":"Member"}}}"#
                }
            } else {
                r#"{"ok":true,"result":{"message_id":99,"date":1700000000,"chat":{"id":1,"type":"private"}}}"#
            };
            records.lock().await.push((path.clone(), body));
            let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len());
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (Bot::new("123:dummy").set_api_url(Url::parse(&format!("http://{address}/")).unwrap()), requests, server)
}

#[cfg(unix)]
pub(crate) fn api_method_is(path: &str, method: &str) -> bool {
    path.rsplit('/').next().is_some_and(|actual| actual.eq_ignore_ascii_case(method))
}

#[cfg(target_os = "linux")]
pub(crate) async fn assert_descendant_stopped(pid: i32) {
    // A reparented child can briefly remain as a zombie awaiting init; allow signal delivery.
    for _ in 0..100 {
        let state = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if state.is_empty() || state.split_whitespace().nth(2) == Some("Z") {
            return;
        }
        time::sleep(Duration::from_millis(10)).await;
    }
    panic!("descendant {pid} still running");
}

#[cfg(unix)]
pub(crate) async fn metadata_caption_recorded_delivery(
    root: &Path, url: &str, metadata: &Value, downloaded_title: Option<&str>,
    downloaded_description: Option<&str>, message_text: Option<&str>,
    sender: Value, video_ok: bool,
) -> (Vec<(String, String)>, usize) {
    let script = format!(r#"#!/bin/sh
# CUSTOM_METADATA
for arg in "$@"; do
  if [ "$arg" = --dump-single-json ]; then
printf 'probe\n' >> '{}/probes'
printf '%s\n' '{}'
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
cp '{}' "$dir/finished.mp4"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
printf '"%s"\t%s\t%s\n' "$dir/finished.mp4" '{}' '{}'
"#, root.display(), metadata, av_fixture().display(),
        serde_json::to_string(downloaded_title.unwrap_or("")).unwrap(),
        serde_json::to_string(&downloaded_description).unwrap());
    let mut settings = fake_settings(root, &script).await;
    settings.max_media_bytes = 1_000_000;
    let (bot, records, server) = recording_bot(video_ok, true).await;
    let message = serde_json::from_value::<Message>(serde_json::json!({
        "message_id": 73, "date": 1700000000,
        "chat": {"id": 1, "type": "private"},
        "from": sender, "text": message_text.unwrap_or(url)
    })).unwrap();
    time::timeout(Duration::from_secs(10), handle_message(bot, message, Arc::new(settings)))
        .await.expect("metadata caption delivery stalled").unwrap();
    let captured = records.lock().await.clone();
    server.abort();
    let probes = fs::read_to_string(root.join("probes")).unwrap_or_default().lines().count();
    (captured, probes)
}

#[cfg(unix)]
pub(crate) fn metadata_caption_from_upload(body: &str) -> &str {
    body.split("name=\"caption\"").nth(1).expect("multipart caption field")
        .split_once("\r\n\r\n").expect("multipart caption value").1
        .trim_start_matches("\r\n")
        .split_once("\r\n").expect("caption chunk size").1
        .split("\r\n").next().expect("caption text")
}

#[cfg(unix)]
pub(crate) fn metadata_caption_from_video(calls: &[(String, String)]) -> &str {
    let body = &calls.iter().find(|(path, _)| api_method_is(path, "sendVideo"))
        .expect("expected a video upload").1;
    metadata_caption_from_upload(body)
}

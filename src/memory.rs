use crate::prelude::*;
use crate::*;
use tokio::sync::Mutex;

pub(crate) const FLAG_INJECTION: i64 = 1;
pub(crate) const FLAG_SECRET: i64 = 2;
pub(crate) const FLAG_JUNK: i64 = 4;

pub(crate) const SUMMARY_MAX_CHARS: usize = 1500;
pub(crate) const RECENT_MESSAGES: usize = 40;
pub(crate) const RECENT_MAX_CHARS: usize = 3000;
const KEEP_NEWEST: usize = 5;
const KEEP_FRESH: usize = 40;
const COMPACT_AFTER_SECS: i64 = 12 * 60 * 60;
const COMPACT_BATCH_MESSAGES: usize = 200;
const COMPACT_BATCH_CHARS: usize = 12_000;
const COMPACT_MESSAGE_CHARS: usize = 500;
const COMPACT_PASSES: usize = 3;
const COMPACT_INTERVAL_SECS: u64 = 10 * 60;
const LOAD_LIMIT: i64 = 2000;

#[derive(Clone)]
pub(crate) struct ChatMessage {
    pub(crate) id: i64,
    pub(crate) author_id: i64,
    pub(crate) author_name: String,
    pub(crate) text: String,
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs() as i64).unwrap_or(0)
}

pub(crate) fn detect_flags(text: &str) -> i64 {
    let trimmed = text.trim();
    let mut flags = 0;
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.chars().count() < 4
        || (!trimmed.contains(char::is_whitespace)
            && (trimmed.starts_with("http://") || trimmed.starts_with("https://")))
        || !trimmed.chars().any(|ch| ch.is_alphanumeric())
    {
        flags |= FLAG_JUNK;
    }
    let lowered = trimmed.to_lowercase();
    const INJECTIONS: &[&str] = &[
        "ignore previous", "ignore all previous", "system prompt", "you are now", "act as",
        "developer mode", "jailbreak", "prompt injection", "ты теперь", "забудь",
        "новые инструкции", "выполни команду", "служебное сообщение", "ignore the above",
        "disregard previous", "repeat the following", "твоя новая роль",
    ];
    if INJECTIONS.iter().any(|marker| lowered.contains(marker)) {
        flags |= FLAG_INJECTION;
    }
    const SECRET_MARKERS: &[&str] = &[
        "sk-", "ghp_", "xoxb", "akia", "-----begin", "password=", "password:",
        "token=", "token:", "cookie:", "api_key", "api key",
    ];
    if SECRET_MARKERS.iter().any(|marker| lowered.contains(marker)) {
        flags |= FLAG_SECRET;
    }
    let compact: String = trimmed.chars().filter(|ch| ch.is_ascii_alphanumeric()).collect();
    if compact.len() >= 40 && compact.chars().any(|ch| ch.is_ascii_digit()) {
        flags |= FLAG_SECRET;
    }
    flags
}

pub(crate) async fn record_message(
    pool: &SqlitePool,
    chat_id: ChatId,
    author_id: i64,
    author_name: &str,
    role: &str,
    text: &str,
    message_id: Option<i32>,
    flags: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO chat_messages (chat_id, author_id, author_name, role, text, message_id, flags, created_at, compacted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(chat_id.0)
    .bind(author_id)
    .bind(author_name)
    .bind(role)
    .bind(text)
    .bind(message_id.map(i64::from))
    .bind(flags)
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn load_memory(
    pool: &SqlitePool,
    chat_id: ChatId,
    current_id: i32,
) -> Result<(Option<String>, Vec<ChatMessage>), sqlx::Error> {
    let summary: Option<(String,)> =
        sqlx::query_as("SELECT summary FROM chat_summaries WHERE chat_id = ?")
            .bind(chat_id.0)
            .fetch_optional(pool)
            .await?;
    let rows: Vec<(i64, i64, String, String, i64)> = sqlx::query_as(
        "SELECT id, author_id, author_name, text, created_at FROM chat_messages WHERE chat_id = ? AND compacted_at IS NULL AND flags = 0 AND (message_id IS NULL OR message_id != ?) ORDER BY id DESC LIMIT 200",
    )
    .bind(chat_id.0)
    .bind(i64::from(current_id))
    .fetch_all(pool)
    .await?;
    let mut selected = Vec::new();
    let mut chars = 0;
    for (id, author_id, author_name, text, created_at) in rows {
        let line = format_line(&author_name, author_id, &text);
        chars += line.chars().count();
        if chars > RECENT_MAX_CHARS && !selected.is_empty() {
            break;
        }
        let _ = created_at;
        selected.push(ChatMessage { id, author_id, author_name, text });
        if selected.len() >= RECENT_MESSAGES {
            break;
        }
    }
    selected.reverse();
    let summary = summary.map(|(value,)| value).filter(|value| !value.trim().is_empty());
    Ok((summary, selected))
}

pub(crate) fn format_line(author_name: &str, author_id: i64, text: &str) -> String {
    format!("{author_name} [{author_id}]: {}", text.replace('\n', " "))
}

pub(crate) fn validate_summary(summary: &str) -> Result<String, String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err("empty summary".into());
    }
    let lowered = summary.to_lowercase();
    if lowered.contains("http://") || lowered.contains("https://") {
        return Err("summary contains a URL".into());
    }
    const REJECT: &[&str] = &[
        "ignore previous", "system prompt", "ты теперь", "developer mode", "jailbreak",
        "новые инструкции", "prompt injection", "разрешил всем", "владелец разрешил",
        "можно всем", "owner_id",
    ];
    if REJECT.iter().any(|marker| lowered.contains(marker)) {
        return Err("summary contains instructions".into());
    }
    Ok(summary.chars().take(SUMMARY_MAX_CHARS).collect())
}

pub(crate) async fn compact_chat(
    chat: &ChatConfig,
    pool: &SqlitePool,
    chat_id: i64,
) -> Result<bool, String> {
    let rows: Vec<(i64, i64, String, String, i64)> = sqlx::query_as(
        "SELECT id, author_id, author_name, text, created_at FROM chat_messages WHERE chat_id = ? AND compacted_at IS NULL AND flags = 0 ORDER BY id DESC LIMIT ?",
    )
    .bind(chat_id)
    .bind(LOAD_LIMIT)
    .fetch_all(pool)
    .await
    .map_err(|_| "database".to_string())?;
    if rows.is_empty() {
        return Ok(false);
    }
    let now = unix_now();
    let keep = if rows.len() > KEEP_FRESH { KEEP_FRESH } else { KEEP_NEWEST };
    let keep_ids: Vec<i64> = rows.iter().take(keep).map(|row| row.0).collect();
    let cutoff_old = now - COMPACT_AFTER_SECS;
    let cutoff_fresh = now - 60 * 60;
    let mut candidates: Vec<_> = rows.into_iter()
        .filter(|row| !keep_ids.contains(&row.0))
        .filter(|row| {
            if row.4 < cutoff_old {
                true
            } else {
                keep == KEEP_FRESH && row.4 < cutoff_fresh
            }
        })
        .collect();
    if candidates.is_empty() {
        return Ok(false);
    }
    candidates.reverse();
    let mut selected = Vec::new();
    let mut log = String::new();
    for (id, author_id, author_name, text, _created_at) in candidates {
        let text: String = text.chars().take(COMPACT_MESSAGE_CHARS).collect();
        let line = format_line(&author_name, author_id, &text);
        if selected.len() >= COMPACT_BATCH_MESSAGES
            || (!log.is_empty() && log.chars().count() + line.chars().count() + 1 > COMPACT_BATCH_CHARS)
        {
            break;
        }
        log.push_str(&line);
        log.push('\n');
        selected.push(id);
    }
    if selected.is_empty() {
        return Ok(false);
    }
    let previous: Option<(String,)> =
        sqlx::query_as("SELECT summary FROM chat_summaries WHERE chat_id = ?")
            .bind(chat_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| "database".to_string())?;
    let previous = previous.map(|(value,)| value).unwrap_or_default();
    let summary = match chat.summarize(&previous, &log).await {
        Ok(summary) => summary,
        Err(reason) if reason == "empty" => {
            if previous.trim().is_empty() {
                eprintln!("Memory summary empty for chat {chat_id}; batch left uncompacted");
                return Ok(false);
            }
            previous.clone()
        }
        Err(reason) => return Err(reason),
    };
    let summary = match validate_summary(&summary) {
        Ok(summary) => summary,
        Err(reason) => {
            eprintln!("Memory summary rejected for chat {chat_id}: {reason}");
            if previous.trim().is_empty() {
                return Ok(false);
            }
            previous.trim().to_owned()
        }
    };
    let through = *selected.iter().max().unwrap_or(&0);
    let mut tx = pool.begin().await.map_err(|_| "database".to_string())?;
    sqlx::query(
        "INSERT INTO chat_summaries (chat_id, summary, through_message_id, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(chat_id) DO UPDATE SET summary = excluded.summary, through_message_id = excluded.through_message_id, updated_at = excluded.updated_at",
    )
    .bind(chat_id)
    .bind(&summary)
    .bind(through)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|_| "database".to_string())?;
    for chunk in selected.chunks(100) {
        let placeholders = std::iter::repeat_n("?", chunk.len()).collect::<Vec<_>>().join(",");
        let query = format!("UPDATE chat_messages SET compacted_at = ? WHERE id IN ({placeholders})");
        let mut request = sqlx::query(&query).bind(now);
        for id in chunk {
            request = request.bind(id);
        }
        request.execute(&mut *tx).await.map_err(|_| "database".to_string())?;
    }
    tx.commit().await.map_err(|_| "database".to_string())?;
    Ok(true)
}

pub(crate) async fn memory_compactor(cfg: Arc<Settings>) {
    loop {
        if let Some(chat) = cfg.chat.as_ref() {
            let chats: Vec<i64> = sqlx::query_scalar(
                "SELECT DISTINCT chat_id FROM chat_messages WHERE compacted_at IS NULL AND flags = 0",
            )
            .fetch_all(&cfg.database)
            .await
            .unwrap_or_default();
            for chat_id in chats {
                let lock = cfg.chat_lock(chat_id).await;
                let _guard = lock.lock().await;
                for _ in 0..COMPACT_PASSES {
                    match compact_chat(chat, &cfg.database, chat_id).await {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(reason) => {
                            eprintln!("Memory compact failed for chat {chat_id}: {reason}");
                            break;
                        }
                    }
                }
            }
        }
        time::sleep(Duration::from_secs(COMPACT_INTERVAL_SECS)).await;
    }
}

#[allow(dead_code)]
pub(crate) type ChatLock = Arc<Mutex<()>>;

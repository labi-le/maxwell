use crate::memory::unix_now;
use crate::prelude::*;
use crate::settings::Settings;
use serde_json::json;

pub(crate) use crate::improve_commands::*;
pub(crate) use crate::improve_store::*;
pub(crate) use crate::improve_views::*;

const IMPROVEMENT_POLL: Duration = Duration::from_secs(5);

/// Telegram handlers and the notifier talk to the queue from async code, so
/// every storage call hops off the Tokio workers.
async fn run_blocking<T, F>(task: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task).await.ok()
}

pub(crate) async fn handle_improve(
    bot: &Bot,
    message: &Message,
    cfg: &Settings,
    text: &str,
) -> bool {
    let Some(rest) = command_goal(text, &cfg.bot_username) else {
        return false;
    };
    if !message.chat.is_private()
        || !message
            .from
            .as_ref()
            .is_some_and(|user| user.id == cfg.owner_id)
    {
        return true;
    }
    let as_owner_message = |reply: String| async move {
        let _ = bot
            .send_message(message.chat.id, reply)
            .disable_notification(true)
            .await;
    };
    if message.forward_origin().is_some() {
        as_owner_message("Форварды не используются для решений по очереди.".to_owned()).await;
        return true;
    }
    let dir = queue_dir(cfg);
    let owner = cfg.owner_id;
    if rest == "off" {
        let write_dir = dir.clone();
        let reply = if run_blocking(move || write_off_marker(&write_dir))
            .await
            .unwrap_or(false)
        {
            "Самоулучшение выключено.".to_owned()
        } else {
            "Не удалось выключить самоулучшение.".to_owned()
        };
        as_owner_message(reply).await;
        return true;
    }
    // Approve/reject/cancel/priority are command words, never goals. Even a
    // malformed id must not fall through and create an inbox task.
    let (head, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let tail = tail.trim();
    if action_word(&clean_word(head.split(':').next().unwrap_or(head))).is_some() {
        let command_text = rest.replacen(':', " ", 1);
        let reply = match parse_owner_command(&command_text) {
            Some(command) => {
                let command_dir = dir.clone();
                run_blocking(move || apply_owner_command(&command_dir, owner, command))
                    .await
                    .unwrap_or_else(|| "Не смог обработать команду.".to_owned())
            }
            None => "Укажи действие и id: /improve approve|reject|cancel|priority <id>.".to_owned(),
        };
        as_owner_message(reply).await;
        return true;
    }
    if head == "status" || head.starts_with("status:") {
        let id = if head == "status" {
            tail
        } else {
            head.strip_prefix("status:").unwrap_or("").trim()
        };
        let id = id_token(id);
        let reply = if !valid_id(id) {
            "Укажите корректный id: /improve status <id>.".to_owned()
        } else {
            let id = id.to_owned();
            let status_dir = dir.clone();
            let snapshot = run_blocking(move || status_snapshot(&status_dir, owner, Some(&id[..])))
                .await
                .unwrap_or(Value::Null);
            let key = cfg
                .chat
                .as_ref()
                .map(|chat| chat.api_key.as_str())
                .unwrap_or("");
            redact_secret(&format_snapshot(&snapshot), key)
        };
        as_owner_message(reply).await;
        return true;
    }
    let goal = rest;
    let reply = if goal.is_empty() {
        "Используйте /improve <задача>.".to_owned()
    } else if goal.chars().count() > MAX_GOAL_CHARS {
        format!("Задача длиннее {MAX_GOAL_CHARS} символов.")
    } else {
        let id = format!("{}-{}", unix_now(), message.id.0);
        let task = json!({
            "id": id,
            "requested_by": owner.0,
            "chat_id": message.chat.id.0,
            "goal": goal,
            "created_at": unix_now(),
        });
        let body = task.to_string();
        let path = dir.join("inbox").join(format!("{id}.json"));
        match run_blocking(move || atomic_write(&path, body.as_bytes())).await {
            Some(Ok(())) => format!("принял, задача #{id}"),
            _ => "Не удалось записать задачу.".to_owned(),
        }
    };
    as_owner_message(reply).await;
    true
}

pub(crate) async fn handle_owner_text(
    bot: &Bot,
    message: &Message,
    cfg: &Settings,
    text: &str,
) -> bool {
    if !message.chat.is_private()
        || !message
            .from
            .as_ref()
            .is_some_and(|user| user.id == cfg.owner_id)
        || message.forward_origin().is_some()
    {
        return false;
    }
    let Some(command) = parse_owner_command(text) else {
        return false;
    };
    let dir = queue_dir(cfg);
    let owner = cfg.owner_id;
    let reply = run_blocking(move || apply_owner_command(&dir, owner, command))
        .await
        .unwrap_or_else(|| "Не смог обработать команду.".to_owned());
    let _ = bot
        .send_message(message.chat.id, reply)
        .disable_notification(true)
        .await;
    true
}

fn format_snapshot(value: &Value) -> String {
    let Some(task) = value
        .get("tasks")
        .and_then(Value::as_array)
        .and_then(|tasks| tasks.first())
    else {
        return "Задача не найдена.".to_owned();
    };
    format_status(task)
}

pub(crate) fn notification_text(value: &Value) -> Option<String> {
    let id = value.get("id").and_then(Value::as_str)?;
    let status = value.get("status").and_then(Value::as_str)?;
    let goal = value.get("goal").and_then(Value::as_str).unwrap_or("");
    let proposer = value
        .get("proposed_by_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let proposed_by = value
        .get("proposed_by")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let owner_id = value
        .get("requested_by")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let attribution = if proposed_by != 0 && proposed_by != owner_id && !proposer.is_empty() {
        format!("предложил: {proposer}\n")
    } else {
        String::new()
    };
    let mut text = match status {
        "planned" => {
            let issue = value.get("issue_url").and_then(Value::as_str).unwrap_or("");
            if issue.is_empty() {
                format!("план #{id} готов\n{attribution}цель: {goal}\nissue не создалась, спроси, что с задачей\n\nделаем или в помойку?")
            } else {
                format!(
                    "план #{id} готов\n{attribution}цель: {goal}\n{issue}\n\nделаем или в помойку?"
                )
            }
        }
        "proposed" => format!(
            "{}{}\n\nапрув на деплой?",
            attribution,
            format_status(value)
        ),
        "deployed" => format!("задача #{id} задеплоена"),
        "failed" => format!(
            "задача #{id} провалилась: {}",
            value
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("без деталей")
        ),
        "rejected" => format!("задача #{id} отклонена"),
        _ => return None,
    };
    if text.chars().count() > 24000 {
        text = text.chars().take(24000).collect();
        text.push_str("\n...[обрезано]");
    }
    Some(text)
}

pub(crate) fn notification_markup(value: &Value) -> Option<teloxide::types::InlineKeyboardMarkup> {
    let status = value.get("status").and_then(Value::as_str)?;
    let gate = match status {
        "planned" => DecisionGate::Plan,
        "proposed" => DecisionGate::Deploy,
        _ => return None,
    };
    let id = value.get("id").and_then(Value::as_str)?;
    decision_markup(&PendingDecision {
        id: id.to_owned(),
        gate,
    })
}

struct QueuedUpdate {
    marker: PathBuf,
    chat_id: i64,
    status: String,
    value: Value,
}

fn collect_updates(dir: &Path) -> Vec<QueuedUpdate> {
    let Ok(entries) = std::fs::read_dir(dir.join("outbox")) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    let notified = dir.join("notified");
    let mut updates = Vec::new();
    for path in paths {
        let Some(value) = read_json(&path) else {
            continue;
        };
        let Some(chat_id) = value
            .get("chat_id")
            .and_then(Value::as_i64)
            .filter(|id| *id != 0)
        else {
            continue;
        };
        let Some(status) = value.get("status").and_then(Value::as_str) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        let marker = notified.join(format!("{id}-{status}"));
        if marker.exists() {
            continue;
        }
        updates.push(QueuedUpdate {
            marker,
            chat_id,
            status: status.to_owned(),
            value,
        });
    }
    updates
}

/// Polls the outbox and sends plan/proposal/deploy updates to the chat that
/// started the task. The orchestrator never talks to Telegram directly.
pub(crate) async fn improvement_notifier(bot: Bot, cfg: Arc<Settings>) {
    loop {
        let dir = cfg.self_improve_dir.clone();
        let updates = run_blocking(move || collect_updates(&dir))
            .await
            .unwrap_or_default();
        for update in updates {
            let Some(mut text) = notification_text(&update.value) else {
                continue;
            };
            let key = cfg
                .chat
                .as_ref()
                .map(|chat| chat.api_key.as_str())
                .unwrap_or("");
            if !key.is_empty() && text.contains(key) {
                eprintln!("Improvement update withheld: credential in output");
            } else {
                let markup = notification_markup(&update.value);
                if matches!(update.status.as_str(), "planned" | "proposed") && markup.is_none() {
                    text.push_str("\nкнопка недоступна для этого id");
                }
                if !send_update(&bot, update.chat_id, &text, markup).await {
                    continue;
                }
            }
            let marker = update.marker;
            let _ = run_blocking(move || atomic_write(&marker, b"1")).await;
        }
        time::sleep(IMPROVEMENT_POLL).await;
    }
}

async fn send_update(
    bot: &Bot,
    chat_id: i64,
    text: &str,
    markup: Option<teloxide::types::InlineKeyboardMarkup>,
) -> bool {
    for (index, chunk) in crate::chat_style::split_message(text)
        .into_iter()
        .enumerate()
    {
        let mut request = bot
            .send_message(ChatId(chat_id), chunk)
            .disable_notification(true);
        if index == 0 {
            if let Some(markup) = markup.clone() {
                request = request.reply_markup(markup);
            }
        }
        if request.await.is_err() {
            return false;
        }
    }
    true
}

pub(crate) async fn handle_callback(
    bot: &Bot,
    query: &teloxide::types::CallbackQuery,
    cfg: &Settings,
) -> bool {
    let answer = |text: &'static str| async move {
        let _ = bot.answer_callback_query(query.id.clone()).text(text).await;
    };
    if query.from.id != cfg.owner_id {
        answer("только владелец решает").await;
        return true;
    }
    let Some(data) = query.data.as_deref() else {
        return true;
    };
    let Some((action, id)) = data.split_once(':') else {
        return true;
    };
    if !valid_id(id) {
        answer("не понял кнопку").await;
        return true;
    }
    let dir = queue_dir(cfg);
    let owner = cfg.owner_id;
    let id = id.to_owned();
    let action = action.to_owned();
    let decision = run_blocking(move || {
        let Some((value, _)) = find_task(&dir, &id) else {
            return "задача уже неактуальна";
        };
        let status = value.get("status").and_then(Value::as_str).unwrap_or("");
        if !matches!(status, "planned" | "proposed") {
            return "задача уже неактуальна";
        }
        match record_decision(&dir, &id, status, &action, owner) {
            Ok(text) | Err(text) => text,
        }
    })
    .await
    .unwrap_or("не понял кнопку");
    answer(decision).await;
    true
}

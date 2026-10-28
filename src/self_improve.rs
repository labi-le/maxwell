use crate::prelude::*;
use crate::*;
use serde_json::{json, Value};

const MAX_GOAL_CHARS: usize = 2000;
const MAX_STATUS_CHARS: usize = 3200;
const IMPROVEMENT_POLL: Duration = Duration::from_secs(5);

fn command_goal<'a>(text: &'a str, username: &str) -> Option<&'a str> {
    let mut parts = text.splitn(2, char::is_whitespace);
    let name = parts.next()?;
    let addressed = name == "/improve" || name.strip_prefix("/improve@")
        .is_some_and(|target| !target.is_empty() && target.eq_ignore_ascii_case(username));
    addressed.then(|| parts.next().unwrap_or("").trim())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn queue_dir(cfg: &Settings) -> PathBuf {
    cfg.self_improve_dir.clone()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(tmp, path)
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn approval_intent(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let words: Vec<&str> = lower.split(|ch: char| !ch.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
    let short = words.len() <= 2;
    ["да", "ага", "угу", "конечно", "делаем", "погнали", "поехали", "запускай", "го", "ок", "окей",
        "согласен", "согласна", "утверждаю", "делай", "начинай", "апрув", "approve", "yes"]
        .iter().any(|trigger| words.contains(trigger))
        || (short && words.contains(&"давай"))
}

fn rejection_intent(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let words: Vec<&str> = lower.split(|ch: char| !ch.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
    ["нет", "отмена", "отменяю", "отклоняю", "стоп", "удал", "нахуй", "не надо", "не делай"].iter().any(|trigger| {
        trigger.contains(' ') && lower.contains(trigger) || words.contains(trigger)
    })
}

fn refinement_intent(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["доработ", "поправ", "передел", "измени", "не так", "уточн", "пересмотр", "добавь в план",
        "слишком", "лучше сделать", "иначе"].iter().any(|word| lower.contains(word))
}

fn plan_id(now: i64, source_chat_id: i64, message_id: i32) -> String {
    format!("plan-{now}-{}-{message_id}", source_chat_id.unsigned_abs())
}

fn queue_plan(
    dir: &Path,
    owner: UserId,
    chat_id: i64,
    source_chat_id: i64,
    proposed_by: UserId,
    proposer: &str,
    message_id: i32,
    goal: &str,
    parent: Option<&str>,
    feedback: &str,
    previous_spec: &str,
    previous_review: &str,
) -> std::io::Result<String> {
    if goal.chars().count() > MAX_GOAL_CHARS {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "goal too long"));
    }
    let id = plan_id(unix_now(), source_chat_id, message_id);
    let task = json!({
        "id": id,
        "requested_by": owner.0,
        "chat_id": chat_id,
        "source_chat_id": source_chat_id,
        "proposed_by": proposed_by.0,
        "proposed_by_name": proposer,
        "goal": goal,
        "stage": "plan",
        "parent": parent.unwrap_or(""),
        "feedback": feedback,
        "previous_spec": previous_spec,
        "previous_review": previous_review,
        "created_at": unix_now(),
    });
    atomic_write(&dir.join("inbox").join(format!("{id}.json")), task.to_string().as_bytes())?;
    Ok(id)
}

/// Queue a feature request from the model's propose_feature tool. The tool
/// only creates a plan; approval and execution stay with the owner.
pub(crate) fn queue_feature(
    dir: &Path,
    owner: UserId,
    source_chat_id: i64,
    proposed_by: UserId,
    proposer: &str,
    message_id: i32,
    goal: &str,
) -> Result<String, String> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err("пустая заявка".to_owned());
    }
    if goal.chars().count() > MAX_GOAL_CHARS {
        return Err("заявка слишком длинная".to_owned());
    }
    if proposed_by != owner && has_pending_proposal(dir, proposed_by) {
        return Err("уже есть активная заявка".to_owned());
    }
    let notify_chat = if proposed_by == owner { source_chat_id } else { owner.0 as i64 };
    queue_plan(dir, owner, notify_chat, source_chat_id, proposed_by, proposer, message_id, goal, None, "", "", "")
        .map_err(|_| "не смог поставить план в очередь".to_owned())
}

fn latest_pending(dir: &Path, chat_id: i64) -> Option<Value> {
    let entries = std::fs::read_dir(dir.join("outbox")).ok()?;
    entries.filter_map(Result::ok)
        .filter_map(|entry| read_json(&entry.path()))
        .filter(|value| {
            value.get("chat_id").and_then(Value::as_i64) == Some(chat_id)
                && matches!(value.get("status").and_then(Value::as_str), Some("planned" | "proposed"))
        })
        .max_by_key(|value| value.get("created_at").and_then(Value::as_i64).unwrap_or(0))
}

fn has_pending_proposal(dir: &Path, proposer: UserId) -> bool {
    let Ok(entries) = std::fs::read_dir(dir.join("outbox")) else { return false; };
    entries.filter_map(Result::ok)
        .filter_map(|entry| read_json(&entry.path()))
        .any(|value| {
            value.get("proposed_by").and_then(Value::as_i64) == Some(proposer.0 as i64)
                && matches!(value.get("status").and_then(Value::as_str), Some("planning" | "planned" | "proposed"))
        })
}

fn latest_pending_any(dir: &Path, owner: UserId) -> Option<Value> {
    let entries = std::fs::read_dir(dir.join("outbox")).ok()?;
    entries.filter_map(Result::ok)
        .filter_map(|entry| read_json(&entry.path()))
        .filter(|value| {
            value.get("requested_by").and_then(Value::as_i64) == Some(owner.0 as i64)
                && matches!(value.get("status").and_then(Value::as_str), Some("planned" | "proposed"))
        })
        .max_by_key(|value| value.get("created_at").and_then(Value::as_i64).unwrap_or(0))
}

fn latest_active(dir: &Path, chat_id: i64) -> Option<Value> {
    let entries = std::fs::read_dir(dir.join("outbox")).ok()?;
    entries.filter_map(Result::ok)
        .filter_map(|entry| read_json(&entry.path()))
        .filter(|value| {
            value.get("chat_id").and_then(Value::as_i64) == Some(chat_id)
                && matches!(value.get("status").and_then(Value::as_str), Some("planning" | "planned" | "proposed" | "failed"))
        })
        .max_by_key(|value| value.get("created_at").and_then(Value::as_i64).unwrap_or(0))
}

fn latest_active_any(dir: &Path, owner: UserId) -> Option<Value> {
    let entries = std::fs::read_dir(dir.join("outbox")).ok()?;
    entries.filter_map(Result::ok)
        .filter_map(|entry| read_json(&entry.path()))
        .filter(|value| {
            value.get("requested_by").and_then(Value::as_i64) == Some(owner.0 as i64)
                && matches!(value.get("status").and_then(Value::as_str), Some("planning" | "planned" | "proposed" | "failed"))
        })
        .max_by_key(|value| value.get("created_at").and_then(Value::as_i64).unwrap_or(0))
}

pub(crate) fn status_question(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    ["статус", "готов", "долго", "как там", "что там", "дума", "план"]
        .iter().any(|word| lower.contains(word))
        || (lower.contains("сколько")
            && ["ещё", "еще", "дума", "план", "готов"].iter().any(|word| lower.contains(word)))
}

fn plan_wait_text(value: &Value) -> String {
    let id = value.get("id").and_then(Value::as_str).unwrap_or("?");
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    let created = value.get("created_at").and_then(Value::as_i64).unwrap_or(0);
    let minutes = if created > 0 { (unix_now() - created).max(0) / 60 } else { 0 };
    let age = if minutes > 0 { format!("~{minutes} мин") } else { "меньше минуты".to_owned() };
    match status {
        "planning" => format!("план #{id} ещё готовится: planner и reviewer читают код, прошло {age}"),
        "planned" => {
            let link = value.get("issue_url").and_then(Value::as_str).filter(|url| !url.is_empty());
            match link {
                Some(url) => format!("план #{id} готов
{url}

делаем или в помойку?"),
                None => format!("план #{id} готов, жду решения"),
            }
        }
        "proposed" => format!("план #{id} готов, ждёт подтверждения деплоя"),
        "failed" => {
            let summary = value.get("summary").and_then(Value::as_str).unwrap_or("не получилось");
            format!("план #{id} не получился: {summary}")
        }
        _ => format!("план #{id}: {status}"),
    }
}

fn approval_dir(status: &str) -> &'static str {
    if status == "planned" { "plan-approvals" } else { "approvals" }
}

fn marker(id: &str, owner: UserId) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&json!({"id": id, "requested_by": owner.0, "approved_at": unix_now()}))
}

fn reply_to_bot(message: &Message, cfg: &Settings) -> bool {
    message.reply_to_message()
        .and_then(|reply| reply.from.as_ref())
        .is_some_and(|user| user.is_bot
            && user.username.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(&cfg.bot_username)))
}

async fn send_quiet(bot: &Bot, chat_id: ChatId, text: &str) {
    let _ = bot.send_message(chat_id, text).disable_notification(true).await;
}

/// Handles a pending plan's approval, refinement, or rejection, plus status
/// questions. Feature requests themselves arrive through the model tool.
pub(crate) async fn handle_natural_improvement(
    bot: &Bot,
    message: &Message,
    cfg: &Settings,
    text: &str,
) -> bool {
    let Some(sender) = message.from.as_ref().filter(|user| !user.is_bot) else {
        return false;
    };
    let owner = sender.id == cfg.owner_id;
    let proposer = sender.username.clone().unwrap_or_else(|| sender.first_name.clone());
    let private = message.chat.is_private();
    if !private && !message.chat.is_group() && !message.chat.is_supergroup() {
        return false;
    }
    if !private && !group_enabled(&cfg.database, message.chat.id).await.is_ok_and(|enabled| enabled) {
        return false;
    }
    let dir = queue_dir(cfg);
    let pending = latest_pending(&dir, message.chat.id.0)
        .or_else(|| owner.then(|| latest_pending_any(&dir, sender.id)).flatten());
    let active = latest_active(&dir, message.chat.id.0)
        .or_else(|| owner.then(|| latest_active_any(&dir, sender.id)).flatten());
    let addressed = crate::chat::chat_prompt(text, private, &cfg.bot_username, reply_to_bot(message, cfg));
    let prompt = match addressed {
        Some(prompt) => prompt,
        // The owner may dictate a feature or answer a pending plan in a group
        // without prefixing the bot name. Others still have to address it.
        None if owner && (pending.is_some() || (status_question(text) && active.is_some())) => text.trim().to_owned(),
        None => return false,
    };
    let short_reply = prompt.split_whitespace().count() <= 3;
    if owner {
        if let Some(value) = active.as_ref() {
            let status = value.get("status").and_then(Value::as_str).unwrap_or("");
            let waiting_on_plan = matches!(status, "planning" | "failed")
                && short_reply
                && (approval_intent(&prompt) || rejection_intent(&prompt) || refinement_intent(&prompt));
            if status_question(&prompt) || waiting_on_plan {
                send_quiet(bot, message.chat.id, &plan_wait_text(value)).await;
                return true;
            }
        }
    }
    if !owner {
        return false;
    }
    let Some(value) = pending else { return false; };
    let id = value.get("id").and_then(Value::as_str).unwrap_or("");
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    if !valid_id(id) {
        return false;
    }
    if refinement_intent(&prompt) {
        let previous_spec = value.get("spec").and_then(Value::as_str).unwrap_or("");
        let previous_review = value.get("review").and_then(Value::as_str).unwrap_or("");
        let goal = value.get("goal").and_then(Value::as_str).unwrap_or("");
        let _ = atomic_write(&dir.join("rejections").join(format!("{id}.json")), b"reject");
        match queue_plan(&dir, cfg.owner_id, message.chat.id.0, message.chat.id.0, sender.id, &proposer, message.id.0, goal, Some(id), &prompt, previous_spec, previous_review) {
            Ok(new_id) => send_quiet(bot, message.chat.id, &format!("принял, переделываю план #{new_id}")).await,
            Err(_) => send_quiet(bot, message.chat.id, "не смог поставить план в очередь").await,
        }
        return true;
    }
    if short_reply && rejection_intent(&prompt) {
        let _ = atomic_write(&dir.join("rejections").join(format!("{id}.json")), b"reject");
        send_quiet(bot, message.chat.id, &format!("ок, отменил #{id}")).await;
        return true;
    }
    if short_reply && approval_intent(&prompt) {
        let bytes = match marker(id, sender.id) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };
        match atomic_write(&dir.join(approval_dir(status)).join(format!("{id}.json")), &bytes) {
            Ok(()) if status == "planned" => send_quiet(bot, message.chat.id, &format!("погнали, задача #{id}")).await,
            Ok(()) => send_quiet(bot, message.chat.id, &format!("задеплою #{id}, когда проверка пройдёт")).await,
            Err(_) => send_quiet(bot, message.chat.id, "не смог записать подтверждение").await,
        }
        return true;
    }
    false
}

pub(crate) async fn handle_improve(
    bot: &Bot,
    message: &Message,
    cfg: &Settings,
    text: &str,
) -> bool {
    let Some(rest) = command_goal(text, &cfg.bot_username) else { return false };
    if !message.chat.is_private()
        || !message.from.as_ref().is_some_and(|user| user.id == cfg.owner_id)
    {
        return true;
    }
    let dir = queue_dir(cfg);
    if std::fs::create_dir_all(dir.join("inbox")).is_err() {
        let _ = bot.send_message(message.chat.id, "Не удалось открыть очередь самоулучшения.")
            .disable_notification(true).await;
        return true;
    }
    let reply = if rest == "off" {
        if atomic_write(&dir.join("off"), b"stop").is_ok() {
            "Самоулучшение выключено.".to_owned()
        } else {
            "Не удалось выключить самоулучшение.".to_owned()
        }
    } else if rest == "status" {
        "Используйте /improve status <id>.".to_owned()
    } else if let Some(id) = rest.strip_prefix("status ").filter(|value| valid_id(value.trim())) {
        let id = id.trim();
        if !valid_id(id) {
            "Используйте /improve status <id>.".to_owned()
        } else {
            match read_json(&dir.join("outbox").join(format!("{id}.json"))) {
                Some(value) => format_status(&value),
                None => format!("Задача #{id} не найдена."),
            }
        }
    } else if rest == "approve" {
        "Используйте /improve approve <id>.".to_owned()
    } else if let Some(id) = rest.strip_prefix("approve ").filter(|value| valid_id(value.trim())) {
        let id = id.trim();
        if !valid_id(id) {
            "Используйте /improve approve <id>.".to_owned()
        } else if let Some(value) = read_json(&dir.join("outbox").join(format!("{id}.json"))) {
            let status = value.get("status").and_then(Value::as_str).unwrap_or("");
            if !matches!(status, "planned" | "proposed") {
                format!("Задача #{id} ещё не готова к деплою.")
            } else {
                match marker(id, cfg.owner_id).and_then(|bytes| {
                    atomic_write(&dir.join(approval_dir(status)).join(format!("{id}.json")), &bytes)
                        .map_err(|_| serde_json::Error::io(std::io::Error::other("write")))
                }) {
                    Ok(()) if status == "planned" => format!("Задача #{id} одобрена, делаю реализацию."),
                    Ok(()) => format!("Задача #{id} одобрена, деплой запущен."),
                    Err(_) => "Не удалось записать одобрение.".to_owned(),
                }
            }
        } else {
            format!("Задача #{id} ещё не готова к деплою.")
        }
    } else if rest == "reject" {
        "Используйте /improve reject <id>.".to_owned()
    } else if let Some(id) = rest.strip_prefix("reject ").filter(|value| valid_id(value.trim())) {
        let id = id.trim();
        if !valid_id(id) {
            "Используйте /improve reject <id>.".to_owned()
        } else {
            match atomic_write(&dir.join("rejections").join(format!("{id}.json")), b"reject") {
                Ok(()) => format!("Задача #{id} отклонена."),
                Err(_) => "Не удалось записать отказ.".to_owned(),
            }
        }
    } else {
        let goal = rest;
        if goal.is_empty() {
            "Используйте /improve <задача>.".to_owned()
        } else if goal.chars().count() > MAX_GOAL_CHARS {
            format!("Задача длиннее {MAX_GOAL_CHARS} символов.")
        } else {
            let id = format!("{}-{}", unix_now(), message.id.0);
            let task = json!({
                "id": id,
                "requested_by": cfg.owner_id.0,
                "chat_id": message.chat.id.0,
                "goal": goal,
                "created_at": unix_now(),
            });
            match atomic_write(&dir.join("inbox").join(format!("{id}.json")), task.to_string().as_bytes()) {
                Ok(()) => format!("принял, задача #{id}"),
                Err(_) => "Не удалось записать задачу.".to_owned(),
            }
        }
    };
    let _ = bot.send_message(message.chat.id, reply).disable_notification(true).await;
    true
}

fn format_status(value: &Value) -> String {
    let status = value.get("status").and_then(Value::as_str).unwrap_or("unknown");
    let mut out = format!("задача #{}: {status}", value.get("id").and_then(Value::as_str).unwrap_or("?"));
    if let Some(goal) = value.get("goal").and_then(Value::as_str) {
        out.push_str(&format!("\nцель: {goal}"));
    }
    if let Some(summary) = value.get("summary").and_then(Value::as_str) {
        out.push_str(&format!("\n{summary}"));
    }
    if let Some(files) = value.get("files").and_then(Value::as_array) {
        let names: Vec<_> = files.iter().filter_map(Value::as_str).take(10).collect();
        if !names.is_empty() {
            out.push_str(&format!("\nфайлы: {}", names.join(", ")));
        }
    }
    if let Some(tests) = value.get("tests").and_then(Value::as_str) {
        out.push_str(&format!("\nтесты: {tests}"));
    }
    if let Some(issue) = value.get("issue_url").and_then(Value::as_str).filter(|url| !url.is_empty()) {
        out.push_str(&format!("\nissue: {issue}"));
    }
    if out.chars().count() > MAX_STATUS_CHARS {
        out = out.chars().take(MAX_STATUS_CHARS).collect();
        out.push_str("...");
    }
    out
}

fn notification_text(value: &Value) -> Option<String> {
    let id = value.get("id").and_then(Value::as_str)?;
    let status = value.get("status").and_then(Value::as_str)?;
    let goal = value.get("goal").and_then(Value::as_str).unwrap_or("");
    let proposer = value.get("proposed_by_name").and_then(Value::as_str).unwrap_or("");
    let proposed_by = value.get("proposed_by").and_then(Value::as_i64).unwrap_or(0);
    let owner_id = value.get("requested_by").and_then(Value::as_i64).unwrap_or(0);
    let attribution = if proposed_by != 0 && proposed_by != owner_id && !proposer.is_empty() {
        format!("предложил: {proposer}\n")
    } else {
        String::new()
    };
    let mut text = match status {
        "planned" => {
            let issue = value.get("issue_url").and_then(Value::as_str).unwrap_or("");
            if issue.is_empty() {
                format!("план #{id} готов\n{attribution}цель: {goal}\nissue не создалась, скажи «статус»\n\nделаем или в помойку?")
            } else {
                format!("план #{id} готов\n{attribution}цель: {goal}\n{issue}\n\nделаем или в помойку?")
            }
        }
        "proposed" => format!("{}{}\n\nапрув на деплой?", attribution, format_status(value)),
        "deployed" => format!("задача #{id} задеплоена"),
        "failed" => format!("задача #{id} провалилась: {}", value.get("summary").and_then(Value::as_str).unwrap_or("без деталей")),
        _ => return None,
    };
    if text.chars().count() > 24000 {
        text = text.chars().take(24000).collect();
        text.push_str("\n...[обрезано]");
    }
    Some(text)
}

fn notification_markup(value: &Value) -> Option<teloxide::types::InlineKeyboardMarkup> {
    if value.get("status").and_then(Value::as_str) != Some("planned") {
        return None;
    }
    let id = value.get("id").and_then(Value::as_str)?;
    let approve = format!("approve:{id}");
    let reject = format!("reject:{id}");
    if approve.len() > 64 || reject.len() > 64 {
        return None;
    }
    Some(teloxide::types::InlineKeyboardMarkup::new(vec![vec![
        teloxide::types::InlineKeyboardButton::callback("Делаем", approve),
        teloxide::types::InlineKeyboardButton::callback("В помойку", reject),
    ]]))
}

/// Polls the outbox and sends plan/proposal/deploy updates to the chat that
/// started the task. The orchestrator never talks to Telegram directly.
pub(crate) async fn improvement_notifier(bot: Bot, cfg: Arc<Settings>) {
    loop {
        if !cfg.self_improve_dir.join("outbox").is_dir() {
            time::sleep(IMPROVEMENT_POLL).await;
            continue;
        }
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(cfg.self_improve_dir.join("outbox")) {
            Ok(entries) => entries.filter_map(Result::ok).map(|entry| entry.path()).collect(),
            Err(_) => vec![],
        };
        entries.sort();
        for path in entries {
            let Some(value) = read_json(&path) else { continue };
            let Some(chat_id) = value.get("chat_id").and_then(Value::as_i64).filter(|id| *id != 0) else { continue };
            let Some(status) = value.get("status").and_then(Value::as_str) else { continue };
            let Some(id) = value.get("id").and_then(Value::as_str) else { continue };
            let key = format!("{id}-{status}");
            let marker = cfg.self_improve_dir.join("notified").join(&key);
            if marker.exists() { continue; }
            let Some(text) = notification_text(&value) else { continue };
            let key = cfg.chat.as_ref().map(|chat| chat.api_key.as_str()).unwrap_or("");
            if !key.is_empty() && text.contains(key) {
                eprintln!("Improvement update withheld: credential in output");
                let _ = atomic_write(&marker, b"1");
                continue;
            }
            let mut sent = true;
            let markup = notification_markup(&value);
            for (index, chunk) in crate::chat::split_message(&text).into_iter().enumerate() {
                let mut request = bot.send_message(ChatId(chat_id), chunk).disable_notification(true);
                if index == 0 {
                    if let Some(markup) = markup.clone() {
                        request = request.reply_markup(markup);
                    }
                }
                if request.await.is_err() {
                    sent = false;
                    break;
                }
            }
            if sent {
                let _ = atomic_write(&marker, b"1");
            }
        }
        time::sleep(IMPROVEMENT_POLL).await;
    }
}

pub(crate) async fn handle_callback(bot: &Bot, query: &teloxide::types::CallbackQuery, cfg: &Settings) -> bool {
    let answer = |text: &'static str| async move {
        let _ = bot.answer_callback_query(query.id.clone()).text(text).await;
    };
    if query.from.id != cfg.owner_id {
        answer("только владелец решает").await;
        return true;
    }
    let Some(data) = query.data.as_deref() else { return true };
    let Some((action, id)) = data.split_once(':') else { return true };
    if !valid_id(id) {
        answer("не понял кнопку").await;
        return true;
    }
    let dir = queue_dir(cfg);
    let Some(value) = read_json(&dir.join("outbox").join(format!("{id}.json"))) else {
        answer("задача уже неактуальна").await;
        return true;
    };
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    let result = match action {
        "approve" if matches!(status, "planned" | "proposed") => {
            marker(id, cfg.owner_id)
                .map_err(|_| ())
                .and_then(|bytes| atomic_write(&dir.join(approval_dir(status)).join(format!("{id}.json")), &bytes).map_err(|_| ()))
                .map(|()| if status == "planned" { "погнали" } else { "деплой запущен" })
        }
        "reject" if matches!(status, "planned" | "proposed") => {
            atomic_write(&dir.join("rejections").join(format!("{id}.json")), b"reject")
                .map(|()| "в помойку")
                .map_err(|_| ())
        }
        _ => Err(()),
    };
    match result {
        Ok(text) => answer(text).await,
        Err(()) => answer("не понял кнопку").await,
    }
    true
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_notification_names_the_proposer_and_links_the_issue() {
        let value = json!({
            "id": "plan-1",
            "status": "planned",
            "goal": "добавь кнопку",
            "spec": "спека",
            "review": "ревью",
            "issue_url": "https://github.com/labi-le/maxwell/issues/1",
            "requested_by": 1,
            "proposed_by": 2,
            "proposed_by_name": "bob",
        });
        let text = notification_text(&value).unwrap();
        assert!(text.contains("предложил: bob"));
        assert!(text.contains("делаем или в помойку?"));
        assert!(text.contains("https://github.com/labi-le/maxwell/issues/1"));
        assert!(!text.contains("спека"));
        assert!(!text.contains("ревью"));
        let markup = notification_markup(&value).unwrap();
        let labels = serde_json::to_string(&markup).unwrap();
        assert!(labels.contains("Делаем"));
        assert!(labels.contains("В помойку"));
        assert!(!labels.contains('✅'));
        assert!(!labels.contains('🗑'));
    }

    #[test]
    fn notification_ignores_non_terminal_states() {
        let value = json!({"id": "plan-1", "status": "planning"});
        assert!(notification_text(&value).is_none());
    }
}

use crate::improve_store::{
    atomic_write, find_in_tasks, read_queue_tasks, record_decision, short_task_id, task_path,
    valid_id,
};
use crate::prelude::*;
use serde_json::json;
use std::collections::HashSet;

/// A gate that still needs the owner's explicit button press. The model can
/// read these values, but only `handle_callback` may turn one into a marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DecisionGate {
    Plan,
    Deploy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingDecision {
    pub(crate) id: String,
    pub(crate) gate: DecisionGate,
}

/// Pure button builder. Callback data is validated here as well as in the
/// callback handler, so a malformed id can never produce a button.
pub(crate) fn decision_markup(
    decision: &PendingDecision,
) -> Option<teloxide::types::InlineKeyboardMarkup> {
    if !valid_id(&decision.id) {
        return None;
    }
    let approve = format!("approve:{}", decision.id);
    let reject = format!("reject:{}", decision.id);
    if approve.len() > 64 || reject.len() > 64 {
        return None;
    }
    let (approve_label, reject_label) = match decision.gate {
        DecisionGate::Plan => ("Делаем", "В помойку"),
        DecisionGate::Deploy => ("Деплой", "В помойку"),
    };
    Some(teloxide::types::InlineKeyboardMarkup::new(vec![vec![
        teloxide::types::InlineKeyboardButton::callback(approve_label, approve),
        teloxide::types::InlineKeyboardButton::callback(reject_label, reject),
    ]]))
}

pub(crate) fn command_goal<'a>(text: &'a str, username: &str) -> Option<&'a str> {
    let mut parts = text.splitn(2, char::is_whitespace);
    let name = parts.next()?;
    let addressed = name == "/improve"
        || name
            .strip_prefix("/improve@")
            .is_some_and(|target| !target.is_empty() && target.eq_ignore_ascii_case(username));
    addressed.then(|| parts.next().unwrap_or("").trim())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerAction {
    Approve,
    Reject,
    Cancel,
    PriorityTop,
    PriorityBottom,
    PrioritySet(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnerCommand {
    pub(crate) action: OwnerAction,
    pub(crate) targets: Vec<String>,
}

pub(crate) fn action_word(word: &str) -> Option<OwnerAction> {
    match word {
        "одобри" | "одобряю" | "апрув" | "approve" => Some(OwnerAction::Approve),
        "отклони" | "отклоняю" | "reject" => Some(OwnerAction::Reject),
        "отмени" | "отменяю" | "cancel" => Some(OwnerAction::Cancel),
        "подними" | "поднять" | "вверх" | "top" => {
            Some(OwnerAction::PriorityTop)
        }
        "опусти" | "опустить" | "вниз" | "bottom" => {
            Some(OwnerAction::PriorityBottom)
        }
        "приоритет" | "priority" => Some(OwnerAction::PrioritySet(0)),
        _ => None,
    }
}

pub(crate) fn clean_word(word: &str) -> String {
    word.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                ',' | ';' | ':' | '!' | '?' | '.' | '…' | '«' | '»' | '"' | '\''
            )
    })
    .to_lowercase()
}

fn clean_target(word: &str) -> String {
    clean_word(word).trim_start_matches('#').to_owned()
}

pub(crate) fn parse_owner_command(text: &str) -> Option<OwnerCommand> {
    let normalized = text.replacen(':', " ", 1);
    let mut words = normalized.split_whitespace();
    let mut head = clean_word(words.next()?);
    if matches!(head.as_str(), "максут" | "максвелл" | "maksut") {
        head = clean_word(words.next()?);
    }
    let action = action_word(&head)?;
    let mut targets: Vec<String> = words
        .map(clean_target)
        .filter(|target| !target.is_empty())
        .collect();
    if targets.is_empty() {
        return None;
    }
    let action = match action {
        OwnerAction::PrioritySet(_) => {
            let value = (targets.len() > 1)
                .then(|| targets.last().and_then(|last| last.parse::<i64>().ok()))
                .flatten();
            match value {
                Some(value) => {
                    targets.pop();
                    OwnerAction::PrioritySet(value.clamp(-1000, 1000))
                }
                None => OwnerAction::PrioritySet(0),
            }
        }
        other => other,
    };
    Some(OwnerCommand { action, targets })
}

pub(crate) fn display_id(id: &str) -> String {
    format!("#{}", short_task_id(id))
}

pub(crate) enum TaskIdMatch {
    Found(String),
    Ambiguous(Vec<String>),
    Missing,
}

pub(crate) fn id_token(token: &str) -> &str {
    let token = token.trim().trim_start_matches('#');
    token.trim_end_matches(['.', ',', ';', ':', '!', '?', '…', '»', '"', '\''])
}

/// An exact full id wins. A numeric prefix or suffix of the displayed id is
/// accepted only when it selects one task; several matches stay ambiguous so
/// the reply can ask which one instead of claiming the task is absent.
pub(crate) fn lookup_in_tasks(
    tasks: &[(i64, String, Value, bool)],
    token: &str,
    allowed: &dyn Fn(&Value) -> bool,
) -> TaskIdMatch {
    let token = id_token(token);
    if !valid_id(token) {
        return TaskIdMatch::Missing;
    }
    let ids: Vec<String> = tasks
        .iter()
        .filter(|(_, _, value, _)| allowed(value))
        .map(|(_, id, _, _)| id.clone())
        .collect();
    if let Some(id) = ids.iter().find(|id| id.as_str() == token) {
        return TaskIdMatch::Found(id.clone());
    }
    let exact: Vec<String> = ids
        .iter()
        .filter(|id| short_task_id(id) == token)
        .cloned()
        .collect();
    match exact.len() {
        1 => return TaskIdMatch::Found(exact.into_iter().next().unwrap_or_default()),
        2.. => return TaskIdMatch::Ambiguous(exact),
        _ => {}
    }
    let numeric = token.chars().all(|ch| ch.is_ascii_digit());
    let mut candidates: Vec<String> = ids
        .iter()
        .filter(|id| {
            let shown = short_task_id(id);
            numeric && (shown.starts_with(token) || shown.ends_with(token))
        })
        .cloned()
        .collect();
    if candidates.is_empty() {
        let suffix = format!("-{token}");
        candidates = ids.into_iter().filter(|id| id.ends_with(&suffix)).collect();
    }
    candidates.sort();
    candidates.dedup();
    match candidates.len() {
        0 => TaskIdMatch::Missing,
        1 => TaskIdMatch::Found(candidates.pop().unwrap_or_default()),
        _ => TaskIdMatch::Ambiguous(candidates),
    }
}

pub(crate) fn lookup_task_id(
    dir: &Path,
    token: &str,
    allowed: &dyn Fn(&Value) -> bool,
) -> TaskIdMatch {
    lookup_in_tasks(&read_queue_tasks(dir), token, allowed)
}

pub(crate) fn ambiguous_reply(token: &str, ids: &[String]) -> String {
    let shown: Vec<String> = ids.iter().take(4).map(|id| display_id(id)).collect();
    format!(
        "Не понял, какая задача #{token}: подходит {}. Уточни.",
        shown.join(", ")
    )
}

fn priority_of(task: &(i64, String, Value, bool)) -> Option<i64> {
    task.2.get("priority").and_then(Value::as_i64)
}

pub(crate) fn apply_owner_command(dir: &Path, owner: UserId, command: OwnerCommand) -> String {
    if command.action == OwnerAction::Cancel {
        return cancel_tasks(dir, owner, &command.targets);
    }
    let Some(target) = command.targets.first() else {
        return "Укажи действие и id.".to_owned();
    };
    let tasks = read_queue_tasks(dir);
    let id = match lookup_in_tasks(&tasks, target, &|_| true) {
        TaskIdMatch::Found(id) => id,
        TaskIdMatch::Ambiguous(ids) => return ambiguous_reply(target, &ids),
        TaskIdMatch::Missing => return format!("Не нашёл задачу #{target}."),
    };
    let Some((value, queued)) = find_in_tasks(&tasks, &id) else {
        return format!("Не нашёл задачу #{target}.");
    };
    let status = if queued {
        "queued"
    } else {
        value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    };
    match command.action {
        OwnerAction::Approve => match record_decision(dir, &id, status, "approve", owner) {
            Ok(reply) => format!("{}: #{}", reply, id),
            Err(reason) => format!("#{}: {}.", id, reason),
        },
        OwnerAction::Reject => match record_decision(dir, &id, status, "reject", owner) {
            Ok(reply) => format!("{}: #{}", reply, id),
            Err(reason) => format!("#{}: {}.", id, reason),
        },
        OwnerAction::Cancel => unreachable!("cancel is handled before id resolution"),
        OwnerAction::PriorityTop | OwnerAction::PriorityBottom | OwnerAction::PrioritySet(_) => {
            let priority = match command.action {
                OwnerAction::PrioritySet(value) => value,
                OwnerAction::PriorityTop => {
                    tasks.iter().filter_map(priority_of).max().unwrap_or(0) + 1
                }
                OwnerAction::PriorityBottom => {
                    tasks.iter().filter_map(priority_of).min().unwrap_or(0) - 1
                }
                _ => 0,
            };
            let mut updated = value;
            updated["priority"] = json!(priority);
            match atomic_write(&task_path(dir, &id, queued), updated.to_string().as_bytes()) {
                Ok(()) => format!("Приоритет #{}: {}.", id, priority),
                Err(_) => format!("Не смог изменить приоритет #{}", id),
            }
        }
    }
}

pub(crate) fn cancel_tasks(dir: &Path, owner: UserId, targets: &[String]) -> String {
    let tasks = read_queue_tasks(dir);
    let mut cancelled: Vec<String> = Vec::new();
    let mut cancelled_ids: HashSet<String> = HashSet::new();
    let mut missing: Vec<String> = Vec::new();
    let mut ambiguous: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    for target in targets {
        let id = match lookup_in_tasks(&tasks, target, &|_| true) {
            TaskIdMatch::Found(id) => id,
            TaskIdMatch::Ambiguous(ids) => {
                ambiguous.push(ambiguous_reply(target, &ids));
                continue;
            }
            TaskIdMatch::Missing => {
                missing.push(format!("#{target}"));
                continue;
            }
        };
        if cancelled_ids.contains(&id) {
            continue;
        }
        let Some((value, queued)) = find_in_tasks(&tasks, &id) else {
            missing.push(display_id(&id));
            continue;
        };
        if queued {
            match std::fs::remove_file(task_path(dir, &id, true)) {
                Ok(()) => {
                    cancelled_ids.insert(id.clone());
                    cancelled.push(id);
                }
                Err(_) => blocked.push(format!("{} — не смог убрать из очереди", display_id(&id))),
            }
            continue;
        }
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if !matches!(status, "planned" | "proposed") {
            blocked.push(format!(
                "{} — уже выполняется или завершена",
                display_id(&id)
            ));
            continue;
        }
        match record_decision(dir, &id, status, "reject", owner) {
            Ok(_) => {
                cancelled_ids.insert(id.clone());
                cancelled.push(id);
            }
            Err(reason) => blocked.push(format!("{} — {reason}", display_id(&id))),
        }
    }
    let mut lines = Vec::new();
    if !cancelled.is_empty() {
        let shown: Vec<String> = cancelled.iter().map(|id| display_id(id)).collect();
        lines.push(format!("Отменил {}: {}", cancelled.len(), shown.join(", ")));
    }
    if !missing.is_empty() {
        lines.push(format!("Не нашёл: {}", missing.join(", ")));
    }
    if !ambiguous.is_empty() {
        lines.push(ambiguous.join("\n"));
    }
    if !blocked.is_empty() {
        lines.push(format!("Не вышло: {}", blocked.join("; ")));
    }
    if lines.is_empty() {
        return "Нечего отменять.".to_owned();
    }
    lines.join("\n")
}

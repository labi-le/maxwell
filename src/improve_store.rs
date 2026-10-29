use crate::improve_commands::{ambiguous_reply, lookup_task_id, TaskIdMatch};
use crate::memory::unix_now;
use crate::prelude::*;
use crate::settings::Settings;
use serde_json::json;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const MAX_GOAL_CHARS: usize = 2000;
const MAX_FEEDBACK_CHARS: usize = 4000;
const MAX_PREVIOUS_SPEC_CHARS: usize = 16000;
const MAX_PREVIOUS_REVIEW_CHARS: usize = 8000;

/// Several Tokio tasks may write the same queue file, so the temp name carries
/// a process-wide sequence to keep concurrent renames on distinct inodes.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The only writer of the queue: `inbox/`, `outbox/`, decision markers, and
/// the notifier's `notified/` markers all go through this module.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub(crate) fn queue_dir(cfg: &Settings) -> PathBuf {
    cfg.self_improve_dir.clone()
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Never ends in `.json`: the orchestrator globs `inbox/*.json` and would read
    // a half-written file.
    let tmp = path.with_extension(format!("tmp-{}-{seq}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(tmp, path)
}

pub(crate) fn write_off_marker(dir: &Path) -> bool {
    atomic_write(&dir.join("off"), b"stop").is_ok()
}

pub(crate) fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    text.chars().take(max).collect()
}

fn plan_id(now: i64, source_chat_id: i64, message_id: i32) -> String {
    format!("plan-{now}-{}-{message_id}", source_chat_id.unsigned_abs())
}

pub(crate) struct PlanProposal<'a> {
    pub(crate) dir: &'a Path,
    pub(crate) owner: UserId,
    pub(crate) chat_id: i64,
    pub(crate) source_chat_id: i64,
    pub(crate) proposed_by: UserId,
    pub(crate) proposer: &'a str,
    pub(crate) message_id: i32,
    pub(crate) goal: &'a str,
    pub(crate) parent: Option<&'a str>,
    pub(crate) feedback: &'a str,
    pub(crate) previous_spec: &'a str,
    pub(crate) previous_review: &'a str,
}

pub(crate) fn queue_plan(proposal: PlanProposal<'_>) -> std::io::Result<String> {
    let PlanProposal {
        dir,
        owner,
        chat_id,
        source_chat_id,
        proposed_by,
        proposer,
        message_id,
        goal,
        parent,
        feedback,
        previous_spec,
        previous_review,
    } = proposal;
    if goal.chars().count() > MAX_GOAL_CHARS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "goal too long",
        ));
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
        "feedback": truncate_chars(feedback, MAX_FEEDBACK_CHARS),
        "previous_spec": truncate_chars(previous_spec, MAX_PREVIOUS_SPEC_CHARS),
        "previous_review": truncate_chars(previous_review, MAX_PREVIOUS_REVIEW_CHARS),
        "created_at": unix_now(),
    });
    atomic_write(
        &dir.join("inbox").join(format!("{id}.json")),
        task.to_string().as_bytes(),
    )?;
    Ok(id)
}

pub(crate) struct RefineProposal<'a> {
    pub(crate) dir: &'a Path,
    pub(crate) owner: UserId,
    pub(crate) source_chat_id: i64,
    pub(crate) proposed_by: UserId,
    pub(crate) proposer: &'a str,
    pub(crate) message_id: i32,
    pub(crate) goal: &'a str,
    pub(crate) refines: Option<&'a str>,
}

/// Queue a new plan or a child plan that revises an existing one. `refines`
/// never rejects the parent: the owner closes it with the trash button.
pub(crate) fn queue_feature_refined(proposal: RefineProposal<'_>) -> Result<String, String> {
    let RefineProposal {
        dir,
        owner,
        source_chat_id,
        proposed_by,
        proposer,
        message_id,
        goal,
        refines,
    } = proposal;
    let goal = goal.trim();
    if goal.is_empty() {
        return Err("пустая заявка".to_owned());
    }
    if goal.chars().count() > MAX_GOAL_CHARS && refines.is_none() {
        return Err("заявка слишком длинная".to_owned());
    }
    if proposed_by != owner && has_pending_proposal(dir, proposed_by) {
        return Err("уже есть активная заявка".to_owned());
    }
    let notify_chat = if proposed_by == owner {
        source_chat_id
    } else {
        owner.0 as i64
    };
    let Some(parent_id) = refines else {
        return queue_plan(PlanProposal {
            dir,
            owner,
            chat_id: notify_chat,
            source_chat_id,
            proposed_by,
            proposer,
            message_id,
            goal,
            parent: None,
            feedback: "",
            previous_spec: "",
            previous_review: "",
        })
        .map_err(|_| "не смог поставить план в очередь".to_owned());
    };
    if proposed_by != owner {
        return Err("дорабатывать план может только владелец".to_owned());
    }
    // Chat answers show the parent as `#<short id>`, and the model hands that form back.
    let parent_id = match lookup_task_id(dir, parent_id, &|_| true) {
        TaskIdMatch::Found(id) => id,
        TaskIdMatch::Ambiguous(ids) => return Err(ambiguous_reply(parent_id, &ids)),
        TaskIdMatch::Missing => return Err("план не найден".to_owned()),
    };
    let Some((parent, _)) = find_task(dir, &parent_id) else {
        return Err("план не найден".to_owned());
    };
    if !crate::improve_views::owned_by_owner(&parent, owner) {
        return Err("план не найден".to_owned());
    }
    let status = parent.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(status, "planning" | "planned" | "proposed" | "failed") {
        return Err("план уже закрыт".to_owned());
    }
    let parent_goal = parent
        .get("goal")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if parent_goal.is_empty() {
        return Err("план не найден".to_owned());
    }
    let previous_spec = parent.get("spec").and_then(Value::as_str).unwrap_or("");
    let previous_review = parent.get("review").and_then(Value::as_str).unwrap_or("");
    // The orchestrator only shows `feedback` next to a previous spec, so a
    // remark on a spec-less failed plan has to ride in the child goal.
    let child_goal = if previous_spec.trim().is_empty() {
        truncate_chars(goal, MAX_GOAL_CHARS)
    } else {
        parent_goal.to_owned()
    };
    queue_plan(PlanProposal {
        dir,
        owner,
        chat_id: notify_chat,
        source_chat_id,
        proposed_by,
        proposer,
        message_id,
        goal: &child_goal,
        parent: Some(parent_id.as_str()),
        feedback: goal,
        previous_spec,
        previous_review,
    })
    .map_err(|_| "не смог поставить план в очередь".to_owned())
}

pub(crate) fn has_pending_proposal(dir: &Path, proposer: UserId) -> bool {
    let Ok(entries) = std::fs::read_dir(dir.join("outbox")) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| read_json(&entry.path()))
        .any(|value| {
            value.get("proposed_by").and_then(Value::as_i64) == Some(proposer.0 as i64)
                && matches!(
                    value.get("status").and_then(Value::as_str),
                    Some("planning" | "planned" | "proposed")
                )
        })
}

pub(crate) fn approval_dir(status: &str) -> &'static str {
    if status == "planned" {
        "plan-approvals"
    } else {
        "approvals"
    }
}

pub(crate) fn marker(id: &str, owner: UserId) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&json!({"id": id, "requested_by": owner.0, "approved_at": unix_now()}))
}

pub(crate) fn decision_marker(dir: &Path, id: &str, status: &str) -> Option<&'static str> {
    if dir.join("rejections").join(format!("{id}.json")).exists() {
        return Some("reject");
    }
    if dir
        .join(approval_dir(status))
        .join(format!("{id}.json"))
        .exists()
    {
        return Some("approve");
    }
    None
}

/// Reads outbox and inbox without ever creating, deleting, or rewriting a
/// file. Inbox tasks that the orchestrator has not picked up yet are `queued`.
pub(crate) fn read_queue_tasks(dir: &Path) -> Vec<(i64, String, Value, bool)> {
    let mut tasks = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let outbox = std::fs::read_dir(dir.join("outbox")).ok();
    if let Some(outbox) = outbox {
        for entry in outbox.filter_map(Result::ok) {
            let Some(value) = read_json(&entry.path()) else {
                continue;
            };
            let Some(id) = value.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            if !valid_id(&id) {
                continue;
            }
            seen.insert(id.clone());
            let created = value.get("created_at").and_then(Value::as_i64).unwrap_or(0);
            tasks.push((created, id, value, false));
        }
    }
    let inbox = std::fs::read_dir(dir.join("inbox")).ok();
    if let Some(inbox) = inbox {
        for entry in inbox.filter_map(Result::ok) {
            let Some(value) = read_json(&entry.path()) else {
                continue;
            };
            let Some(id) = value.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            if !valid_id(&id) || !seen.insert(id.clone()) {
                continue;
            }
            let created = value.get("created_at").and_then(Value::as_i64).unwrap_or(0);
            tasks.push((created, id, value, true));
        }
    }
    tasks.sort_by(
        |(left_at, left_id, left, _), (right_at, right_id, right, _)| {
            let left_priority = left.get("priority").and_then(Value::as_i64).unwrap_or(0);
            let right_priority = right.get("priority").and_then(Value::as_i64).unwrap_or(0);
            right_priority
                .cmp(&left_priority)
                .then_with(|| right_at.cmp(left_at))
                .then_with(|| right_id.cmp(left_id))
        },
    );
    tasks
}

pub(crate) fn find_in_tasks(
    tasks: &[(i64, String, Value, bool)],
    id: &str,
) -> Option<(Value, bool)> {
    tasks
        .iter()
        .find(|(_, task_id, _, _)| task_id == id)
        .map(|(_, _, value, queued)| (value.clone(), *queued))
}

/// A single task by exact id. Callers that already hold a queue snapshot use
/// `find_in_tasks` instead of paying for another directory scan.
pub(crate) fn find_task(dir: &Path, id: &str) -> Option<(Value, bool)> {
    if !valid_id(id) {
        return None;
    }
    if let Some(value) = read_json(&dir.join("outbox").join(format!("{id}.json"))) {
        return Some((value, false));
    }
    read_json(&dir.join("inbox").join(format!("{id}.json"))).map(|value| (value, true))
}

/// The id shown in a queue answer: the trailing message id of a plan id,
/// otherwise the whole id. Notifications and status lines both display it, so
/// lookups must accept it back.
pub(crate) fn short_task_id(id: &str) -> &str {
    if let Some((prefix, tail)) = id.rsplit_once('-') {
        if prefix.starts_with("plan-")
            && tail.len() >= 4
            && tail.chars().all(|ch| ch.is_ascii_digit())
        {
            return tail;
        }
    }
    id
}

pub(crate) fn task_path(dir: &Path, id: &str, queued: bool) -> PathBuf {
    dir.join(if queued { "inbox" } else { "outbox" })
        .join(format!("{id}.json"))
}

/// The only writer of `plan-approvals`, `approvals`, and `rejections` markers.
/// Text paths and model tools never reach these writes. The mutex serializes
/// two rapid button presses so the first decision wins.
static DECISION_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn record_decision(
    dir: &Path,
    id: &str,
    status: &str,
    action: &str,
    owner: UserId,
) -> Result<&'static str, &'static str> {
    let _guard = DECISION_WRITE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if decision_marker(dir, id, status).is_some() {
        return Err("решение уже принято");
    }
    match action {
        "approve" if matches!(status, "planned" | "proposed") => marker(id, owner)
            .map_err(|_| "не понял кнопку")
            .and_then(|bytes| {
                atomic_write(
                    &dir.join(approval_dir(status)).join(format!("{id}.json")),
                    &bytes,
                )
                .map_err(|_| "не понял кнопку")
            })
            .map(|()| {
                if status == "planned" {
                    "погнали"
                } else {
                    "деплой запущен"
                }
            }),
        "reject" if matches!(status, "planned" | "proposed") => atomic_write(
            &dir.join("rejections").join(format!("{id}.json")),
            b"reject",
        )
        .map(|()| "в помойку")
        .map_err(|_| "не понял кнопку"),
        _ => Err("не понял кнопку"),
    }
}

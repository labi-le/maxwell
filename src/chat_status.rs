use crate::self_improve::short_task_id;
use serde_json::Value;

fn status_label(status: &str) -> &'static str {
    match status {
        "queued" => "в очереди",
        "planning" => "планируется",
        "planned" => "план готов",
        "running" => "делается",
        "proposed" => "ждёт деплоя",
        "deployed" => "задеплоено",
        "failed" => "провалено",
        "rejected" => "отклонено",
        _ => "неизвестно",
    }
}

fn task_status_label(task: &Value) -> String {
    let status = task.get("status").and_then(Value::as_str).unwrap_or("");
    let approved = task.get("already_decided").and_then(Value::as_str) == Some("approve");
    match (status, approved) {
        ("planned", true) => "план одобрен, ждёт реализации".to_owned(),
        ("proposed", true) => "деплой одобрен, ждёт выполнения".to_owned(),
        _ => status_label(status).to_owned(),
    }
}

fn task_goal_suffix(task: &Value) -> String {
    let goal = task
        .get("goal")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if goal.is_empty() {
        String::new()
    } else {
        format!(": {goal}")
    }
}

fn task_issue_url(task: &Value) -> Option<&str> {
    task.get("issue_url")
        .and_then(Value::as_str)
        .filter(|url| url.starts_with("https://github.com/"))
}

fn status_details_answer(tasks: &[Value], overview: Option<&Value>) -> String {
    if tasks.is_empty() {
        return "Очередь пуста".to_owned();
    }
    if tasks.len() == 1 {
        let task = &tasks[0];
        let id = task.get("id").and_then(Value::as_str).unwrap_or("без id");
        let mut line = format!(
            "#{}: {}{}",
            short_task_id(id),
            task_status_label(task),
            task_goal_suffix(task)
        );
        if let Some(url) = task_issue_url(task) {
            line.push('\n');
            line.push_str(url);
        }
        return line;
    }
    let mut lines = vec!["Очередь".to_owned()];
    for task in tasks {
        let Some(status) = task.get("status").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(
            status,
            "queued" | "planning" | "planned" | "running" | "proposed"
        ) {
            continue;
        }
        let id = task.get("id").and_then(Value::as_str).unwrap_or("без id");
        lines.push(format!(
            "• #{} — {}{}",
            short_task_id(id),
            task_status_label(task),
            task_goal_suffix(task)
        ));
        if let Some(url) = task_issue_url(task) {
            lines.push(format!("  {url}"));
        }
    }
    if lines.len() == 1 {
        lines.push("• активных нет".to_owned());
    }
    status_totals(&mut lines, tasks, overview);
    lines.join("\n")
}

fn status_summary_answer(tasks: &[Value], overview: Option<&Value>) -> String {
    let mut lines = vec!["Очередь".to_owned()];
    let mut active = 0;
    for task in tasks {
        let Some(status) = task.get("status").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(
            status,
            "queued" | "planning" | "planned" | "running" | "proposed"
        ) {
            continue;
        }
        let id = task.get("id").and_then(Value::as_str).unwrap_or("без id");
        lines.push(format!(
            "• #{} — {}",
            short_task_id(id),
            task_status_label(task)
        ));
        active += 1;
    }
    if active == 0 {
        lines.push("• активных нет".to_owned());
    }
    status_totals(&mut lines, tasks, overview);
    lines.join("\n")
}

fn status_links_answer(tasks: &[Value]) -> String {
    let mut links = Vec::new();
    for task in tasks {
        let Some(status) = task.get("status").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(status, "planned" | "proposed" | "running") {
            continue;
        }
        let Some(url) = task_issue_url(task) else {
            continue;
        };
        let id = task.get("id").and_then(Value::as_str).unwrap_or("без id");
        links.push(format!(
            "#{} — {}{}\n{url}",
            short_task_id(id),
            task_status_label(task),
            task_goal_suffix(task)
        ));
    }
    if links.is_empty() {
        "Ссылок на issue нет.".to_owned()
    } else {
        format!("Ссылки на issue:\n\n{}", links.join("\n\n"))
    }
}

fn status_totals(lines: &mut Vec<String>, tasks: &[Value], overview: Option<&Value>) {
    let mut totals = Vec::new();
    for (status, label) in [
        ("deployed", "Задеплоено"),
        ("failed", "Провалено"),
        ("rejected", "Отклонено"),
    ] {
        let count = overview
            .and_then(|value| value.get("counts"))
            .and_then(|counts| counts.get(status))
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                tasks
                    .iter()
                    .filter(|task| task.get("status").and_then(Value::as_str) == Some(status))
                    .count() as u64
            });
        if count > 0 {
            totals.push(format!("{label}: {count}"));
        }
    }
    if !totals.is_empty() {
        lines.push(String::new());
        lines.push("Итоги".to_owned());
        lines.push(totals.join(" · "));
    }
}

pub(crate) fn status_answer(snapshot: &Value, overview: Option<&Value>, view: &str) -> String {
    let tasks = snapshot
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if tasks.is_empty() {
        return match snapshot.get("error").and_then(Value::as_str) {
            Some(error) => format!("Не вижу: {error}"),
            None => "Очередь пуста".to_owned(),
        };
    }
    match view {
        "links" => status_links_answer(&tasks),
        "summary" => status_summary_answer(&tasks, overview),
        _ => status_details_answer(&tasks, overview),
    }
}

fn task_matches_line(task: &Value, line: &str) -> bool {
    let Some(id) = task.get("id").and_then(Value::as_str) else {
        return false;
    };
    if line.contains(id) {
        return true;
    }
    let short = short_task_id(id);
    short != id && line.contains(&format!("#{short}"))
}

fn status_claims(line: &str) -> Vec<&'static str> {
    let lower = line.to_lowercase();
    let mut claims = Vec::new();
    for (needle, status) in [
        ("задепло", "deployed"),
        ("провал", "failed"),
        ("отклон", "rejected"),
        ("в очереди", "queued"),
        ("план готов", "planned"),
        ("план одобрен", "planned"),
        ("делается", "running"),
        ("ждёт деплоя", "proposed"),
        ("ждет деплоя", "proposed"),
        ("deployed", "deployed"),
        ("failed", "failed"),
        ("rejected", "rejected"),
        ("queued", "queued"),
        ("planned", "planned"),
        ("running", "running"),
        ("proposed", "proposed"),
    ] {
        let Some(index) = lower.find(needle) else {
            continue;
        };
        let prefix = &lower[..index];
        let negated = prefix.ends_with("не ") || prefix.ends_with("not ") || prefix.ends_with("не");
        if !negated && !claims.contains(&status) {
            claims.push(status);
        }
    }
    claims
}

fn status_line_has_unknown_reference(line: &str, tasks: &[Value]) -> bool {
    for raw in line.split_whitespace() {
        let token = raw.trim_matches(|ch: char| {
            matches!(
                ch,
                '(' | ')' | '[' | ']' | '{' | '}' | ',' | '.' | ';' | ':'
            )
        });
        if let Some(rest) = token.strip_prefix('#') {
            if rest.len() >= 4 && rest.chars().all(|ch| ch.is_ascii_digit()) {
                let known = tasks.iter().any(|task| {
                    task.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| short_task_id(id) == rest || id == rest)
                });
                if !known {
                    return true;
                }
            }
        }
        if token.starts_with("plan-")
            && !tasks
                .iter()
                .any(|task| task.get("id").and_then(Value::as_str) == Some(token))
        {
            return true;
        }
        if token.starts_with("https://github.com/")
            && !tasks.iter().any(|task| task_issue_url(task) == Some(token))
        {
            return true;
        }
    }
    false
}

fn status_line_claims_emptiness(line: &str) -> bool {
    if line.contains('?') {
        return false;
    }
    let lower = line.to_lowercase();
    [
        "очередь пуста",
        "пустая очередь",
        "активных нет",
        "нет активных",
        "ничего нет",
        "нет задач",
        "no active",
        "empty",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// The model may phrase a status answer freely, but a claim attached to a task
/// must match the snapshot. This rejects the classic "everything is deployed"
/// hallucination while leaving room for explanations and clarifying questions.
pub(crate) fn status_answer_grounded(text: &str, snapshot: Option<&Value>, view: &str) -> bool {
    let Some(snapshot) = snapshot else {
        return false;
    };
    let tasks = snapshot
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if text.trim().is_empty() {
        return false;
    }
    let mut has_url = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if status_line_has_unknown_reference(line, &tasks)
            || (!tasks.is_empty() && status_line_claims_emptiness(line))
        {
            return false;
        }
        has_url |= line.contains("https://github.com/");
        let matched: Vec<&Value> = tasks
            .iter()
            .filter(|task| task_matches_line(task, line))
            .collect();
        let claims = status_claims(line);
        if claims.is_empty() {
            continue;
        }
        if matched.is_empty() {
            if line.contains('?') {
                continue;
            }
            return false;
        }
        let actual: Vec<&str> = matched
            .iter()
            .filter_map(|task| task.get("status").and_then(Value::as_str))
            .collect();
        if actual.is_empty() || actual.iter().any(|status| *status != actual[0]) {
            return false;
        }
        if claims.iter().any(|claim| *claim != actual[0]) {
            return false;
        }
    }
    if view == "links" && tasks.iter().any(|task| task_issue_url(task).is_some()) && !has_url {
        return false;
    }
    true
}

/// Detailed answers keep their own lines and URLs; the one-line chat style
/// would cut issue links at the 280-character boundary.
pub(crate) fn style_status_reply(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !lines.is_empty() && lines.last().is_some_and(|line: &String| !line.is_empty()) {
                lines.push(String::new());
            }
            continue;
        }
        lines.push(line.trim_end_matches(['.', '…']).to_owned());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

use crate::prelude::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct FailureDiagnostic {
    pub(crate) stage: &'static str,
    pub(crate) detail: &'static str,
    pub(crate) exit_code: Option<i32>,
}

impl FailureDiagnostic {
    pub(crate) fn new(stage: &'static str, detail: &'static str) -> Self {
        Self {
            stage,
            detail,
            exit_code: None,
        }
    }
}

impl std::fmt::Display for FailureDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.detail)
    }
}

#[derive(Debug)]
pub(crate) struct DownloadFailure {
    pub(crate) reason: String,
    pub(crate) diagnostic: FailureDiagnostic,
    pub(crate) events: Vec<FailureDiagnostic>,
}

impl DownloadFailure {
    pub(crate) fn new(reason: String, stage: &'static str, detail: &'static str) -> Self {
        Self {
            reason,
            diagnostic: FailureDiagnostic::new(stage, detail),
            events: Vec::new(),
        }
    }
}

pub(crate) fn trace_event(events: &mut Vec<FailureDiagnostic>, event: FailureDiagnostic) {
    if events.len() < 12 {
        events.push(event);
    }
}

pub(crate) fn source_user_diagnostic(
    diagnostic: FailureDiagnostic,
    events: &[FailureDiagnostic],
) -> FailureDiagnostic {
    // Keep the video/authentication origin in the journal, but display the actionable failure.
    if diagnostic.stage != "yt-dlp"
        || !matches!(
            diagnostic.detail,
            "Authentication required" | "Cookies expired" | "Download failed"
        )
    {
        return diagnostic;
    }
    events
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event.stage,
                "yt-dlp" | "metadata" | "photo-page" | "photo-cdn"
            ) || (event.stage == "selection" && event.detail == "Size limit")
                || (event.stage == "retry"
                    && matches!(
                        event.detail,
                        "Launch failed" | "Attempt failed" | "Preparation failed"
                    ))
        })
        .copied()
        .unwrap_or(diagnostic)
}

pub(crate) async fn record_failure(pool: &SqlitePool, url: &Url, failure: FailureDiagnostic) {
    record_failure_with_events(pool, url, failure, &[]).await;
}

pub(crate) async fn record_failure_with_events(
    pool: &SqlitePool,
    url: &Url,
    failure: FailureDiagnostic,
    events: &[FailureDiagnostic],
) {
    let result = async {
        let mut transaction = pool.begin().await?;
        let rowid = sqlx::query("INSERT INTO download_failures (url_host, url_path, stage, detail, exit_code) VALUES (?, ?, ?, ?, ?)")
            .bind(url.host_str().unwrap_or(""))
            .bind(url.path())
            .bind(failure.stage)
            .bind(failure.detail)
            .bind(failure.exit_code)
            .execute(&mut *transaction).await?.last_insert_rowid();
        for (index, event) in events.iter().take(12).enumerate() {
            sqlx::query("INSERT INTO download_failure_events (failure_rowid, event_index, stage, detail, exit_code) VALUES (?, ?, ?, ?, ?)")
                .bind(rowid).bind(index as i64 + 1).bind(event.stage).bind(event.detail)
                .bind(event.exit_code).execute(&mut *transaction).await?;
        }
        if events.is_empty() {
            sqlx::query("INSERT INTO download_failure_events (failure_rowid, event_index, stage, detail, exit_code) VALUES (?, 1, ?, ?, ?)")
                .bind(rowid).bind(failure.stage).bind(failure.detail).bind(failure.exit_code)
                .execute(&mut *transaction).await?;
        }
        sqlx::query("DELETE FROM download_failure_events WHERE failure_rowid NOT IN (SELECT rowid FROM download_failures ORDER BY rowid DESC LIMIT 200)")
            .execute(&mut *transaction).await?;
        sqlx::query("DELETE FROM download_failures WHERE rowid NOT IN (SELECT rowid FROM download_failures ORDER BY rowid DESC LIMIT 200)")
            .execute(&mut *transaction).await?;
        transaction.commit().await
    }.await;
    if result.is_err() {
        eprintln!("Не удалось сохранить диагностику загрузки");
    }
}

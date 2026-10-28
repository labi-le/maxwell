use crate::prelude::*;
use crate::*;

pub(crate) async fn open_database(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(url)?.create_if_missing(true);
    let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS group_settings (chat_id INTEGER PRIMARY KEY, enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)))",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS download_failures (created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), url_host TEXT NOT NULL, url_path TEXT NOT NULL, stage TEXT NOT NULL, detail TEXT NOT NULL, exit_code INTEGER)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS download_failure_events (failure_rowid INTEGER NOT NULL, event_index INTEGER NOT NULL, stage TEXT NOT NULL, detail TEXT NOT NULL, exit_code INTEGER, PRIMARY KEY(failure_rowid, event_index))",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat_turns (id INTEGER PRIMARY KEY AUTOINCREMENT, chat_id INTEGER NOT NULL, user_id INTEGER NOT NULL, role TEXT NOT NULL CHECK(role IN ('user', 'assistant')), content TEXT NOT NULL, created_at INTEGER NOT NULL)",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS chat_turns_lookup ON chat_turns(chat_id, user_id, created_at)",
    )
    .execute(&pool)
    .await?;
    init_cache(&pool).await?;
    Ok(pool)
}

pub(crate) async fn group_enabled(pool: &SqlitePool, chat_id: ChatId) -> Result<bool, sqlx::Error> {
    let enabled: Option<i64> = sqlx::query_scalar("SELECT enabled FROM group_settings WHERE chat_id = ?")
        .bind(chat_id.0)
        .fetch_optional(pool)
        .await?;
    Ok(enabled == Some(1))
}

pub(crate) async fn set_group_enabled(
    pool: &SqlitePool,
    chat_id: ChatId,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO group_settings (chat_id, enabled) VALUES (?, ?) ON CONFLICT(chat_id) DO UPDATE SET enabled = excluded.enabled",
    )
    .bind(chat_id.0)
    .bind(i64::from(enabled))
    .execute(pool)
    .await?;
    Ok(())
}

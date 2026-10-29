use crate::{MediaKind, MAX_FILES};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};
use url::Url;

const TTL_SECONDS: i64 = 24 * 60 * 60;
const MAX_PUBLICATIONS: i64 = 200;
const MAX_DESCRIPTION_UTF16: usize = 1024;
const MAX_FILE_ID_BYTES: usize = 1024;
// JSON escaping can use six bytes per original UTF-16 unit or file-ID byte.
const MAX_ITEMS_JSON_BYTES: usize =
    MAX_FILES * (6 * (MAX_FILE_ID_BYTES + MAX_DESCRIPTION_UTF16) + 128) + 2;

#[derive(Debug)]
pub(crate) struct CachedMedia {
    pub kind: MediaKind,
    pub file_id: String,
    pub description: Option<String>,
}

pub(crate) fn cache_key(bot_token: &str, url: &Url) -> String {
    let mut digest = Sha256::new();
    digest.update(b"maksut/media-cache/v4-real-title\0");
    digest.update(bot_token.as_bytes());
    digest.update(b"\0");
    digest.update(url.as_str().as_bytes());
    format!("{:x}", digest.finalize())
}

pub(crate) async fn init_cache(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS media_cache (
            chat_id INTEGER NOT NULL,
            cache_key TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            items_json TEXT NOT NULL,
            PRIMARY KEY(chat_id, cache_key)
        )",
    )
    .execute(&mut *transaction)
    .await?;
    prune_cache(&mut transaction).await?;
    transaction.commit().await
}

async fn prune_cache(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM media_cache
         WHERE typeof(created_at) != 'integer'
            OR created_at <= unixepoch() - ?
            OR created_at > unixepoch()",
    )
    .bind(TTL_SECONDS)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "DELETE FROM media_cache WHERE rowid NOT IN (
            SELECT rowid FROM media_cache ORDER BY created_at DESC, rowid DESC LIMIT ?
        )",
    )
    .bind(MAX_PUBLICATIONS)
    .execute(connection)
    .await?;
    Ok(())
}

pub(crate) async fn lookup_cache(
    pool: &SqlitePool,
    chat_id: i64,
    key: &str,
) -> Result<Option<Vec<CachedMedia>>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    prune_cache(&mut transaction).await?;
    // Do not materialize unbounded or incorrectly typed database values in Rust.
    let row = sqlx::query_scalar::<_, Option<String>>(
        "SELECT CASE
            WHEN typeof(items_json) = 'text' AND length(CAST(items_json AS BLOB)) <= ?
            THEN items_json ELSE NULL END
         FROM media_cache WHERE chat_id = ? AND cache_key = ?",
    )
    .bind(MAX_ITEMS_JSON_BYTES as i64)
    .bind(chat_id)
    .bind(key)
    .fetch_optional(&mut *transaction)
    .await?;
    let items = match row {
        None => None,
        Some(json) => {
            let items = json.and_then(|json| parse_items(&json));
            if items.is_none() {
                sqlx::query("DELETE FROM media_cache WHERE chat_id = ? AND cache_key = ?")
                    .bind(chat_id)
                    .bind(key)
                    .execute(&mut *transaction)
                    .await?;
            }
            items
        }
    };
    transaction.commit().await?;
    Ok(items)
}

pub(crate) async fn store_cache(
    pool: &SqlitePool,
    chat_id: i64,
    key: &str,
    items: &[CachedMedia],
) -> Result<(), sqlx::Error> {
    if items.is_empty()
        || items.len() > MAX_FILES
        || items.iter().any(|item| !valid_file_id(&item.file_id))
    {
        // A failed fresh publication must not leave an older usable entry behind.
        invalidate_cache(pool, chat_id, key).await?;
        return Err(sqlx::Error::Protocol("Invalid cache publication".into()));
    }
    let json = serialize_items(items)?;
    let mut transaction = pool.begin().await?;
    // Replacement gives a fresh rowid, so equally timed stores retain publication order.
    sqlx::query(
        "INSERT OR REPLACE INTO media_cache (chat_id, cache_key, created_at, items_json)
         VALUES (?, ?, unixepoch(), ?)",
    )
    .bind(chat_id)
    .bind(key)
    .bind(json)
    .execute(&mut *transaction)
    .await?;
    prune_cache(&mut transaction).await?;
    transaction.commit().await
}

pub(crate) async fn invalidate_cache(
    pool: &SqlitePool,
    chat_id: i64,
    key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM media_cache WHERE chat_id = ? AND cache_key = ?")
        .bind(chat_id)
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

fn valid_file_id(file_id: &str) -> bool {
    !file_id.is_empty()
        && file_id.len() <= MAX_FILE_ID_BYTES
        && !file_id
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn description_prefix(description: &str) -> &str {
    let mut units = 0;
    for (offset, ch) in description.char_indices() {
        units += ch.len_utf16();
        if units > MAX_DESCRIPTION_UTF16 {
            return &description[..offset];
        }
    }
    description
}

fn serialize_items(items: &[CachedMedia]) -> Result<String, sqlx::Error> {
    let capacity = 2 + items
        .iter()
        .map(|item| {
            64 + item.file_id.len()
                + item.description.as_ref().map_or(4, |description| {
                    description.len().min(MAX_DESCRIPTION_UTF16 * 4)
                })
        })
        .sum::<usize>();
    let mut json = Vec::with_capacity(capacity);
    json.push(b'[');
    for (index, item) in items.iter().enumerate() {
        if index != 0 {
            json.push(b',');
        }
        let kind = match item.kind {
            MediaKind::Photo => b"{\"kind\":\"photo\",\"file_id\":".as_slice(),
            MediaKind::Video => b"{\"kind\":\"video\",\"file_id\":".as_slice(),
            MediaKind::Audio => b"{\"kind\":\"audio\",\"file_id\":".as_slice(),
            MediaKind::Document => b"{\"kind\":\"document\",\"file_id\":".as_slice(),
        };
        json.extend_from_slice(kind);
        serde_json::to_writer(&mut json, &item.file_id)
            .map_err(|_| sqlx::Error::Protocol("Unable to encode cache publication".into()))?;
        json.extend_from_slice(b",\"description\":");
        let description = item.description.as_deref().map(description_prefix);
        serde_json::to_writer(&mut json, &description)
            .map_err(|_| sqlx::Error::Protocol("Unable to encode cache publication".into()))?;
        json.push(b'}');
    }
    json.push(b']');
    String::from_utf8(json)
        .map_err(|_| sqlx::Error::Protocol("Unable to encode cache publication".into()))
}

fn parse_items(json: &str) -> Option<Vec<CachedMedia>> {
    let Value::Array(values) = serde_json::from_str::<Value>(json).ok()? else {
        return None;
    };
    if values.is_empty() || values.len() > MAX_FILES {
        return None;
    }
    let mut items = Vec::with_capacity(values.len());
    for value in values {
        let Value::Object(mut object) = value else {
            return None;
        };
        if object.len() != 3 {
            return None;
        }
        let kind = match object.remove("kind")?.as_str()? {
            "photo" => MediaKind::Photo,
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            "document" => MediaKind::Document,
            _ => return None,
        };
        let Value::String(file_id) = object.remove("file_id")? else {
            return None;
        };
        if !valid_file_id(&file_id) {
            return None;
        }
        let description = match object.remove("description")? {
            Value::Null => None,
            Value::String(description) => {
                if description
                    .encode_utf16()
                    .take(MAX_DESCRIPTION_UTF16 + 1)
                    .count()
                    > MAX_DESCRIPTION_UTF16
                {
                    return None;
                }
                Some(description)
            }
            _ => return None,
        };
        items.push(CachedMedia {
            kind,
            file_id,
            description,
        });
    }
    Some(items)
}

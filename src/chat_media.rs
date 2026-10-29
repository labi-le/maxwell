use crate::prelude::*;

const CHAT_IMAGE_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct ChatImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) mime: String,
}

/// The image or sticker frame that best represents a message: the largest
/// photo, a static sticker, or a thumbnail for animated and video media.
pub(crate) fn message_image(message: &Message) -> Option<(FileId, String)> {
    if let Some(sizes) = message.photo() {
        let best = sizes
            .iter()
            .max_by_key(|photo| u64::from(photo.width) * u64::from(photo.height))?;
        return Some((best.file.id.clone(), "image/jpeg".into()));
    }
    if let Some(sticker) = message.sticker() {
        if sticker.is_animated() || sticker.is_video() {
            return sticker
                .thumbnail
                .as_ref()
                .map(|thumbnail| (thumbnail.file.id.clone(), "image/jpeg".into()));
        }
        return Some((sticker.file.id.clone(), "image/webp".into()));
    }
    if let Some(animation) = message.animation() {
        return animation
            .thumbnail
            .as_ref()
            .map(|thumbnail| (thumbnail.file.id.clone(), "image/jpeg".into()));
    }
    if let Some(video) = message.video() {
        return video
            .thumbnail
            .as_ref()
            .map(|thumbnail| (thumbnail.file.id.clone(), "image/jpeg".into()));
    }
    if let Some(note) = message.video_note() {
        return note
            .thumbnail
            .as_ref()
            .map(|thumbnail| (thumbnail.file.id.clone(), "image/jpeg".into()));
    }
    if let Some(document) = message.document() {
        let mime = document.mime_type.as_ref()?.to_string();
        if mime.starts_with("image/") {
            return Some((document.file.id.clone(), mime));
        }
    }
    None
}

/// One client for every attachment download: building a fresh one per photo
/// message threw away the connection pool and its TLS setup.
static IMAGE_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

pub(crate) async fn download_image(bot: &Bot, file_id: &FileId, mime: &str) -> Option<ChatImage> {
    let file = bot.get_file(file_id.clone()).await.ok()?;
    let url = format!(
        "https://api.telegram.org/file/bot{}/{}",
        bot.token(),
        file.path
    );
    let response = IMAGE_CLIENT.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    if response
        .content_length()
        .is_some_and(|length| length > CHAT_IMAGE_MAX_BYTES)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?.to_vec();
    if bytes.is_empty() || bytes.len() as u64 > CHAT_IMAGE_MAX_BYTES {
        return None;
    }
    Some(ChatImage {
        bytes,
        mime: mime.to_owned(),
    })
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        encoded.push(TABLE[usize::from(first >> 2)] as char);
        encoded.push(TABLE[usize::from((first & 0x03) << 4 | second >> 4)] as char);
        encoded.push(if chunk.len() > 1 {
            TABLE[usize::from((second & 0x0f) << 2 | third >> 6)] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[usize::from(third & 0x3f)] as char
        } else {
            '='
        });
    }
    encoded
}

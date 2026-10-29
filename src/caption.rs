use crate::prelude::*;
use crate::*;

pub(crate) fn escape_html_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub(crate) fn placeholder_title(title: &str) -> bool {
    if title.starts_with("Video by ") || title.starts_with("Photo by ") {
        return true;
    }
    // yt-dlp's default when a post has no real title: "<Extractor> video #<id>".
    let Some((prefix, id)) = title.rsplit_once(" #") else {
        return false;
    };
    matches!(
        prefix.rsplit_once(' ').map(|(_, kind)| kind),
        Some("video" | "photo")
    ) && !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn caption_text(title: Option<&str>, description: Option<&str>) -> Option<String> {
    let title = title.map(str::trim).filter(|text| !text.is_empty());
    let description = description.map(str::trim).filter(|text| !text.is_empty());
    let title = title.filter(|title| !placeholder_title(title));
    match (title, description) {
        (Some(title), Some(description)) if title == description => Some(title.to_owned()),
        (Some(title), Some(description)) => Some(format!("{title}\n\n{description}")),
        (Some(text), None) | (None, Some(text)) => Some(text.to_owned()),
        (None, None) => None,
    }
}

// The sender's own words: message text left after removing the chosen link.
pub(crate) fn source_comment(text: &str, url: &Url) -> Option<String> {
    let mut removed = None;
    let mut offset = 0;
    for word in text.split_whitespace() {
        let start = offset + text[offset..].find(word).unwrap_or(0);
        let end = start + word.len();
        offset = end;
        let trimmed = word
            .trim_start_matches(['(', '[', '<', '"', '\''])
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\'']);
        if removed.is_none() && allowed_url(trimmed).as_ref() == Some(url) {
            removed = Some((start, end));
        }
    }
    let mut comment = match removed {
        Some((start, end)) => format!("{}{}", &text[..start], &text[end..]),
        None => text.to_owned(),
    };
    // Removing the link can leave a doubled separator; keep line breaks but tidy spaces.
    comment = comment
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    comment = comment.trim().to_owned();
    (!comment.is_empty()).then_some(comment)
}

pub(crate) fn media_caption(
    sender: &teloxide::types::User,
    url: &Url,
    body: Option<&str>,
) -> Result<String, &'static str> {
    let display = escape_html_attribute(&sender.first_name);
    let footer = format!(
        "<code>От {display}</code>\n<a href=\"{}\">Ссылка</a>",
        escape_html_attribute(url.as_str())
    );
    let mut budget = 1024usize
        .checked_sub(footer.encode_utf16().count())
        .and_then(|budget| budget.checked_sub(1))
        .ok_or("Подпись слишком длинная для Telegram")?;
    let mut caption = String::new();
    // The caption carries only the publication's own text; the sender's message
    // stays untouched in the chat.
    for text in [body].into_iter().flatten().filter(|text| !text.is_empty()) {
        let separator = usize::from(!caption.is_empty());
        let mut chunk = String::new();
        let mut used = 0;
        for ch in text.chars() {
            let entity = match ch {
                '&' => Some("&amp;"),
                '<' => Some("&lt;"),
                '>' => Some("&gt;"),
                '"' => Some("&quot;"),
                '\'' => Some("&#39;"),
                _ => None,
            };
            let size = entity.map_or(ch.len_utf16(), |text| text.len());
            if size > budget.saturating_sub(used + separator) {
                break;
            }
            used += size;
            if let Some(entity) = entity {
                chunk.push_str(entity);
            } else {
                chunk.push(ch);
            }
        }
        if chunk.is_empty() {
            continue;
        }
        if separator == 1 {
            caption.push('\n');
        }
        caption.push_str(&chunk);
        budget -= used + separator;
    }
    if !caption.is_empty() {
        caption.push('\n');
    }
    caption.push_str(&footer);
    Ok(caption)
}

use crate::research::ResearchSource;

const TELEGRAM_MESSAGE_CHARS: usize = 3900;
const MAX_REPLY_CHARS: usize = 280;
const RESEARCH_BODY_CHARS: usize = 4800;
const RESEARCH_SOURCES_CHARS: usize = 1200;
const RESEARCH_MAX_CHARS: usize = 6000;

/// The prompt with the bot name stripped, or `None` when the message was not
/// addressed to the bot. The private chat counts as addressed, and so does a
/// reply to one of the bot's own messages in a group.
pub(crate) fn chat_prompt(
    text: &str,
    private: bool,
    bot_username: &str,
    reply_to_bot: bool,
) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if private || reply_to_bot {
        return Some(text.to_owned());
    }
    let mut names = vec!["максут".to_string(), "maksut".to_string()];
    if !bot_username.is_empty() {
        names.push(bot_username.to_lowercase());
    }
    let mut cleaned = text.to_owned();
    let mut addressed = false;
    for name in names {
        let (rest, found) = strip_name(&cleaned, &name);
        if found {
            addressed = true;
            cleaned = rest;
        }
    }
    if !addressed {
        return None;
    }
    let cleaned = cleaned.trim_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '.' | ':' | ';' | '!' | '?' | '-')
    });
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

fn strip_name(text: &str, name: &str) -> (String, bool) {
    let mut found = false;
    let kept: Vec<&str> = text
        .split_whitespace()
        .filter(|word| {
            let core = word.trim_matches(|ch: char| !ch.is_alphanumeric());
            if !core.is_empty() && core.to_lowercase() == name {
                found = true;
                false
            } else {
                true
            }
        })
        .collect();
    (kept.join(" "), found)
}

/// Plain-text chunks within Telegram's message limit, split on line
/// boundaries where possible.
pub(crate) fn split_message(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.chars().count() > TELEGRAM_MESSAGE_CHARS {
            if !current.trim().is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            let mut rest = line;
            while rest.chars().count() > TELEGRAM_MESSAGE_CHARS {
                let split = rest
                    .char_indices()
                    .nth(TELEGRAM_MESSAGE_CHARS)
                    .map(|(index, _)| index)
                    .unwrap_or(rest.len());
                chunks.push(rest[..split].to_owned());
                rest = &rest[split..];
            }
            current.push_str(rest);
            continue;
        }
        let projected =
            current.chars().count() + usize::from(!current.is_empty()) + line.chars().count();
        if projected > TELEGRAM_MESSAGE_CHARS {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Lowercases the whole reply, except for URLs whose path may be case-sensitive.
pub(crate) fn lowercase_reply(text: &str) -> String {
    text.split_inclusive(char::is_whitespace)
        .map(|token| {
            let trimmed = token.trim_start_matches(|ch: char| !ch.is_alphanumeric());
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                token.to_owned()
            } else {
                token.to_lowercase()
            }
        })
        .collect()
}

/// Research style: paragraphs and citation markers survive, but the body is
/// bounded and the bot always appends a verified source list.
pub(crate) fn style_research_reply(
    text: &str,
    sources: &[ResearchSource],
) -> Result<String, String> {
    let lowered = lowercase_reply(text);
    let normalized = lowered.replace('\r', "");
    let mut body = String::new();
    let mut blank = 0;
    for line in normalized.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank <= 2 {
                body.push('\n');
            }
        } else {
            blank = 0;
            body.push_str(line);
            body.push('\n');
        }
    }
    let body = body.replace(['—', '–'], "-");
    let body = truncate_text(body.trim(), RESEARCH_BODY_CHARS);
    let (body, cited) = sanitize_citations(&body, sources.len());
    let selected = citation_ids(cited, sources.len());
    let block = render_sources_block(&selected, sources, RESEARCH_SOURCES_CHARS);
    let body_limit = RESEARCH_MAX_CHARS
        .saturating_sub(block.chars().count() + 2)
        .min(RESEARCH_BODY_CHARS);
    let body = truncate_text(body.trim(), body_limit);
    if body.trim().is_empty() {
        return Err("empty".into());
    }
    let (body, cited) = sanitize_citations(&body, sources.len());
    let selected = citation_ids(cited, sources.len());
    let block = render_sources_block(&selected, sources, RESEARCH_SOURCES_CHARS);
    Ok(format!("{body}\n\n{block}"))
}

fn citation_ids(cited: Vec<usize>, source_count: usize) -> Vec<usize> {
    if cited.is_empty() {
        (1..=source_count.min(3)).collect()
    } else {
        cited
    }
}

fn sanitize_citations(text: &str, source_count: usize) -> (String, Vec<usize>) {
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut cited = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '[' {
            if let Some(close) = chars[index + 1..].iter().position(|ch| *ch == ']') {
                let content: String = chars[index + 1..index + 1 + close].iter().collect();
                if !content.is_empty() && content.chars().all(|ch| ch.is_ascii_digit()) {
                    if let Ok(id) = content.parse::<usize>() {
                        if (1..=source_count).contains(&id) {
                            output.push('[');
                            output.push_str(&id.to_string());
                            output.push(']');
                            if !cited.contains(&id) {
                                cited.push(id);
                            }
                        }
                    }
                    index += close + 2;
                    continue;
                }
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    (output, cited)
}

fn render_sources_block(
    selected: &[usize],
    sources: &[ResearchSource],
    max_chars: usize,
) -> String {
    let mut lines = vec!["источники:".to_string()];
    let mut used = "источники:".chars().count();
    for (index, id) in selected.iter().enumerate() {
        let Some(source) = sources.iter().find(|source| source.id == *id) else {
            continue;
        };
        let title = source.title.to_lowercase().replace(['—', '–'], "-");
        let prefix = format!("[{id}] ");
        let suffix = format!(" - {}", source.url);
        let full = format!("{prefix}{title}{suffix}");
        let remaining = max_chars.saturating_sub(used + 1);
        let line = if full.chars().count() <= remaining {
            full
        } else {
            let fixed = prefix.chars().count() + suffix.chars().count();
            if fixed <= remaining {
                let title_budget = remaining.saturating_sub(fixed);
                format!("{prefix}{}{suffix}", truncate_text(&title, title_budget))
            } else if index == 0 {
                // Never cut a URL; the body limit absorbs the overflow.
                full
            } else {
                break;
            }
        };
        used += line.chars().count() + 1;
        lines.push(line);
    }
    lines.join("\n")
}

/// Chat style: lowercase, one line, no trailing period, and short enough to
/// read like a message rather than an article.
pub(crate) fn style_reply(text: &str) -> String {
    let lowered = lowercase_reply(text);
    let joined = lowered
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    shorten(&joined, MAX_REPLY_CHARS)
        .replace(['—', '–'], "-")
        .trim()
        .trim_end_matches(['.', '…'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn shorten(text: &str, max_chars: usize) -> &str {
    if text.chars().count() <= max_chars {
        return text;
    }
    let limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let head = &text[..limit];
    if let Some(index) = head.rfind(['.', '!', '?']) {
        return head[..index].trim_end();
    }
    head.rfind(char::is_whitespace)
        .map(|index| &head[..index])
        .unwrap_or(head)
}

pub(crate) fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let head = &text[..limit];
    let cut = head.rfind(char::is_whitespace).unwrap_or(head.len());
    let head = if cut == 0 { head } else { &head[..cut] };
    head.trim_end().to_owned()
}

pub(crate) fn is_bidi_or_control(ch: char) -> bool {
    ch.is_control()
        || matches!(ch as u32,
            0x00AD
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2066..=0x2069
            | 0xFEFF)
}

pub(crate) fn strip_url_schemes(text: &str) -> String {
    const SCHEMES: [&str; 7] = [
        "https://",
        "http://",
        "tg://",
        "javascript:",
        "data:",
        "file:",
        "ftp://",
    ];
    let lower = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let boundary = text[..index]
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_alphanumeric());
        let mut matched = None;
        if boundary {
            for scheme in SCHEMES {
                if lower[index..].starts_with(scheme) {
                    matched = Some(scheme);
                    break;
                }
            }
        }
        if let Some(scheme) = matched {
            index += scheme.len();
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or_default();
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

pub(crate) fn explicit_sticker_request(text: &str) -> bool {
    let text = text.to_lowercase();
    let verb = text.contains("скинь")
        || text.contains("покажи")
        || text.contains("добавь")
        || text.contains("кинь");
    verb && (text.contains("стикер")
        || text.contains("пак")
        || text.contains("девоч")
        || text.contains("девуш"))
}

pub(crate) fn explicit_feature_request(text: &str) -> bool {
    let text = text.to_lowercase();
    let verb = text.contains("добавь")
        || text.contains("измени")
        || text.contains("убери")
        || text.contains("запили")
        || text.contains("почини");
    verb && (text.contains("бот")
        || text.contains("фич")
        || text.contains("функци")
        || text.contains("поведени")
        || text.contains("возможност"))
}

pub(crate) fn sticker_request_query(text: &str) -> &'static str {
    let text = text.to_lowercase();
    if text.contains("девоч") || text.contains("девуш") {
        "девушка"
    } else if text.contains("кот") {
        "кот"
    } else if text.contains("собак") {
        "собака"
    } else if text.contains("груст") {
        "грустный"
    } else {
        "стикер"
    }
}

pub(crate) fn sticker_emotion_hint(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "бля",
        "хуй",
        "пизд",
        "заеб",
        "бесит",
        "устал",
        "устала",
        "заколеб",
        "достал",
        "охуе",
        "кайф",
        "радост",
        "смешн",
        "ору",
        "ржака",
        "груст",
        "плохо",
        "деплой",
        "прод",
    ]
    .iter()
    .any(|word| text.contains(word))
}

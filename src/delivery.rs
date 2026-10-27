use crate::prelude::*;
use crate::*;

#[derive(Debug)]
pub(crate) struct DeliveryError {
    pub(crate) reason: String,
    pub(crate) diagnostic: FailureDiagnostic,
    pub(crate) invalid_cached_file: bool,
}

impl DeliveryError {
    pub(crate) fn request(error: teloxide::RequestError, max_bytes: u64) -> Self {
        Self {
            reason: user_errors::request_message(&error, max_bytes),
            diagnostic: user_errors::request_diagnostic(&error),
            invalid_cached_file: user_errors::invalid_cached_file(&error),
        }
    }

    pub(crate) fn preparation(reason: String) -> Self {
        Self {
            reason,
            diagnostic: FailureDiagnostic::new("preparation", "Media preparation failed"),
            invalid_cached_file: false,
        }
    }
}

pub(crate) struct DeliveryReceipt {
    pub(crate) kind: MediaKind,
    pub(crate) message: Message,
}

pub(crate) struct PreparedFile {
    pub(crate) input: InputFile,
    pub(crate) kind: MediaKind,
    pub(crate) converted: Option<tempfile::TempPath>,
}

pub(crate) async fn prepare_media(file: &MediaFile, max_bytes: u64, deadline: time::Instant)
    -> Result<PreparedFile, DeliveryError>
{
    let mut kind = file.kind;
    if kind == MediaKind::Video {
        require_video_audio(&file.path, deadline).await.map_err(DeliveryError::preparation)?;
    }
    let converted = if kind == MediaKind::Document && convertible_video(&file.path) {
        if let Some(dimensions) = video_dimensions(&file.path, deadline, "V:0").await
            .map_err(DeliveryError::preparation)?
        {
            require_video_audio(&file.path, deadline).await.map_err(DeliveryError::preparation)?;
            let output = convert_video(&file.path, dimensions, max_bytes, deadline).await
                .map_err(DeliveryError::preparation)?;
            kind = MediaKind::Video;
            Some(output)
        } else if video_dimensions(&file.path, deadline, "v:0").await
            .map_err(DeliveryError::preparation)?.is_some()
        {
            return Err(DeliveryError::preparation("Видеофайл содержит только обложку".into()));
        } else {
            None
        }
    } else {
        None
    };
    let path: &Path = converted.as_ref().map_or(file.path.as_path(), |output| output.as_ref());
    Ok(PreparedFile { input: InputFile::file(path.to_path_buf()), kind, converted })
}

pub(crate) async fn send_single_media(bot: &Bot, chat_id: ChatId, input: InputFile, kind: MediaKind,
    caption: &str, max_bytes: u64, allow_document_fallback: bool)
    -> Result<DeliveryReceipt, DeliveryError>
{
    let fallback_input = (allow_document_fallback && matches!(kind, MediaKind::Photo | MediaKind::Audio))
        .then(|| input.clone());
    let sent = match kind {
        MediaKind::Photo => bot.send_photo(chat_id, input).caption(caption)
            .parse_mode(ParseMode::Html).disable_notification(true).await,
        MediaKind::Video => bot.send_video(chat_id, input).caption(caption)
            .parse_mode(ParseMode::Html).supports_streaming(true).disable_notification(true).await,
        MediaKind::Audio => bot.send_audio(chat_id, input).caption(caption)
            .parse_mode(ParseMode::Html).disable_notification(true).await,
        MediaKind::Document => bot.send_document(chat_id, input).caption(caption)
            .parse_mode(ParseMode::Html).disable_notification(true).await,
    };
    match sent {
        Ok(message) => Ok(DeliveryReceipt { kind, message }),
        Err(error) => {
            if user_errors::rejected_media_upload(&error) {
                if let Some(input) = fallback_input {
                    return bot.send_document(chat_id, input).caption(caption)
                        .parse_mode(ParseMode::Html).disable_notification(true).await
                        .map(|message| DeliveryReceipt { kind: MediaKind::Document, message })
                        .map_err(|error| DeliveryError::request(error, max_bytes));
                }
            }
            Err(DeliveryError::request(error, max_bytes))
        }
    }
}

pub(crate) async fn send_media(bot: &Bot, chat_id: ChatId, file: &MediaFile, caption: &str,
    max_bytes: u64, deadline: time::Instant) -> Result<DeliveryReceipt, DeliveryError>
{
    let prepared = prepare_media(file, max_bytes, deadline).await?;
    // The converted TempPath remains owned here until the upload has completed.
    send_single_media(bot, chat_id, prepared.input, prepared.kind, caption, max_bytes, true).await
}

pub(crate) struct DeliveryItem<'a> {
    pub(crate) input: Option<InputFile>,
    pub(crate) kind: MediaKind,
    pub(crate) caption: String,
    pub(crate) description: Option<&'a str>,
}

pub(crate) struct DeliveryReport {
    pub(crate) delivered: usize,
    pub(crate) cached_items: Option<Vec<CachedMedia>>,
    pub(crate) error: Option<DeliveryError>,
}

impl DeliveryReport {
    pub(crate) fn new() -> Self {
        Self { delivered: 0, cached_items: Some(Vec::new()), error: None }
    }

    pub(crate) fn push(&mut self, kind: MediaKind, message: Message, description: Option<&str>) {
        self.delivered += 1;
        if self.cached_items.is_none() { return; }
        let file_id = match kind {
            MediaKind::Photo => message.photo().and_then(|photos| photos.iter()
                .max_by_key(|photo| u64::from(photo.width) * u64::from(photo.height)))
                .map(|photo| &photo.file.id.0),
            MediaKind::Video => message.video().map(|video| &video.file.id.0),
            MediaKind::Audio => message.audio().map(|audio| &audio.file.id.0),
            MediaKind::Document => message.document().map(|document| &document.file.id.0),
        };
        match file_id.filter(|id| !id.is_empty()) {
            Some(file_id) => self.cached_items.as_mut().expect("cache IDs are being collected").push(
                CachedMedia {
                    kind, file_id: file_id.clone(),
                    description: description.map(|text| {
                        let mut units = 0;
                        let end = text.char_indices().find_map(|(index, ch)| {
                            units += ch.len_utf16();
                            (units > 1024).then_some(index)
                        }).unwrap_or(text.len());
                        text[..end].to_owned()
                    }),
                }),
            None => self.cached_items = None,
        }
    }
}

pub(crate) fn album_compatible(first: MediaKind, next: MediaKind) -> bool {
    first == next || matches!((first, next),
        (MediaKind::Photo, MediaKind::Video) | (MediaKind::Video, MediaKind::Photo))
}

pub(crate) async fn deliver_items(bot: &Bot, chat_id: ChatId, mut items: Vec<DeliveryItem<'_>>,
    max_bytes: u64, allow_document_fallback: bool) -> DeliveryReport
{
    let mut report = DeliveryReport::new();
    if !allow_document_fallback { report.cached_items = None; }
    let mut start = 0;
    while start < items.len() {
        if items[start].input.is_none() {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < items.len() && end - start < 10
            && items[end].input.is_some()
            && album_compatible(items[start].kind, items[end].kind)
        {
            end += 1;
        }
        if end - start == 1 {
            let item = &mut items[start];
            let Some(input) = item.input.take() else { start = end; continue; };
            match send_single_media(bot, chat_id, input,
                item.kind, &item.caption, max_bytes, allow_document_fallback).await
            {
                Ok(receipt) => report.push(receipt.kind, receipt.message, item.description),
                Err(error) => { report.error = Some(error); break; }
            }
        } else {
            let media: Vec<_> = items[start..end].iter_mut().filter_map(|item| {
                let input = item.input.take()?;
                let caption = std::mem::take(&mut item.caption);
                Some(match item.kind {
                    MediaKind::Photo => InputMedia::Photo(InputMediaPhoto::new(input)
                        .caption(caption).parse_mode(ParseMode::Html)),
                    MediaKind::Video => InputMedia::Video(InputMediaVideo::new(input)
                        .caption(caption).parse_mode(ParseMode::Html).supports_streaming(true)),
                    MediaKind::Audio => InputMedia::Audio(InputMediaAudio::new(input)
                        .caption(caption).parse_mode(ParseMode::Html)),
                    MediaKind::Document => InputMedia::Document(InputMediaDocument::new(input)
                        .caption(caption).parse_mode(ParseMode::Html)),
                })
            }).collect();
            match bot.send_media_group(chat_id, media).disable_notification(true).await {
                Ok(messages) if messages.len() == end - start => {
                    for (item, message) in items[start..end].iter().zip(messages) {
                        report.push(item.kind, message, item.description);
                    }
                }
                Ok(_) => {
                    // A successful but unexpected response may already have delivered the album.
                    report.error = Some(DeliveryError {
                        reason: "Telegram вернул некорректный ответ на отправку альбома.".into(),
                        diagnostic: FailureDiagnostic::new("delivery", "Invalid response"),
                        invalid_cached_file: false,
                    });
                    break;
                }
                Err(error) => {
                    report.error = Some(DeliveryError::request(error, max_bytes));
                    break;
                }
            }
        }
        start = end;
    }
    report
}

pub(crate) async fn deliver_fresh(bot: &Bot, chat_id: ChatId, result: &DownloadResult,
    sender: &teloxide::types::User, url: &Url,
    max_bytes: u64, deadline: time::Instant)
    -> DeliveryReport
{
    let description = |file: &MediaFile| result.file_descriptions.iter()
        .find(|(path, _)| path == &file.path).map(|(_, text)| text.as_str())
        .or(result.description.as_deref());
    let mut report = DeliveryReport::new();
    // A one-item publication uses the same preparation and singleton upload entry point.
    if let [file] = result.selection.files.as_slice() {
        let description = description(file);
        let caption = match media_caption(sender, url, description) {
            Ok(caption) => caption,
            Err(reason) => {
                report.error = Some(DeliveryError {
                    reason: reason.into(),
                    diagnostic: FailureDiagnostic::new("caption", "Caption too long"),
                    invalid_cached_file: false,
                });
                return report;
            }
        };
        match send_media(bot, chat_id, file, &caption, max_bytes, deadline).await {
            Ok(receipt) => report.push(receipt.kind, receipt.message, description),
            Err(error) => report.error = Some(error),
        }
        return report;
    }
    let mut items = Vec::with_capacity(result.selection.files.len());
    let mut converted = Vec::new();
    let mut preparation_error = None;
    for file in &result.selection.files {
        let description = description(file);
        let caption = match media_caption(sender, url, description) {
            Ok(caption) => caption,
            Err(reason) => {
                report.error = Some(DeliveryError {
                    reason: reason.into(),
                    diagnostic: FailureDiagnostic::new("caption", "Caption too long"),
                    invalid_cached_file: false,
                });
                return report;
            }
        };
        match prepare_media(file, max_bytes, deadline).await {
            Ok(prepared) => {
                items.push(DeliveryItem { input: Some(prepared.input), kind: prepared.kind, caption, description });
                if let Some(output) = prepared.converted { converted.push(output); }
            }
            Err(error) => preparation_error = Some(error),
        }
    }
    // All conversion guards remain live while contiguous compatibility batches are uploaded.
    let mut report = deliver_items(bot, chat_id, items, max_bytes, true).await;
    if report.error.is_none() { report.error = preparation_error; }
    report
}

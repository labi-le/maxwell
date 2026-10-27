use crate::FailureDiagnostic;
use teloxide::{ApiError, RequestError};

pub(crate) fn http_detail(status: u16) -> &'static str {
    match status {
        401 => "Authentication required",
        403 => "HTTP 403",
        404 => "HTTP 404",
        408 => "Timeout",
        429 => "HTTP 429",
        500 => "HTTP 500",
        502 => "HTTP 502",
        503 => "HTTP 503",
        504 => "HTTP 504",
        501 | 505..=599 => "HTTP 5xx",
        _ => "HTTP error",
    }
}

pub(crate) fn diagnostic_message(diagnostic: FailureDiagnostic, max_bytes: u64) -> String {
    if diagnostic.stage == "delivery" {
        return match diagnostic.detail {
            "Rate limit" | "HTTP 429" =>
                "Telegram временно ограничил отправку медиа (429). Попробуйте позже.",
            "File too large" | "Size limit" =>
                "Telegram отклонил медиа: размер файла превышает его лимит.",
            "Insufficient rights" =>
                "У бота недостаточно прав для отправки медиа в этот чат.",
            "Bot blocked" => "Бот заблокирован пользователем и не может отправить медиа.",
            "Bot kicked" => "Бот удалён из чата и не может отправить медиа.",
            "Invalid file ID" =>
                "Telegram отклонил идентификатор сохранённого файла (file_id).",
            "Media format rejected" => "Telegram не смог обработать формат медиа.",
            "Timeout" =>
                "Истекло время ожидания ответа Telegram; доставка медиа не подтверждена.",
            "Network error" =>
                "Сетевая ошибка при связи с Telegram; доставка медиа не подтверждена.",
            "Invalid response" =>
                "Telegram вернул некорректный ответ; доставка медиа не подтверждена.",
            "File I/O error" => "Не удалось прочитать файл для отправки в Telegram.",
            "Chat migrated" => "Чат перенесён в супергруппу; отправка в прежний чат невозможна.",
            "Chat unavailable" => "Чат недоступен для отправки медиа.",
            "Bot authorization failed" => "Telegram отклонил авторизацию бота.",
            _ => "Telegram отклонил отправку медиа; причина неизвестна.",
        }.to_owned();
    }

    let message = match diagnostic.detail {
        "HTTP 403" => "Сайт отказал в доступе к публикации или медиа (HTTP 403).",
        "HTTP 404" => "Публикация или медиа не найдены на сайте (HTTP 404).",
        "HTTP 429" => "Сайт временно ограничил частоту запросов (HTTP 429). Попробуйте позже.",
        "HTTP 500" => "На стороне сайта произошла ошибка сервера (HTTP 500). Попробуйте позже.",
        "HTTP 502" => "Сайт временно недоступен из-за ошибки сервера (HTTP 502). Попробуйте позже.",
        "HTTP 503" => "Сайт временно недоступен (HTTP 503). Попробуйте позже.",
        "HTTP 504" => "Сервер сайта не дождался ответа (HTTP 504). Попробуйте позже.",
        "HTTP 5xx" => "На стороне сайта произошла временная ошибка сервера (HTTP 5xx). Попробуйте позже.",
        "HTTP error" => "Сайт вернул ошибку HTTP; точная причина неизвестна.",
        "Private video" => "Публикация закрытая или приватная; сайт не предоставил доступ к медиа.",
        "Video unavailable" => "Публикация или её медиа недоступны на сайте.",
        "Region blocked" => "Публикация недоступна в текущем регионе.",
        "Members only" => "Публикация доступна только участникам канала.",
        "Format unavailable" => "Сайт не выдал выбранный формат видео; попробуйте ещё раз.",
        "Download interrupted" => "Соединение с сайтом прервалось во время загрузки; попробуйте ещё раз.",
        "Processing failed" => "Не удалось обработать скачанный файл (ffmpeg).",
        "Site access restricted" => "Сайт ограничил доступ к этой публикации и не предоставил медиа.",
        "Audience restricted" => "Сайт ограничил аудиторию этой публикации и не предоставил медиа.",
        "Authentication required" => "Сайт требует авторизацию для доступа к публикации.",
        "Age verification required" => "Сайт требует подтверждения возраста для доступа к медиа.",
        "Cookies expired" => "Авторизация на сайте истекла: требуется обновить cookies.",
        "Cookies unavailable" => "Не удалось подготовить cookies для авторизации на сайте.",
        "Timeout" => "Превышено время ожидания загрузки медиа с сайта.",
        "Network error" => "Сетевая ошибка при загрузке медиа с сайта.",
        "File I/O error" => "Не удалось прочитать или сохранить файлы загрузки во временном хранилище.",
        "Empty photo" => "Сайт вернул пустой файл фото.",
        "Temporary storage limit" => "Достигнут общий лимит временного хранилища для загрузки медиа.",
        "Size limit" | "File too large" => {
            return format!("Размер медиа превышает допустимый лимит ({max_bytes} байт).");
        }
        "Unsupported URL" => "Этот адрес публикации не поддерживается загрузчиком.",
        "Unsupported format" => "Формат медиа не поддерживается.",
        "Extractor failed" => "Загрузчик не смог извлечь медиа публикации; точная причина неизвестна.",
        "Webpage unavailable" => "Загрузчик не смог получить страницу публикации; точная причина неизвестна.",
        "API unavailable" => "Загрузчик не смог получить данные от сайта; точная причина неизвестна.",
        "No sendable media" | "Media not found" => "В публикации не найдено медиа для отправки.",
        "Metadata failed" => "Не удалось получить метаданные публикации; точная причина неизвестна.",
        "Photo metadata failed" => "Не удалось получить сведения о фото публикации; точная причина неизвестна.",
        "Photo download failed" => "Не удалось загрузить фото с сайта; точная причина неизвестна.",
        "Launch failed" => "Не удалось запустить загрузчик медиа.",
        "Attempt failed" => "Процесс загрузчика медиа завершился с ошибкой; точная причина неизвестна.",
        "Download failed" => {
            let code = diagnostic.exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "неизвестен".into());
            return format!("Загрузчик не смог скачать медиа (код {code}); точная причина не распознана.");
        }
        "Preparation failed" => "Не удалось подготовить повторную загрузку медиа.",
        "Resolution failed" => "Не удалось разрешить короткую ссылку; точная причина неизвестна.",
        "Media skipped" => "Некоторые медиа пропущены; точная причина неизвестна.",
        _ => "Не удалось загрузить медиа; причина неизвестна.",
    };
    message.to_owned()
}

pub(crate) fn request_diagnostic(error: &RequestError) -> FailureDiagnostic {
    let detail = match error {
        RequestError::Api(error) => api_detail(error),
        RequestError::RetryAfter(_) => "Rate limit",
        RequestError::Network(error) if error.is_timeout() => "Timeout",
        RequestError::Network(_) => "Network error",
        RequestError::InvalidJson { .. } => "Invalid response",
        RequestError::Io(_) => "File I/O error",
        RequestError::MigrateToChatId(_) => "Chat migrated",
    };
    FailureDiagnostic::new("delivery", detail)
}

pub(crate) fn request_message(error: &RequestError, max_bytes: u64) -> String {
    if let RequestError::RetryAfter(seconds) = error {
        return format!(
            "Telegram временно ограничил отправку медиа (429). Повторите попытку через {} сек.",
            seconds.seconds(),
        );
    }
    diagnostic_message(request_diagnostic(error), max_bytes)
}

pub(crate) fn invalid_cached_file(error: &RequestError) -> bool {
    // Only teloxide's definitive typed ID errors permit a fresh-download restart.
    // An unrecognised API rejection may be ambiguous even if it mentions file IDs.
    matches!(error, RequestError::Api(
        ApiError::WrongFileId | ApiError::WrongFileIdOrUrl | ApiError::FileIdInvalid
    ))
}

pub(crate) fn rejected_media_upload(error: &RequestError) -> bool {
    match error {
        RequestError::Api(ApiError::ImageProcessFailed) => true,
        RequestError::Api(ApiError::Unknown(description)) => format_rejection(description),
        _ => false,
    }
}

fn format_rejection(description: &str) -> bool {
    [
        "Bad Request: invalid media content type",
        "Bad Request: IMAGE_PROCESS_FAILED",
        "Bad Request: PHOTO_INVALID_DIMENSIONS",
    ].iter().any(|known| description.eq_ignore_ascii_case(known))
}

fn api_detail(error: &ApiError) -> &'static str {
    match error {
        ApiError::WrongFileId | ApiError::WrongFileIdOrUrl | ApiError::FileIdInvalid => "Invalid file ID",
        ApiError::RequestEntityTooLarge => "File too large",
        ApiError::ImageProcessFailed => "Media format rejected",
        ApiError::NotEnoughRightsToPostMessages
        | ApiError::NotEnoughRightsToPinMessage
        | ApiError::NotEnoughRightsToManagePins
        | ApiError::NotEnoughRightsToChangeChatPermissions
        | ApiError::NotEnoughRightsToRestrict => "Insufficient rights",
        ApiError::BotBlocked => "Bot blocked",
        ApiError::BotKicked | ApiError::BotKickedFromSupergroup | ApiError::BotKickedFromChannel => "Bot kicked",
        ApiError::ChatNotFound => "Chat unavailable",
        ApiError::InvalidToken => "Bot authorization failed",
        ApiError::Unknown(description) if format_rejection(description) => "Media format rejected",
        ApiError::Unknown(description) => unknown_api_detail(description),
        _ => "Unknown Telegram rejection",
    }
}

fn unknown_api_detail(description: &str) -> &'static str {
    // Never format, store, or forward the API description: it may contain URLs,
    // tokens, cookies, or raw JSON. Match only canonical leading cause phrases.
    let description = ["Bad Request:", "Forbidden:", "Unauthorized:"].iter()
        .find_map(|prefix| description.get(..prefix.len())
            .filter(|leading| leading.eq_ignore_ascii_case(prefix))
            .map(|_| description[prefix.len()..].trim_start()))
        .unwrap_or(description);

    for (phrase, detail) in [
        ("not enough rights", "Insufficient rights"),
        ("need administrator rights", "Insufficient rights"),
        ("have no rights to send a message", "Insufficient rights"),
        ("CHAT_WRITE_FORBIDDEN", "Insufficient rights"),
        ("CHAT_SEND_MEDIA_FORBIDDEN", "Insufficient rights"),
        ("CHAT_SEND_PHOTOS_FORBIDDEN", "Insufficient rights"),
        ("CHAT_SEND_VIDEOS_FORBIDDEN", "Insufficient rights"),
        ("CHAT_SEND_AUDIOS_FORBIDDEN", "Insufficient rights"),
        ("CHAT_SEND_DOCS_FORBIDDEN", "Insufficient rights"),
        ("bot was blocked by the user", "Bot blocked"),
        ("bot was kicked from a chat", "Bot kicked"),
        ("bot was kicked from the supergroup chat", "Bot kicked"),
        ("bot was kicked from the channel chat", "Bot kicked"),
        ("wrong file identifier/HTTP URL specified", "Invalid file ID"),
        ("wrong file id", "Invalid file ID"),
        ("invalid file id", "Invalid file ID"),
        ("wrong remote file identifier specified", "Invalid file ID"),
        ("file is too big", "File too large"),
        ("file is too large", "File too large"),
        ("Request Entity Too Large", "File too large"),
    ] {
        if description.get(..phrase.len()).is_some_and(|leading| leading.eq_ignore_ascii_case(phrase))
            && description[phrase.len()..].chars().next().is_none_or(|next|
                next.is_ascii_whitespace() || matches!(next, ':' | '.' | ';'))
        {
            return detail;
        }
    }
    "Unknown Telegram rejection"
}

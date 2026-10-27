use crate::prelude::*;
use crate::*;

// A message that carries anything besides the link (the sender's own words or
// attached media) is the user's content, not a disposable link container.
pub(crate) fn has_content_beyond_link(message: &Message) -> bool {
    if message.photo().is_some() || message.video().is_some() || message.audio().is_some()
        || message.voice().is_some() || message.document().is_some()
        || message.animation().is_some() || message.sticker().is_some()
        || message.video_note().is_some()
    {
        return true;
    }
    let Some(text) = message.text().or_else(|| message.caption()) else { return true };
    let Some(url) = first_link(text) else { return true };
    source_comment(text, &url).is_some()
}

pub(crate) async fn delete_source(bot: &Bot, message: &Message) {
    if has_content_beyond_link(message) { return; }
    if bot.delete_message(message.chat.id, message.id).await.is_err() {
        let warning = if message.chat.is_private() {
            "Медиа доставлены, но не удалось удалить исходную ссылку."
        } else {
            "Медиа доставлены, но не удалось удалить исходную ссылку. Назначьте бота администратором группы с правом удаления сообщений (can_delete_messages)."
        };
        let _ = bot.send_message(message.chat.id, warning).disable_notification(true).await;
    }
}

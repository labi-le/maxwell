use crate::*;

pub(crate) async fn handle_command(bot: &Bot, message: &Message, cfg: &Settings, command: &str) {
    if !message.chat.is_group() && !message.chat.is_supergroup() {
        if message.chat.is_private()
            && message
                .from
                .as_ref()
                .is_some_and(|user| user.id == cfg.owner_id)
        {
            let _ = bot.send_message(message.chat.id, "Команды /maksut status|on|off работают только в группе; в личном чате ссылки доступны только владельцу бота.").disable_notification(true).await;
        }
        return;
    }
    if !message
        .from
        .as_ref()
        .is_some_and(|user| user.id == cfg.owner_id)
    {
        return;
    }
    let result = match command {
        "status" => group_enabled(&cfg.database, message.chat.id).await,
        "on" => set_group_enabled(&cfg.database, message.chat.id, true)
            .await
            .map(|()| true),
        "off" => set_group_enabled(&cfg.database, message.chat.id, false)
            .await
            .map(|()| false),
        _ => {
            let _ = bot
                .send_message(
                    message.chat.id,
                    "Используйте /maksut status, /maksut on или /maksut off.",
                )
                .disable_notification(true)
                .await;
            return;
        }
    };
    let reply = match result {
        Ok(true) => "Максут включён в этой группе.",
        Ok(false) => "Максут выключен в этой группе.",
        Err(_) => "Не удалось прочитать или сохранить настройки группы.",
    };
    let _ = bot
        .send_message(message.chat.id, reply)
        .disable_notification(true)
        .await;
}

pub(crate) fn maksut_command<'a>(text: &'a str, username: &str) -> Option<&'a str> {
    let mut parts = text.split_whitespace();
    let name = parts.next()?;
    let addressed = name == "/maksut"
        || name
            .strip_prefix("/maksut@")
            .is_some_and(|target| !target.is_empty() && target.eq_ignore_ascii_case(username));
    addressed.then(|| parts.next().unwrap_or(""))
}

/// Replies with the sticker for the first standalone «да»/«нет» in the message.
/// Only the owner can trigger actions; other participants are read-only.
async fn reply_with_sticker(bot: &Bot, message: &Message, cfg: &Settings, text: &str) -> bool {
    if !message
        .from
        .as_ref()
        .is_some_and(|user| user.id == cfg.owner_id)
    {
        return false;
    }
    // A standalone «да»/«нет» must not hide a pending plan/deploy gate from
    // the model. The read-only probe writes nothing.
    let improve_dir = cfg.self_improve_dir.clone();
    let owner_id = cfg.owner_id;
    let has_pending = tokio::task::spawn_blocking(move || {
        crate::self_improve::has_pending_decision(&improve_dir, owner_id)
    })
    .await
    .unwrap_or(false);
    if has_pending {
        return false;
    }
    let Some(unique_id) = sticker_rule(text) else {
        return false;
    };
    let Some(file_id) = cfg.sticker_replies.file_id(unique_id) else {
        return false;
    };
    if !message.chat.is_private()
        && !group_enabled(&cfg.database, message.chat.id)
            .await
            .is_ok_and(|enabled| enabled)
    {
        return false;
    }
    let request = bot.send_sticker(message.chat.id, InputFile::file_id(file_id.clone()));
    let request = if message.chat.is_private() {
        request.reply_parameters(ReplyParameters::new(message.id))
    } else {
        request
    };
    if let Ok(sent) = request.disable_notification(true).await {
        let _ = crate::memory::record_message(
            &cfg.database,
            RecordMessage::new(message.chat.id, 0, "максут", "assistant", "[стикер]")
                .with_message_id(Some(sent.id.0))
                .with_flags(crate::memory::FLAG_JUNK),
        )
        .await;
    }
    true
}

fn user_record<'a>(
    message: &Message,
    author_id: i64,
    author_name: &'a str,
    text: &'a str,
    flags: i64,
) -> RecordMessage<'a> {
    RecordMessage::new(message.chat.id, author_id, author_name, "user", text)
        .with_message_id(Some(message.id.0))
        .with_flags(flags)
}

pub(crate) async fn handle_message(
    bot: Bot,
    message: Message,
    cfg: Arc<Settings>,
) -> ResponseResult<()> {
    let text = message.text().or_else(|| message.caption()).unwrap_or("");
    let current_media = message_image(&message);
    let reply_media = message.reply_to_message().and_then(message_image);
    if text.is_empty() && current_media.is_none() && reply_media.is_none() {
        return Ok(());
    }
    let Some(sender) = message.from.as_ref().filter(|user| !user.is_bot) else {
        return Ok(());
    };
    let owner = sender.id == cfg.owner_id;
    let sender_name = sender
        .username
        .clone()
        .unwrap_or_else(|| sender.first_name.clone());
    let log_text = if text.trim().is_empty() {
        "[вложение]"
    } else {
        text
    };
    let mut flags = crate::memory::detect_flags(log_text);
    if text.trim().is_empty() && (current_media.is_some() || reply_media.is_some()) {
        flags |= crate::memory::FLAG_JUNK;
    }
    let now = crate::memory::unix_now();
    if !owner {
        match crate::memory::ignore_active(&cfg.database, sender.id, now).await {
            Ok(true) => {
                let flags = flags | crate::memory::FLAG_IGNORED;
                let _ = crate::memory::record_message(
                    &cfg.database,
                    user_record(&message, sender.id.0 as i64, &sender_name, log_text, flags),
                )
                .await;
                return Ok(());
            }
            Ok(false) => {}
            Err(error) => eprintln!("Ignore lookup failed: {error}"),
        }
        let group = message.chat.is_group() || message.chat.is_supergroup();
        if group
            && crate::memory::injection_attempt(log_text)
            && group_enabled(&cfg.database, message.chat.id)
                .await
                .is_ok_and(|enabled| enabled)
        {
            if let Err(error) = crate::memory::ignore_user(
                &cfg.database,
                sender.id,
                now,
                message.chat.id,
                message.id.0,
            )
            .await
            {
                eprintln!("Ignore write failed: {error}");
            }
            let flags = flags | crate::memory::FLAG_INJECTION | crate::memory::FLAG_IGNORED;
            let _ = crate::memory::record_message(
                &cfg.database,
                user_record(&message, sender.id.0 as i64, &sender_name, log_text, flags),
            )
            .await;
            return Ok(());
        }
    }
    let mut media = current_media;
    if media.is_none() {
        if let Some(reply_media) = reply_media {
            let reply_author = message
                .reply_to_message()
                .and_then(|reply| reply.from.as_ref());
            let reply_ignored = match reply_author {
                Some(user) if user.id != cfg.owner_id => {
                    crate::memory::ignore_active(&cfg.database, user.id, now)
                        .await
                        .is_ok_and(|active| active)
                }
                _ => false,
            };
            if !reply_ignored {
                media = Some(reply_media);
            }
        }
    }
    if text.is_empty() && media.is_none() {
        return Ok(());
    }
    if text.trim_start().starts_with('/') {
        flags |= crate::memory::FLAG_JUNK;
        let _ = crate::memory::record_message(
            &cfg.database,
            user_record(&message, sender.id.0 as i64, &sender_name, log_text, flags),
        )
        .await;
        if crate::self_improve::handle_improve(&bot, &message, &cfg, text).await {
            return Ok(());
        }
        if let Some(command) = maksut_command(text, &cfg.bot_username) {
            handle_command(&bot, &message, &cfg, command).await;
        }
        return Ok(());
    }
    if message.chat.is_private() && sender.id != cfg.owner_id {
        let _ = crate::memory::record_message(
            &cfg.database,
            user_record(&message, sender.id.0 as i64, &sender_name, log_text, flags),
        )
        .await;
        return Ok(());
    }
    if !message.chat.is_private() && !message.chat.is_group() && !message.chat.is_supergroup() {
        return Ok(());
    }
    let _ = crate::memory::record_message(
        &cfg.database,
        user_record(&message, sender.id.0 as i64, &sender_name, log_text, flags),
    )
    .await;
    if owner
        && message.chat.is_private()
        && crate::self_improve::handle_owner_text(&bot, &message, &cfg, text).await
    {
        return Ok(());
    }
    let Some(url) = first_link(text) else {
        if reply_with_sticker(&bot, &message, &cfg, text).await {
            return Ok(());
        }
        let reply_to_bot = message
            .reply_to_message()
            .and_then(|reply| reply.from.as_ref())
            .is_some_and(|user| {
                user.is_bot
                    && user
                        .username
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&cfg.bot_username))
            });
        let private = message.chat.is_private();
        let prompt = match chat_prompt(text, private, &cfg.bot_username, reply_to_bot) {
            Some(prompt) => Some(prompt),
            None if media.is_some() && (private || reply_to_bot) => Some(String::new()),
            None => None,
        };
        if let Some(prompt) = prompt {
            let image = match media.as_ref() {
                Some((file_id, mime)) => download_image(&bot, file_id, mime).await,
                None => None,
            };
            let prompt = if media.is_some() && image.is_none() {
                format!("{prompt}\n[не удалось загрузить вложение]")
            } else {
                prompt
            };
            let turn = ChatTurn {
                chat_id: message.chat.id,
                user_id: sender.id,
                sender_name: sender_name.as_str(),
                message_id: message.id.0,
                owner,
                prompt: prompt.as_str(),
                image: image.as_ref(),
            };
            reply_with_chat(&bot, &message, &cfg, &turn).await;
        }
        return Ok(());
    };
    let user_semaphore = cfg.user_rate_limit(sender.id).await;
    let Ok(_user_permit) = user_semaphore.try_acquire_owned() else {
        record_failure(
            &cfg.database,
            &url,
            FailureDiagnostic::new("queue", "Rate limit"),
        )
        .await;
        return Ok(());
    };
    if !message.chat.is_private() {
        match group_enabled(&cfg.database, message.chat.id).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(_) => {
                record_failure(
                    &cfg.database,
                    &url,
                    FailureDiagnostic::new("settings", "Database unavailable"),
                )
                .await;
                return Ok(());
            }
        }
    }
    let Ok(_permit) = cfg.downloads.acquire().await else {
        record_failure(
            &cfg.database,
            &url,
            FailureDiagnostic::new("queue", "Download unavailable"),
        )
        .await;
        return Ok(());
    };
    // Recheck after queueing: an administrator may have disabled the group while we waited.
    if !message.chat.is_private() {
        match group_enabled(&cfg.database, message.chat.id).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(_) => {
                record_failure(
                    &cfg.database,
                    &url,
                    FailureDiagnostic::new("settings", "Database unavailable"),
                )
                .await;
                return Ok(());
            }
        }
    }
    let key = cache_key(bot.token(), &url);
    if let Ok(Some(cached)) = lookup_cache(&cfg.database, message.chat.id.0, &key).await {
        let mut items = Vec::with_capacity(cached.len());
        for item in &cached {
            let caption = match media_caption(sender, &url, item.description.as_deref()) {
                Ok(caption) => caption,
                Err(reason) => {
                    record_failure(
                        &cfg.database,
                        &url,
                        FailureDiagnostic::new("caption", "Caption too long"),
                    )
                    .await;
                    let _ = bot
                        .send_message(message.chat.id, reason)
                        .disable_notification(true)
                        .await;
                    return Ok(());
                }
            };
            items.push(DeliveryItem {
                input: Some(InputFile::file_id(FileId(item.file_id.clone()))),
                kind: item.kind,
                caption,
                description: item.description.as_deref(),
            });
        }
        let report = deliver_items(&bot, message.chat.id, items, cfg.max_media_bytes, false).await;
        match report.error {
            None => {
                delete_source(&bot, &message).await;
                return Ok(());
            }
            Some(error) => {
                if error.invalid_cached_file {
                    let _ = invalidate_cache(&cfg.database, message.chat.id.0, &key).await;
                }
                if !error.invalid_cached_file || report.delivered != 0 {
                    record_failure(&cfg.database, &url, error.diagnostic).await;
                    let warning = format!(
                        "Не все медиа доставлены; исходная ссылка сохранена. {}",
                        error.reason
                    );
                    let _ = bot
                        .send_message(message.chat.id, warning)
                        .disable_notification(true)
                        .await;
                    return Ok(());
                }
                // Definitively invalid IDs before any delivery permit one fresh attempt below.
            }
        }
    }
    let dir = match tempfile::Builder::new()
        .prefix("post-")
        .tempdir_in(&cfg.download_dir)
    {
        Ok(dir) => dir,
        Err(_) => {
            record_failure(
                &cfg.database,
                &url,
                FailureDiagnostic::new("storage", "Download directory unavailable"),
            )
            .await;
            let _ = bot
                .send_message(message.chat.id, "Не удалось создать папку загрузки.")
                .disable_notification(true)
                .await;
            return Ok(());
        }
    };
    let deadline = time::Instant::now() + DOWNLOAD_TIMEOUT;
    let result = download_with_timeout(
        &url,
        &cfg,
        dir.path(),
        deadline.saturating_duration_since(time::Instant::now()),
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(failure) => {
            record_failure_with_events(&cfg.database, &url, failure.diagnostic, &failure.events)
                .await;
            let reply = if failure.diagnostic.detail == "Download failed"
                && failure.diagnostic.stage != "metadata"
            {
                failure.reason
            } else {
                user_errors::diagnostic_message(
                    source_user_diagnostic(failure.diagnostic, &failure.events),
                    cfg.max_media_bytes,
                )
            };
            let _ = bot
                .send_message(message.chat.id, reply)
                .disable_notification(true)
                .await;
            return Ok(());
        }
    };
    if result.selection.files.is_empty() {
        let diagnostic = result.diagnostic.unwrap_or_else(|| {
            FailureDiagnostic::new(
                "selection",
                if result.selection.size_skipped != 0 {
                    "Size limit"
                } else if result.selection.skipped != 0 {
                    "Media skipped"
                } else {
                    "No sendable media"
                },
            )
        });
        let mut events = result.events;
        if events.is_empty() {
            trace_event(&mut events, diagnostic);
        }
        record_failure_with_events(&cfg.database, &url, diagnostic, &events).await;
        let reply = user_errors::diagnostic_message(
            source_user_diagnostic(diagnostic, &events),
            cfg.max_media_bytes,
        );
        let _ = bot
            .send_message(message.chat.id, reply)
            .disable_notification(true)
            .await;
        return Ok(());
    }
    let report = deliver_fresh(
        &bot,
        message.chat.id,
        &result,
        sender,
        &url,
        cfg.max_media_bytes,
        deadline,
    )
    .await;
    let found = result.selection.files.len();
    let skipped = result.selection.skipped;
    let complete = result.process_ok
        && !result.limited
        && !result.twitter_photos_failed
        && result.diagnostic.is_none()
        && skipped == 0
        && report.delivered == found
        && report.error.is_none();
    if complete {
        if let Some(items) = report.cached_items {
            let _ = store_cache(&cfg.database, message.chat.id.0, &key, &items).await;
        }
        delete_source(&bot, &message).await;
        return Ok(());
    }
    if !complete {
        let diagnostic = if let Some(error) = &report.error {
            error.diagnostic
        } else if let Some(failure) = result.diagnostic {
            failure
        } else if result.twitter_photos_failed {
            FailureDiagnostic::new("metadata", "Photo metadata failed")
        } else {
            FailureDiagnostic::new(
                "selection",
                if result.selection.size_skipped != 0 {
                    "Size limit"
                } else {
                    "Media skipped"
                },
            )
        };
        let user_diagnostic = source_user_diagnostic(diagnostic, &result.events);
        let mut events = result.events;
        if skipped != 0 && diagnostic.stage != "selection" {
            trace_event(
                &mut events,
                FailureDiagnostic::new(
                    "selection",
                    if result.selection.size_skipped != 0 {
                        "Size limit"
                    } else {
                        "Media skipped"
                    },
                ),
            );
        }
        if report.error.is_some() || events.is_empty() {
            trace_event(&mut events, diagnostic);
        }
        record_failure_with_events(&cfg.database, &url, diagnostic, &events).await;
        let mut warning = String::from("Не все медиа доставлены; исходная ссылка сохранена.");
        if result.limited {
            warning.push_str(" Достигнут лимит места.");
        } else if !result.process_ok {
            warning.push_str(" Загрузка завершилась с ошибкой.");
        }
        if result.twitter_photos_failed {
            warning.push_str(" Фото X не удалось извлечь.");
        }
        if skipped != 0 {
            warning.push_str(" Некоторые файлы пропущены.");
        }
        warning.push(' ');
        if let Some(error) = report.error {
            warning.push_str(&error.reason);
        } else {
            warning.push_str(&user_errors::diagnostic_message(
                user_diagnostic,
                cfg.max_media_bytes,
            ));
        }
        let _ = bot
            .send_message(message.chat.id, warning)
            .disable_notification(true)
            .await;
    }
    Ok(())
}

pub(crate) async fn handle_callback_query(
    bot: Bot,
    query: teloxide::types::CallbackQuery,
    cfg: Arc<Settings>,
) -> ResponseResult<()> {
    if query.from.id != cfg.owner_id
        && crate::memory::ignore_active(&cfg.database, query.from.id, crate::memory::unix_now())
            .await
            .is_ok_and(|active| active)
    {
        return Ok(());
    }
    crate::self_improve::handle_callback(&bot, &query, &cfg).await;
    Ok(())
}

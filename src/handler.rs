use crate::prelude::*;
use crate::*;

pub(crate) async fn handle_command(bot: &Bot, message: &Message, cfg: &Settings, command: &str) {
    if !message.chat.is_group() && !message.chat.is_supergroup() {
        if message.chat.is_private() && message.from.as_ref().is_some_and(|user| user.id == cfg.owner_id) {
            let _ = bot.send_message(message.chat.id, "Команды /maksut status|on|off работают только в группе; в личном чате ссылки доступны только владельцу бота.").disable_notification(true).await;
        }
        return;
    }
    let authorized = if message.from.as_ref().is_some_and(|user| user.id == cfg.owner_id) {
        true
    } else if let Some(user) = message.from.as_ref() {
        bot.get_chat_member(message.chat.id, user.id)
            .await
            .is_ok_and(|member| member.kind.is_privileged())
    } else {
        false
    };
    if !authorized {
        return;
    }
    let result = match command {
        "status" => group_enabled(&cfg.database, message.chat.id).await,
        "on" => set_group_enabled(&cfg.database, message.chat.id, true).await.map(|()| true),
        "off" => set_group_enabled(&cfg.database, message.chat.id, false).await.map(|()| false),
        _ => {
            let _ = bot.send_message(message.chat.id, "Используйте /maksut status, /maksut on или /maksut off.").disable_notification(true).await;
            return;
        }
    };
    let reply = match result {
        Ok(true) => "Максут включён в этой группе.",
        Ok(false) => "Максут выключен в этой группе.",
        Err(_) => "Не удалось прочитать или сохранить настройки группы.",
    };
    let _ = bot.send_message(message.chat.id, reply).disable_notification(true).await;
}

pub(crate) fn maksut_command<'a>(text: &'a str, username: &str) -> Option<&'a str> {
    let mut parts = text.split_whitespace();
    let name = parts.next()?;
    let addressed = name == "/maksut" || name.strip_prefix("/maksut@")
        .is_some_and(|target| !target.is_empty() && target.eq_ignore_ascii_case(username));
    addressed.then(|| parts.next().unwrap_or(""))
}

pub(crate) async fn handle_message(bot: Bot, message: Message, cfg: Arc<Settings>) -> ResponseResult<()> {
    let Some(text) = message.text().or_else(|| message.caption()) else { return Ok(()) };
    let Some(sender) = message.from.as_ref().filter(|user| !user.is_bot) else { return Ok(()) };
    if text.trim_start().starts_with('/') {
        if let Some(command) = maksut_command(text, &cfg.bot_username) {
            handle_command(&bot, &message, &cfg, command).await;
        }
        return Ok(());
    }
    if message.chat.is_private() {
        if sender.id != cfg.owner_id { return Ok(()) }
    } else if !message.chat.is_group() && !message.chat.is_supergroup() {
        return Ok(());
    }
    let Some(url) = first_link(text) else { return Ok(()) };
    let user_semaphore = {
        let mut limits = cfg.user_rate_limits.write().await;
        Arc::clone(limits.entry(sender.id)
            .or_insert_with(|| Arc::new(Semaphore::new(USER_DOWNLOAD_LIMIT))))
    };
    let Ok(_user_permit) = user_semaphore.try_acquire_owned() else {
        record_failure(&cfg.database, &url, FailureDiagnostic::new("queue", "Rate limit")).await;
        return Ok(());
    };
    if !message.chat.is_private() {
        match group_enabled(&cfg.database, message.chat.id).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(_) => {
                record_failure(&cfg.database, &url, FailureDiagnostic::new("settings", "Database unavailable")).await;
                return Ok(());
            }
        }
    }
    let Ok(_permit) = cfg.downloads.acquire().await else {
        record_failure(&cfg.database, &url, FailureDiagnostic::new("queue", "Download unavailable")).await;
        return Ok(());
    };
    // Recheck after queueing: an administrator may have disabled the group while we waited.
    if !message.chat.is_private() {
        match group_enabled(&cfg.database, message.chat.id).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(_) => {
                record_failure(&cfg.database, &url, FailureDiagnostic::new("settings", "Database unavailable")).await;
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
                    record_failure(&cfg.database, &url, FailureDiagnostic::new("caption", "Caption too long")).await;
                    let _ = bot.send_message(message.chat.id, reason).disable_notification(true).await;
                    return Ok(());
                }
            };
            items.push(DeliveryItem {
                input: Some(InputFile::file_id(FileId(item.file_id.clone()))),
                kind: item.kind, caption, description: item.description.as_deref(),
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
                    let warning = format!("Не все медиа доставлены; исходная ссылка сохранена. {}", error.reason);
                    let _ = bot.send_message(message.chat.id, warning).disable_notification(true).await;
                    return Ok(());
                }
                // Definitively invalid IDs before any delivery permit one fresh attempt below.
            }
        }
    }
    let dir = match tempfile::Builder::new().prefix("post-").tempdir_in(&cfg.download_dir) {
        Ok(dir) => dir,
        Err(_) => {
            record_failure(&cfg.database, &url, FailureDiagnostic::new("storage", "Download directory unavailable")).await;
            let _ = bot.send_message(message.chat.id, "Не удалось создать папку загрузки.").disable_notification(true).await;
            return Ok(());
        }
    };
    let deadline = time::Instant::now() + DOWNLOAD_TIMEOUT;
    let result = download_with_timeout(&url, &cfg, dir.path(),
        deadline.saturating_duration_since(time::Instant::now())).await;
    let result = match result {
        Ok(result) => result,
        Err(failure) => {
            record_failure_with_events(&cfg.database, &url, failure.diagnostic, &failure.events).await;
            let reply = if failure.diagnostic.detail == "Download failed"
                && failure.diagnostic.stage != "metadata"
            {
                failure.reason
            } else {
                user_errors::diagnostic_message(
                    source_user_diagnostic(failure.diagnostic, &failure.events), cfg.max_media_bytes)
            };
            let _ = bot.send_message(message.chat.id, reply).disable_notification(true).await;
            return Ok(());
        }
    };
    if result.selection.files.is_empty() {
        let diagnostic = result.diagnostic.unwrap_or_else(|| {
            FailureDiagnostic::new("selection", if result.selection.size_skipped != 0 {
                "Size limit"
            } else if result.selection.skipped != 0 { "Media skipped" } else { "No sendable media" })
        });
        let mut events = result.events;
        if events.is_empty() { trace_event(&mut events, diagnostic); }
        record_failure_with_events(&cfg.database, &url, diagnostic, &events).await;
        let reply = user_errors::diagnostic_message(
            source_user_diagnostic(diagnostic, &events), cfg.max_media_bytes);
        let _ = bot.send_message(message.chat.id, reply).disable_notification(true).await;
        return Ok(());
    }
    let report = deliver_fresh(&bot, message.chat.id, &result, sender, &url,
        cfg.max_media_bytes, deadline).await;
    let found = result.selection.files.len();
    let skipped = result.selection.skipped;
    let complete = result.process_ok && !result.limited && !result.twitter_photos_failed
        && result.diagnostic.is_none() && skipped == 0 && report.delivered == found
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
            FailureDiagnostic::new("selection", if result.selection.size_skipped != 0 {
                "Size limit"
            } else { "Media skipped" })
        };
        let user_diagnostic = source_user_diagnostic(diagnostic, &result.events);
        let mut events = result.events;
        if skipped != 0 && diagnostic.stage != "selection" {
            trace_event(&mut events, FailureDiagnostic::new("selection",
                if result.selection.size_skipped != 0 { "Size limit" } else { "Media skipped" }));
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
                user_diagnostic, cfg.max_media_bytes));
        }
        let _ = bot.send_message(message.chat.id, warning).disable_notification(true).await;
    }
    Ok(())
}

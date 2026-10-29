use crate::prelude::*;
use crate::*;

pub(crate) async fn download_videos(
    url: &Url,
    cfg: &Settings,
    dir: &Path,
    duration: Duration,
) -> Result<DownloadResult, DownloadFailure> {
    let deadline = time::Instant::now() + duration;
    let selected_format = if youtube_single_video(url) {
        match probe_youtube_mp4(url, cfg, dir, deadline).await {
            Ok(format) => format,
            Err(detail) => {
                return Err(DownloadFailure::new(
                    "Видео слишком длинное или это стрим; такие ролики не скачиваются.".into(),
                    "metadata",
                    detail,
                ));
            }
        }
    } else {
        None
    };
    let (mut first, interrupted) =
        download_videos_attempt(url, cfg, dir, deadline, selected_format.as_deref())
            .await
            .map_err(|reason| DownloadFailure::new(reason, "yt-dlp", "Download failed"))?;
    if let Some(diagnostic) = first.diagnostic {
        trace_event(&mut first.events, diagnostic);
    }
    let auth_failure = !first.process_ok
        && !first.limited
        && !interrupted
        && first.selection.files.is_empty()
        && first
            .diagnostic
            .is_some_and(|diagnostic| is_cookie_auth_detail(diagnostic.detail));
    if auth_failure {
        if cfg.cookies_file.is_some()
            && time::Instant::now() < deadline
            && retryable_output(dir, cfg.max_media_bytes)
            && request_cookie_refresh(cfg, cookie_source(url), deadline, &mut first.events).await
            && time::Instant::now() < deadline
        {
            // Reuse the selected format, without a second metadata probe or fallback.
            if clear_failed_attempt(dir).is_ok() && time::Instant::now() < deadline {
                trace_event(
                    &mut first.events,
                    FailureDiagnostic::new("retry", "Started"),
                );
                match download_videos_attempt(url, cfg, dir, deadline, selected_format.as_deref())
                    .await
                {
                    Ok((mut retry, _)) => {
                        if let Some(diagnostic) = retry.diagnostic {
                            trace_event(&mut first.events, diagnostic);
                        }
                        if !retry.process_ok {
                            retry.diagnostic = first.diagnostic;
                        }
                        retry.events = first.events;
                        return Ok(retry);
                    }
                    Err(reason) => trace_event(
                        &mut first.events,
                        FailureDiagnostic::new(
                            "retry",
                            if reason == "Не удалось запустить yt-dlp" {
                                "Launch failed"
                            } else {
                                "Attempt failed"
                            },
                        ),
                    ),
                }
            } else {
                trace_event(
                    &mut first.events,
                    FailureDiagnostic::new("retry", "Preparation failed"),
                );
            }
        }
        return Ok(first);
    }
    if selected_format.is_none()
        || first.process_ok
        || first.limited
        || interrupted
        || !first.selection.files.is_empty()
        || time::Instant::now() >= deadline
        || !retryable_output(dir, cfg.max_media_bytes)
    {
        return Ok(first);
    }
    if let Err(reason) = clear_failed_attempt(dir) {
        return Err(DownloadFailure {
            reason,
            diagnostic: FailureDiagnostic::new("yt-dlp", "Download failed"),
            events: first.events,
        });
    }
    if time::Instant::now() >= deadline {
        return Ok(first);
    }
    trace_event(
        &mut first.events,
        FailureDiagnostic::new("format", "Fallback started"),
    );
    let (mut fallback, _) = match download_videos_attempt(url, cfg, dir, deadline, None).await {
        Ok(fallback) => fallback,
        Err(reason) => {
            trace_event(
                &mut first.events,
                FailureDiagnostic::new("format", "Fallback failed"),
            );
            return Err(DownloadFailure {
                reason,
                diagnostic: FailureDiagnostic::new("yt-dlp", "Download failed"),
                events: first.events,
            });
        }
    };
    if let Some(diagnostic) = fallback.diagnostic {
        trace_event(&mut first.events, diagnostic);
    }
    fallback.events = first.events;
    Ok(fallback)
}

pub(crate) async fn download_videos_attempt(
    url: &Url,
    cfg: &Settings,
    dir: &Path,
    deadline: time::Instant,
    selected_format: Option<&str>,
) -> Result<(DownloadResult, bool), String> {
    let cookie = process_cookies(cfg.cookies_file.as_deref(), dir)?;
    let manifest = dir.join("completed-paths.txt");
    let output = dir.join("%(playlist_index|0)s-%(id)s-%(autonumber)s.%(ext)s");
    let mut cmd = Command::new(&cfg.ytdlp_bin);
    cmd.arg("--quiet")
        .arg("--no-progress")
        .arg("--max-downloads")
        .arg(MAX_FILES.to_string())
        .arg("--playlist-end")
        .arg(MAX_FILES.to_string())
        .arg("--max-filesize")
        .arg(cfg.max_media_bytes.to_string())
        .arg("--output")
        .arg(output)
        .arg("--print-to-file")
        .arg("after_move:filepath")
        .arg(&manifest)
        .arg("--print")
        .arg("after_move:%(filepath)j\t%(title)j\t%(description)j")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // TikTok returns Site Maintenance for Chrome-like UAs on this network;
    // metadata and downloads work with Firefox's User-Agent.
    if url
        .host_str()
        .is_some_and(|host| short_link_host_allowed(host, "tiktok"))
    {
        cmd.arg("--add-header")
            .arg(format!("User-Agent: {TIKTOK_USER_AGENT}"));
        // HEVC renditions can be silent even when yt-dlp reports an AAC codec.
        cmd.arg("-S").arg("res,vcodec:h264");
    }
    let filter_applied = matches!(
        url.host_str(),
        Some(
            "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be"
        )
    );
    if filter_applied {
        // yt-dlp rejects live/upcoming and overlong entries before opening a
        // media stream. This is the backstop when the metadata probe fails.
        cmd.arg("--match-filter").arg(youtube_duration_filter());
    }
    if image_source(url).is_some() {
        cmd.arg("--ignore-no-formats-error");
    }
    if let Some((_, path)) = &cookie {
        cmd.arg("--cookies").arg(path);
        if matches!(
            url.host_str(),
            Some(
                "youtube.com"
                    | "www.youtube.com"
                    | "m.youtube.com"
                    | "music.youtube.com"
                    | "youtu.be"
            )
        ) {
            cmd.arg("--extractor-args")
                .arg("youtube:player_client=default,web_embedded");
        }
    }
    if let Some(format) = selected_format {
        cmd.arg("-f")
            .arg(format)
            .arg("--merge-output-format")
            .arg("mp4");
    } else {
        // Never fall back to a video-only stream; keep standalone audio supported.
        cmd.arg("-f").arg("bv+ba/b[acodec!=none]/ba");
    }
    cmd.arg("--").arg(url.as_str());
    #[cfg(unix)]
    cmd.as_std_mut().process_group(0);
    let child = cmd
        .spawn()
        .map_err(|_| "Не удалось запустить yt-dlp".to_string())?;
    let mut process = DownloadProcess::new(child);
    process.capture_stdout();
    // Checking once per second detects storage overflow before it becomes critical.
    let mut interval = time::interval_at(
        time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(1),
    );
    let timeout = time::sleep_until(deadline);
    tokio::pin!(timeout);
    let mut limited = false;
    let mut interrupted = false;
    let status = loop {
        tokio::select! {
            result = process.wait() => break result.map_err(|_| "Ошибка процесса yt-dlp".to_string())?,
            _ = &mut timeout => {
                interrupted = true;
                process.stop();
                break process.wait().await.map_err(|_| "Ошибка процесса yt-dlp".to_string())?;
            }
            _ = interval.tick() => {
                let usage = match disk_usage(dir) {
                    Ok(usage) => usage,
                    Err(_) => {
                        process.stop();
                        let _ = process.wait().await;
                        return Err("Ошибка проверки места для загрузки".into());
                    }
                };
                if usage > cfg.max_media_bytes.saturating_mul(MAX_FILES as u64) {
                    limited = true;
                    process.stop();
                    break process.wait().await.map_err(|_| "Ошибка процесса yt-dlp".to_string())?;
                }
            }
        }
    };
    let label = process.stderr_label(deadline).await;
    let selection = select_media(dir, &manifest, cfg.max_media_bytes)
        .map_err(|_| "Ошибка чтения загруженных файлов".to_string())?;
    let file_descriptions = process
        .stdout_records(deadline)
        .await
        .into_iter()
        .filter(|(path, _)| selection.files.iter().any(|file| file.path == *path))
        .collect();
    // yt-dlp uses exit code 101 for --max-downloads, not just genuine failures.
    let process_ok = !interrupted
        && !limited
        && (status.success() || (status.code() == Some(101) && selection.files.len() == MAX_FILES));
    let size_skipped = selection.size_skipped != 0 || label == "Size limit";
    let diagnostic =
        (!process_ok || size_skipped || status.code() == Some(101)).then_some(FailureDiagnostic {
            stage: if !interrupted && !limited && (size_skipped || status.code() == Some(101)) {
                "selection"
            } else {
                "yt-dlp"
            },
            detail: if interrupted {
                "Timeout"
            } else if limited {
                "Temporary storage limit"
            } else if size_skipped {
                "Size limit"
            } else if status.code() == Some(101) {
                "Media skipped"
            } else {
                label
            },
            exit_code: status.code(),
        });
    Ok((
        DownloadResult {
            selection,
            process_ok,
            limited,
            twitter_photos_failed: false,
            description: None,
            file_descriptions,
            diagnostic,
            events: Vec::new(),
        },
        interrupted,
    ))
}

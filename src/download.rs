use crate::prelude::*;
use crate::*;

pub(crate) struct DownloadResult {
    pub(crate) selection: Selection,
    pub(crate) description: Option<String>,
    pub(crate) file_descriptions: Vec<(PathBuf, String)>,
    pub(crate) process_ok: bool,
    pub(crate) limited: bool,
    pub(crate) twitter_photos_failed: bool,
    pub(crate) diagnostic: Option<FailureDiagnostic>,
    pub(crate) events: Vec<FailureDiagnostic>,
}

#[cfg(test)]
pub(crate) async fn download(url: &Url, cfg: &Settings, dir: &Path) -> Result<DownloadResult, String> {
    download_with_timeout(url, cfg, dir, DOWNLOAD_TIMEOUT).await.map_err(|failure| failure.reason)
}

pub(crate) async fn tiktok_metadata_with_refresh(
    url: &Url, cfg: &Settings, dir: &Path, deadline: time::Instant,
) -> Result<ImageMetadata, DownloadFailure> {
    let first = extract_tiktok_images(url, cfg, deadline).await;
    let first = match first {
        Ok(metadata) => return Ok(metadata),
        Err(failure) => failure,
    };
    let mut events = Vec::new();
    if !refresh_after_auth_failure(cfg, "tiktok", dir, deadline, &mut events, &first).await {
        let mut first = first;
        first.events = events;
        return Err(first);
    }
    if time::Instant::now() >= deadline {
        let mut first = first;
        let mut merged = first.events;
        merged.extend(events);
        trace_event(&mut merged, FailureDiagnostic::new("retry", "Timed out"));
        first.events = merged;
        return Err(first);
    }
    match extract_tiktok_images(url, cfg, deadline).await {
        Ok(metadata) => Ok(metadata),
        Err(mut second) => {
            let original = first.diagnostic;
            let mut merged = first.events;
            merged.extend(events);
            trace_event(&mut merged, second.diagnostic);
            second.diagnostic = original;
            second.events = merged;
            Err(second)
        }
    }
}

pub(crate) async fn images_metadata_with_refresh(
    url: &Url, cfg: &Settings, source: &str, dir: &Path, deadline: time::Instant,
) -> Result<ImageMetadata, DownloadFailure> {
    let first = extract_images(url, cfg, source, dir,
        deadline.saturating_duration_since(time::Instant::now())).await;
    let first = match first {
        Ok(metadata) => return Ok(metadata),
        Err(failure) => failure,
    };
    let mut events = Vec::new();
    if !refresh_after_auth_failure(cfg, source, dir, deadline, &mut events, &first).await {
        let mut first = first;
        first.events = events;
        return Err(first);
    }
    if time::Instant::now() >= deadline {
        let mut first = first;
        let mut merged = first.events;
        merged.extend(events);
        trace_event(&mut merged, FailureDiagnostic::new("retry", "Timed out"));
        first.events = merged;
        return Err(first);
    }
    match extract_images(url, cfg, source, dir,
        deadline.saturating_duration_since(time::Instant::now())).await
    {
        Ok(metadata) => Ok(metadata),
        Err(mut second) => {
            let original = first.diagnostic;
            let mut merged = first.events;
            merged.extend(events);
            trace_event(&mut merged, second.diagnostic);
            second.diagnostic = original;
            second.events = merged;
            Err(second)
        }
    }
}

pub(crate) async fn download_with_timeout(
    url: &Url, cfg: &Settings, dir: &Path, duration: Duration,
) -> Result<DownloadResult, DownloadFailure> {
    let deadline = time::Instant::now() + duration;
    let mut refresh_events = Vec::new();
    if let Some(path) = &cfg.cookies_file {
        if validate_cookie_jar(path).is_err() {
            if !request_cookie_refresh(cfg, cookie_source(url), deadline, &mut refresh_events).await
                || validate_cookie_jar(path).is_err()
            {
                let mut failure = DownloadFailure::new(
                    "Не удалось подготовить cookies".into(), "preflight", "Cookies unavailable");
                failure.events = refresh_events;
                return Err(failure);
            }
        }
        if process_cookies(Some(path), dir).is_err() {
            let mut failure = DownloadFailure::new(
                "Не удалось подготовить временный каталог".into(), "preflight", "File I/O error");
            failure.events = refresh_events;
            return Err(failure);
        }
    }
    let resolved = if short_link_family(url).is_some() {
        Some(resolve_short_link(url, deadline).await.ok_or_else(|| DownloadFailure::new(
            "Не удалось разрешить короткую ссылку".into(), "short-link", "Resolution failed"))?)
    } else {
        None
    };
    let url = resolved.as_ref().unwrap_or(url);
    let source = image_source(url);
    let mut twitter_failed = false;
    let mut twitter_events = Vec::new();
    let mut photo_diagnostic = None;
    // The helper may time out; start yt-dlp at the same time so finished videos
    // are still available when photo extraction fails.
    let (metadata, video_result) = if source == Some("twitter") && twitter_status_id(url).is_some() {
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        let (photos, video) = tokio::join!(
            extract_twitter_images(url, cfg, dir, remaining),
            download_videos(url, cfg, dir, remaining)
        );
        let video_has_files = video.as_ref().is_ok_and(|result| !result.selection.files.is_empty());
        let photos = match photos {
            Ok(photos) => Some(photos),
            Err(error) => {
                twitter_failed = true;
                let detail = if error.auth { "Authentication required" } else { "Photo metadata failed" };
                photo_diagnostic = Some(FailureDiagnostic::new("metadata", detail));
                trace_event(&mut twitter_events, FailureDiagnostic::new("metadata", detail));
                if error.auth && cfg.cookies_file.is_some()
                    && time::Instant::now() < deadline
                    && (retryable_output(dir, cfg.max_media_bytes) || video_has_files)
                {
                    let mut refresh_events = Vec::new();
                    if request_cookie_refresh(cfg, "twitter", deadline, &mut refresh_events).await {
                        if time::Instant::now() >= deadline {
                            twitter_events.append(&mut refresh_events);
                            trace_event(&mut twitter_events,
                                FailureDiagnostic::new("retry", "Timed out"));
                            None
                        } else {
                            let remaining = deadline.saturating_duration_since(time::Instant::now());
                            match extract_twitter_images(url, cfg, dir, remaining).await {
                                Ok(recovered) => {
                                    twitter_failed = false;
                                    photo_diagnostic = None;
                                    twitter_events.clear();
                                    Some(recovered)
                                }
                                Err(retry_error) => {
                                    twitter_events.append(&mut refresh_events);
                                    let detail = if retry_error.auth {
                                        "Authentication required"
                                    } else {
                                        "Photo metadata failed"
                                    };
                                    trace_event(&mut twitter_events,
                                        FailureDiagnostic::new("metadata", detail));
                                    None
                                }
                            }
                        }
                    } else {
                        twitter_events.append(&mut refresh_events);
                        None
                    }
                } else {
                    None
                }
            }
        };
        (photos, Some(video))
    } else if source == Some("tiktok") {
        (Some(tiktok_metadata_with_refresh(url, cfg, dir, deadline).await?), None)
    } else if let Some(source @ ("pinterest" | "instagram")) = source {
        (Some(images_metadata_with_refresh(url, cfg, source, dir, deadline).await?), None)
    } else {
        (None, None)
    };
    let manifest = dir.join("completed-paths.txt");
    let mut result = if metadata.as_ref().is_none_or(|data| data.has_video) {
        let video = match video_result {
            Some(video) => video,
            None => download_videos(url, cfg, dir, deadline.saturating_duration_since(time::Instant::now())).await,
        };
        match video {
            Ok(result) => result,
            Err(error) if source == Some("twitter") && metadata.as_ref().is_some_and(|data| !data.images.is_empty()) => {
                DownloadResult { selection: Selection { files: Vec::new(), skipped: 0, size_skipped: 0 },
                    process_ok: false, limited: false, twitter_photos_failed: false,
                    diagnostic: Some(error.diagnostic),
                    description: None, file_descriptions: Vec::new(),
                    events: error.events }
            }
            Err(mut error) => {
                if twitter_failed {
                    error.events.extend(std::mem::take(&mut twitter_events));
                }
                return Err(error);
            }
        }
    } else {
        DownloadResult {
            selection: Selection { files: Vec::new(), skipped: 0, size_skipped: 0 },
            process_ok: true, limited: false, twitter_photos_failed: false, diagnostic: None,
            description: None, file_descriptions: Vec::new(),
            events: Vec::new(),
        }
    };
    if let Some(metadata) = metadata {
        result.description = metadata.description;
        result.selection.skipped += metadata.skipped;
        let max_images = if source == Some("tiktok") { MAX_TIKTOK_PHOTOS } else { MAX_FILES };
        if let Err(failures) = add_images(&metadata.images, &metadata.alternates, cfg, dir,
            &manifest, max_images, &mut result.selection, deadline).await
        {
            result.process_ok = false;
            for diagnostic in failures {
                result.diagnostic.get_or_insert(diagnostic);
                trace_event(&mut result.events, diagnostic);
            }
        }
    }
    if twitter_failed {
        result.process_ok = false;
        result.events.extend(twitter_events);
        let photo = photo_diagnostic.unwrap_or_else(|| {
            FailureDiagnostic::new("metadata", "Photo metadata failed")
        });
        if result.diagnostic.is_none()
            || result.diagnostic.is_some_and(|diagnostic| diagnostic.stage == "yt-dlp"
                && diagnostic.detail == "Download failed")
        {
            result.diagnostic = Some(photo);
        }
        if result.events.iter().all(|event| event.stage != "metadata") {
            trace_event(&mut result.events, FailureDiagnostic::new("metadata", "Photo metadata failed"));
        }
    }
    result.twitter_photos_failed = twitter_failed;
    Ok(result)
}

pub(crate) fn retryable_output(dir: &Path, max_bytes: u64) -> bool {
    // A manifest entry may be finished but unsendable; never discard it.
    let manifest = dir.join("completed-paths.txt");
    match fs::read_to_string(&manifest) {
        Ok(paths) if !paths.trim().is_empty() => return false,
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return false,
        _ => {}
    }
    matches!(disk_usage(dir), Ok(usage) if usage <= max_bytes.saturating_mul(MAX_FILES as u64))
}

pub(crate) fn clear_failed_attempt(dir: &Path) -> Result<(), String> {
    // The previous yt-dlp process has exited; only incomplete outputs remain.
    for entry in fs::read_dir(dir).map_err(|_| "Ошибка очистки загрузки".to_string())? {
        let entry = entry.map_err(|_| "Ошибка очистки загрузки".to_string())?;
        let kind = entry.file_type().map_err(|_| "Ошибка очистки загрузки".to_string())?;
        if kind.is_file() || kind.is_symlink() {
            fs::remove_file(entry.path()).map_err(|_| "Ошибка очистки загрузки".to_string())?;
        }
    }
    Ok(())
}

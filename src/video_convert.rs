use crate::*;

pub(crate) fn convertible_video(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["webm", "mkv", "mov", "avi", "m4v", "3gp"]
                .iter()
                .any(|item| ext.eq_ignore_ascii_case(item))
        })
}

// Uppercase V excludes attached pictures, thumbnails and cover art.
pub(crate) async fn video_dimensions(
    path: &Path,
    deadline: time::Instant,
    selector: &str,
) -> Result<Option<(u32, u32)>, String> {
    let mut cmd = Command::new("ffprobe");
    cmd.arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg(selector)
        .arg("-show_entries")
        .arg("stream=width,height")
        .arg("-of")
        .arg("csv=p=0")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.as_std_mut().process_group(0);
    let child = cmd
        .spawn()
        .map_err(|_| "Не удалось запустить ffprobe".to_string())?;
    let mut process = DownloadProcess::new(child);
    let mut output = Vec::new();
    let result = time::timeout_at(deadline, async {
        process
            .child
            .as_mut()
            .expect("child is owned")
            .stdout
            .take()
            .expect("ffprobe stdout is piped")
            .take(128)
            .read_to_end(&mut output)
            .await?;
        process.wait().await
    })
    .await;
    match result {
        Ok(Ok(status)) if status.success() => {
            if output.is_empty() {
                return Ok(None);
            }
            let text = std::str::from_utf8(&output).map_err(|_| "Некорректные размеры видео")?;
            let (width, height) = text
                .trim()
                .split_once(',')
                .ok_or("Некорректные размеры видео")?;
            let width = width
                .parse::<u32>()
                .map_err(|_| "Некорректные размеры видео")?;
            let height = height
                .parse::<u32>()
                .map_err(|_| "Некорректные размеры видео")?;
            if width == 0 || height == 0 {
                return Err("Некорректные размеры видео".into());
            }
            Ok(Some((width, height)))
        }
        _ => Err("Не удалось проверить видеопоток".into()),
    }
}

pub(crate) async fn require_video_audio(
    path: &Path,
    deadline: time::Instant,
) -> Result<(), String> {
    let mut cmd = Command::new("ffprobe");
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=index",
        "-of",
        "csv=p=0",
    ])
    .arg(path)
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    #[cfg(unix)]
    cmd.as_std_mut().process_group(0);
    let child = cmd
        .spawn()
        .map_err(|_| "Не удалось запустить ffprobe".to_string())?;
    let mut process = DownloadProcess::new(child);
    let mut output = Vec::new();
    let result = time::timeout_at(deadline, async {
        process
            .child
            .as_mut()
            .expect("child is owned")
            .stdout
            .take()
            .expect("ffprobe stdout is piped")
            .take(128)
            .read_to_end(&mut output)
            .await?;
        process.wait().await
    })
    .await;
    match result {
        Ok(Ok(status)) if status.success() => {
            if std::str::from_utf8(&output)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
                .is_some()
            {
                Ok(())
            } else {
                Err("Видео без аудиодорожки не отправляется: в выбранном файле нет звука.".into())
            }
        }
        _ => Err("Не удалось проверить аудиодорожку видео".into()),
    }
}

pub(crate) fn check_video_dimensions((width, height): (u32, u32)) -> Result<(), String> {
    if width > MAX_VIDEO_DIMENSION
        || height > MAX_VIDEO_DIMENSION
        || width.saturating_mul(height) > MAX_VIDEO_PIXELS
    {
        return Err("Разрешение видео превышает допустимый предел".into());
    }
    Ok(())
}

pub(crate) async fn convert_video(
    path: &Path,
    dimensions: (u32, u32),
    max_bytes: u64,
    deadline: time::Instant,
) -> Result<tempfile::TempPath, String> {
    check_video_dimensions(dimensions)?;
    let parent = path
        .parent()
        .ok_or("Не удалось подготовить конвертацию")?
        .to_owned();
    let output = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix("inline-")
            .suffix(".mp4")
            .tempfile_in(&parent)
            .map(|file| file.into_temp_path())
    })
    .await
    .map_err(|_| "Не удалось подготовить конвертацию")?
    .map_err(|_| "Не удалось подготовить конвертацию")?;
    let output_path: &Path = output.as_ref();
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-nostdin").arg("-v").arg("error").arg("-y")
        .arg("-filter_threads").arg("2")
        .arg("-max_pixels").arg(MAX_VIDEO_PIXELS.to_string())
        .arg("-i").arg(path).arg("-map").arg("0:V:0").arg("-map").arg("0:a:0")
        .arg("-vf").arg("scale=w='min(1920,iw)':h='min(1920,ih)':force_original_aspect_ratio=decrease,pad=ceil(iw/2)*2:ceil(ih/2)*2")
        .arg("-c:v").arg("libx264").arg("-threads:v").arg("2")
        .arg("-pix_fmt").arg("yuv420p")
        .arg("-c:a").arg("aac").arg("-threads:a").arg("2")
        .arg("-movflags").arg("+faststart")
        .arg("-f").arg("mp4").arg(output_path)
        .stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.as_std_mut().process_group(0);
    let child = cmd
        .spawn()
        .map_err(|_| "Не удалось запустить ffmpeg".to_string())?;
    let mut process = DownloadProcess::new(child);
    let mut interval = time::interval_at(
        time::Instant::now() + Duration::from_millis(300),
        Duration::from_millis(300),
    );
    let failure = loop {
        tokio::select! {
            result = process.wait() => {
                if !result.is_ok_and(|status| status.success()) {
                    return Err("Не удалось конвертировать видео".into());
                }
                break None;
            }
            _ = time::sleep_until(deadline) => break Some("Превышено время конвертации видео"),
            _ = interval.tick() => {
                match tokio::fs::metadata(&output).await {
                    Ok(metadata) if metadata.len() > max_bytes => {
                        break Some("Видео после конвертации превышает лимит размера");
                    }
                    Ok(_) => {}
                    Err(_) => break Some("Ошибка проверки размера видео"),
                }
            }
        }
    };
    if let Some(reason) = failure {
        // ffmpeg still holds the output open, so it must be reaped before unlinking.
        process.stop();
        let _ = process.wait().await;
        let _ = tokio::fs::remove_file(output_path).await;
        return Err(reason.to_string());
    }
    let size = tokio::fs::metadata(&output)
        .await
        .map_err(|_| "Ошибка проверки размера видео")?
        .len();
    if size == 0 || size > max_bytes {
        return Err("Видео после конвертации пустое или превышает лимит размера".into());
    }
    Ok(output)
}

use crate::prelude::*;
use crate::*;

pub(crate) fn youtube_single_video(url: &Url) -> bool {
    if url.query_pairs().any(|(key, _)| key == "list") { return false; }
    match url.host_str() {
        Some("youtu.be") => true,
        Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com") => {
            matches!(url.path(), "/watch")
                || ["/shorts/", "/live/", "/embed/"].iter().any(|prefix| url.path().starts_with(prefix))
        }
        _ => false,
    }
}

pub(crate) fn youtube_format_id(format: &Value) -> Option<&str> {
    format.get("format_id").and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 16 && id.bytes().all(|byte| byte.is_ascii_digit()))
}

// Format IDs are passed to yt-dlp's -f expression, never a shell. Still restrict
// them to actual numeric YouTube IDs so metadata cannot inject format syntax.
pub(crate) fn choose_youtube_mp4(info: &Value, max_bytes: u64) -> Option<String> {
    if info.get("entries").is_some()
        || info.get("_type").and_then(Value::as_str).is_some_and(|kind| kind != "video")
        || !matches!(info.get("live_status").and_then(Value::as_str), Some("not_live" | "was_live"))
        || info.get("duration").and_then(Value::as_f64).is_none_or(|duration| !duration.is_finite() || duration <= 0.0)
    {
        return None;
    }
    let formats = info.get("formats")?.as_array()?;
    let known_size = |format: &Value| format.get("filesize").and_then(Value::as_u64).filter(|size| *size > 0);
    let fits = |video: u64, audio: u64| {
        video.checked_add(audio)
            .and_then(|total| total.checked_add(total / 100))
            .and_then(|total| total.checked_add(MP4_MUX_HEADROOM))
            .is_some_and(|total| total <= max_bytes)
    };
    type Choice = ((u64, f64, f64), usize, Option<usize>);
    let mut best: Option<Choice> = None;
    for (video_index, video) in formats.iter().enumerate() {
        if video.get("ext").and_then(Value::as_str) != Some("mp4")
            || !video.get("vcodec").and_then(Value::as_str).is_some_and(|codec| codec.starts_with("avc1"))
            || video.get("has_drm").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let (Some(_), Some(video_size), Some(width), Some(height)) = (
            youtube_format_id(video), known_size(video),
            video.get("width").and_then(Value::as_u64),
            video.get("height").and_then(Value::as_u64),
        ) else { continue };
        let Some(pixels) = width.checked_mul(height).filter(|pixels| *pixels > 0) else { continue };
        let fps = video.get("fps").and_then(Value::as_f64).filter(|fps| fps.is_finite() && *fps > 0.0).unwrap_or(0.0);
        let mut consider = |audio_index: Option<usize>, audio_size: u64, abr: f64| {
            if !fits(video_size, audio_size) { return; }
            let score = (pixels, fps, abr);
            if best.as_ref().is_none_or(|(old, _, _)| score > *old) {
                best = Some((score, video_index, audio_index));
            }
        };
        if video.get("acodec").and_then(Value::as_str).is_some_and(|codec| codec.starts_with("mp4a")) {
            consider(None, 0, video.get("abr").and_then(Value::as_f64).unwrap_or(0.0));
        } else if video.get("acodec").and_then(Value::as_str) == Some("none") {
            for (audio_index, audio) in formats.iter().enumerate() {
                if audio.get("ext").and_then(Value::as_str) != Some("m4a")
                    || audio.get("vcodec").and_then(Value::as_str) != Some("none")
                    || !audio.get("acodec").and_then(Value::as_str).is_some_and(|codec| codec.starts_with("mp4a"))
                    || audio.get("has_drm").and_then(Value::as_bool) == Some(true)
                {
                    continue;
                }
                if let (Some(_), Some(audio_size)) = (youtube_format_id(audio), known_size(audio)) {
                    consider(Some(audio_index), audio_size, audio.get("abr").and_then(Value::as_f64).unwrap_or(0.0));
                }
            }
        }
    }
    let (_, video_index, audio_index) = best?;
    let video = youtube_format_id(&formats[video_index])?;
    Some(match audio_index {
        Some(index) => format!("{video}+{}", youtube_format_id(&formats[index])?),
        None => video.to_string(),
    })
}

pub(crate) async fn probe_youtube_mp4(
    url: &Url, cfg: &Settings, dir: &Path, deadline: time::Instant,
) -> Option<String> {
    let cookie = process_cookies(cfg.cookies_file.as_deref(), dir).ok()?;
    let mut cmd = Command::new(&cfg.ytdlp_bin);
    cmd.arg("--quiet").arg("--no-warnings").arg("--skip-download")
        .arg("--no-playlist").arg("--dump-single-json")
        .stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    if let Some((_, path)) = &cookie {
        cmd.arg("--cookies").arg(path)
            .arg("--extractor-args").arg("youtube:player_client=default,web_embedded");
    }
    cmd.arg("--").arg(url.as_str());
    #[cfg(unix)]
    cmd.as_std_mut().process_group(0);
    let mut process = DownloadProcess::new(cmd.spawn().ok()?);
    let mut stdout = process.child.as_mut()?.stdout.take()?.take(MAX_METADATA_BYTES + 1);
    let probe_deadline = deadline.min(time::Instant::now() + YOUTUBE_PROBE_TIMEOUT);
    let (bytes, status) = time::timeout_at(probe_deadline, async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.ok()?;
        if bytes.len() as u64 > MAX_METADATA_BYTES { return None; }
        Some((bytes, process.wait().await.ok()?))
    }).await.ok()??;
    if !status.success() { return None; }
    choose_youtube_mp4(&serde_json::from_slice::<Value>(&bytes).ok()?, cfg.max_media_bytes)
}

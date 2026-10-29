use crate::feature_support::*;
use crate::test_support::*;
use crate::*;

#[tokio::test]
async fn real_webm_video_converts_but_audio_only_stays_document() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
        || std::process::Command::new("ffprobe")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
    {
        return; // ffmpeg/ffprobe are optional for local Rust-only builds.
    }
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("sample.webm");
    assert!(std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=33x31:rate=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-t",
            "1",
            "-c:v",
            "libvpx",
            "-c:a",
            "libopus",
            "-y"
        ])
        .arg(&video)
        .status()
        .unwrap()
        .success());
    let deadline = time::Instant::now() + Duration::from_secs(20);
    let dimensions = video_dimensions(&video, deadline, "V:0")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dimensions, (33, 31));
    let mp4 = convert_video(&video, dimensions, 1_000_000, deadline)
        .await
        .unwrap();
    let mp4_path: &Path = mp4.as_ref();
    let details = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name,pix_fmt,width,height",
            "-of",
            "json",
        ])
        .arg(mp4_path)
        .output()
        .unwrap();
    assert!(details.status.success());
    let streams: Value = serde_json::from_slice(&details.stdout).unwrap();
    assert_eq!(streams["streams"][0]["codec_name"], "h264");
    assert_eq!(streams["streams"][0]["pix_fmt"], "yuv420p");
    assert_eq!(streams["streams"][0]["width"], 34);
    assert_eq!(streams["streams"][0]["height"], 32);
    assert!(video.exists());
    drop(mp4);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(convert_video(
        &video,
        dimensions,
        1,
        time::Instant::now() + Duration::from_secs(20)
    )
    .await
    .is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

    let audio = dir.path().join("audio.webm");
    assert!(std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.2",
            "-c:a",
            "libopus",
            "-y"
        ])
        .arg(&audio)
        .status()
        .unwrap()
        .success());
    assert_eq!(media_kind(&audio), Some(MediaKind::Document));
    assert!(
        video_dimensions(&audio, time::Instant::now() + Duration::from_secs(5), "V:0")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(check_video_dimensions((1080, 1920)), Ok(()));
    assert!(check_video_dimensions((3841, 100)).is_err());
    assert!(check_video_dimensions((3840, 2161)).is_err());
    #[cfg(unix)]
    {
        let (bot, records, server) = recording_bot(true, true).await;
        let caption = "video caption";
        let deadline = time::Instant::now() + Duration::from_secs(20);
        send_media(
            &bot,
            ChatId(1),
            &MediaFile {
                path: video.clone(),
                kind: MediaKind::Document,
            },
            caption,
            1_000_000,
            deadline,
        )
        .await
        .unwrap();
        send_media(
            &bot,
            ChatId(1),
            &MediaFile {
                path: audio.clone(),
                kind: MediaKind::Document,
            },
            caption,
            1_000_000,
            deadline,
        )
        .await
        .unwrap();
        let calls = records.lock().await;
        assert!(
            api_method_is(&calls[0].0, "sendVideo"),
            "WebM video must be inline"
        );
        assert!(
            calls[0].1.contains(".mp4"),
            "converted upload must have MP4 filename"
        );
        assert!(
            api_method_is(&calls[1].0, "sendDocument"),
            "audio-only WebM is not video"
        );
        server.abort();
    }
}

#[tokio::test]
async fn attached_cover_is_never_converted_as_motion_video() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
        || std::process::Command::new("ffprobe")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mixed = dir.path().join("mixed.m4v");
    assert!(std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=1",
            "-f",
            "lavfi",
            "-i",
            "color=red:size=32x32:rate=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-map",
            "2:a",
            "-t",
            "1",
            "-frames:v:0",
            "1",
            "-frames:v:1",
            "1",
            "-c:a",
            "aac",
            "-c:v:0",
            "mpeg4",
            "-c:v:1",
            "mjpeg",
            "-disposition:v:1",
            "attached_pic",
            "-f",
            "mp4",
            "-y"
        ])
        .arg(&mixed)
        .status()
        .unwrap()
        .success());
    let deadline = time::Instant::now() + Duration::from_secs(20);
    let details = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=width,height:stream_disposition=attached_pic",
            "-of",
            "json",
        ])
        .arg(&mixed)
        .output()
        .unwrap();
    assert!(details.status.success());
    let streams: Value = serde_json::from_slice(&details.stdout).unwrap();
    assert!(streams["streams"]
        .as_array()
        .unwrap()
        .iter()
        .any(|stream| stream["width"] == 32
            && stream["height"] == 32
            && stream["disposition"]["attached_pic"] == 1));
    assert_eq!(
        video_dimensions(&mixed, deadline, "V:0").await.unwrap(),
        Some((64, 48))
    );
    let mp4 = convert_video(&mixed, (64, 48), 1_000_000, deadline)
        .await
        .unwrap();
    let mp4_path: &Path = mp4.as_ref();
    let details = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "V:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
        ])
        .arg(mp4_path)
        .output()
        .unwrap();
    assert!(details.status.success());
    let streams: Value = serde_json::from_slice(&details.stdout).unwrap();
    assert_eq!(streams["streams"][0]["width"], 64);
    assert_eq!(streams["streams"][0]["height"], 48);

    let cover_only = dir.path().join("cover-only.m4v");
    assert!(std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.2",
            "-f",
            "lavfi",
            "-i",
            "color=red:size=32x32:rate=1",
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-frames:v",
            "1",
            "-c:a",
            "aac",
            "-c:v",
            "mjpeg",
            "-disposition:v:0",
            "attached_pic",
            "-f",
            "mp4",
            "-y"
        ])
        .arg(&cover_only)
        .status()
        .unwrap()
        .success());
    let details = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=width,height:stream_disposition=attached_pic",
            "-of",
            "json",
        ])
        .arg(&cover_only)
        .output()
        .unwrap();
    assert!(details.status.success());
    let streams: Value = serde_json::from_slice(&details.stdout).unwrap();
    assert!(streams["streams"]
        .as_array()
        .unwrap()
        .iter()
        .any(|stream| stream["width"] == 32
            && stream["height"] == 32
            && stream["disposition"]["attached_pic"] == 1));
    assert_eq!(
        video_dimensions(&cover_only, deadline, "V:0")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        video_dimensions(&cover_only, deadline, "v:0")
            .await
            .unwrap(),
        Some((32, 32))
    );
    #[cfg(unix)]
    {
        let (bot, records, server) = recording_bot(true, true).await;
        let caption = "video caption";
        send_media(
            &bot,
            ChatId(1),
            &MediaFile {
                path: mixed,
                kind: MediaKind::Document,
            },
            caption,
            1_000_000,
            deadline,
        )
        .await
        .unwrap();
        assert!(send_media(
            &bot,
            ChatId(1),
            &MediaFile {
                path: cover_only,
                kind: MediaKind::Document
            },
            caption,
            1_000_000,
            deadline
        )
        .await
        .is_err());
        let calls = records.lock().await;
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _)| api_method_is(path, "sendVideo"))
                .count(),
            1
        );
        assert!(!calls
            .iter()
            .any(|(path, _)| api_method_is(path, "sendDocument")));
        server.abort();
    }
}

#[tokio::test]
async fn features_silent_or_unreadable_video_never_uploads_caches_or_deletes_source() {
    for extension in ["mp4", "webm", "mkv", "mov", "avi", "m4v", "3gp", "corrupt"] {
        let file_extension = if extension == "corrupt" {
            "mp4"
        } else {
            extension
        };
        let fixture = FeatureFixture::new(&[file_extension], "exit 0").await;
        let input = fixture.root.path().join(format!("silent.{file_extension}"));
        if extension == "corrupt" {
            fs::write(&input, b"not a media file").unwrap();
        } else {
            let codec = if extension == "webm" {
                "libvpx"
            } else {
                "libx264"
            };
            assert!(std::process::Command::new("ffmpeg")
                .args(["-nostdin", "-v", "error", "-i",])
                .arg(av_fixture())
                .args([
                    "-map",
                    "0:v:0",
                    "-an",
                    "-c:v",
                    codec,
                    "-threads:v",
                    "1",
                    "-y",
                ])
                .arg(&input)
                .status()
                .unwrap()
                .success());
        }
        let create = if file_extension == "mp4" {
            format!("cp '{}' \"$dir/asset-01.mp4\"", av_fixture().display())
        } else {
            format!("printf '%s' 'asset-00' > \"$dir/asset-01.{file_extension}\"")
        };
        let script = feature_script(fixture.root.path(), &[file_extension], "exit 0").replace(
            &create,
            &format!(
                "cp '{}' \"$dir/asset-01.{file_extension}\"",
                input.display()
            ),
        );
        fs::write(&fixture.cfg.ytdlp_bin, script).unwrap();
        fixture.send(1, 1, "Owner", LINK).await;
        let requests = fixture.api.snapshot().await;
        assert!(
            !requests.iter().any(|request| request.is_media()),
            "{extension}: silent/corrupt video leaked"
        );
        feature_retained(&requests);
        feature_reason(&requests, &["аудиодорожк"]);
        let cached: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_cache")
            .fetch_one(&fixture.cfg.database)
            .await
            .unwrap();
        assert_eq!(
            cached, 0,
            "{extension}: rejected video must not enter the file_id cache"
        );
    }
}

#[tokio::test]
async fn features_converted_video_stays_alive_in_visual_album_and_caches_as_video() {
    let fixture = FeatureFixture::new(&["jpg", "webm", "png"], "exit 0").await;
    let input = fixture.root.path().join("moving.webm");
    assert!(std::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=32x32:rate=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-t",
            "1",
            "-c:v",
            "libvpx",
            "-c:a",
            "libopus",
            "-y",
        ])
        .arg(&input)
        .status()
        .expect("project ffmpeg fixture prerequisite")
        .success());
    let script = feature_script(fixture.root.path(), &["jpg", "webm", "png"], "exit 0").replace(
        "printf '%s' 'asset-01' > \"$dir/asset-02.webm\"",
        &format!("cp '{}' \"$dir/asset-02.webm\"", input.display()),
    );
    fs::write(&fixture.cfg.ytdlp_bin, script).unwrap();
    fixture.send(1, 1, "Original", LINK).await;
    let requests = fixture.api.snapshot().await;
    let media: Vec<_> = requests
        .iter()
        .filter(|request| request.is_media())
        .collect();
    assert_eq!(
        media.len(),
        1,
        "conversion must happen before visual compatibility grouping"
    );
    assert_eq!(media[0].method, "sendmediagroup");
    let items = media[0].media();
    assert_eq!(
        items
            .iter()
            .map(|item| item["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["photo", "video", "photo"]
    );
    let video = media[0]
        .upload(&items[1])
        .expect("converted MP4 must remain readable during upload");
    assert!(
        video.windows(4).any(|bytes| bytes == b"ftyp"),
        "visual album must upload the converted MP4 container"
    );
    let delivered = fixture.root.path().join("delivered.mp4");
    fs::write(&delivered, video).unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ])
        .arg(&delivered)
        .output()
        .unwrap();
    assert!(probe.status.success());
    assert_eq!(
        String::from_utf8(probe.stdout).unwrap().trim(),
        "aac",
        "actual uploaded conversion must retain the audio track"
    );
    assert_eq!(items[1]["supports_streaming"], true);
    assert_eq!(requests.last().unwrap().method, "deletemessage");
    feature_silent(&requests);
    let calls = fixture.calls();
    fixture.api.clear().await;
    fixture.send(1, 1, "Current", LINK).await;
    assert_eq!(
        fixture.calls(),
        calls,
        "converted delivery should cache the actual video ID"
    );
    let requests = fixture.api.snapshot().await;
    let media: Vec<_> = requests
        .iter()
        .filter(|request| request.is_media())
        .collect();
    assert_eq!(media.len(), 1);
    assert_eq!(media[0].method, "sendmediagroup");
    assert!(media[0].uploads.is_empty());
    assert_eq!(media[0].media()[1]["type"], "video");
    assert_eq!(
        media[0].media()[1]["media"],
        "features-video-2",
        "conversion's original document classification must never corrupt the cached media kind"
    );
    feature_silent(&requests);
}

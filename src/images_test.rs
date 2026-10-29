use crate::test_support::*;
use crate::*;

#[test]
fn tiktok_photo_links_use_only_signed_public_image_cdn_urls() {
    let photo = allowed_url("https://www.tiktok.com/@name/photo/111").unwrap();
    assert_eq!(image_source(&photo), Some("tiktok"));

    for url in [
            "https://p16-sign-va.tiktokcdn.com/tos-useast2a-p-0037/first~tplv-photomode-image.jpeg?x-expires=1900000000&x-signature=abc",
            "https://p16-sign.tiktokcdn-us.com/tos-useast5-p-0068-tx/second~tplv-photomode-image.jpeg?x-expires=1900000000&x-signature=def",
        ] {
            assert_eq!(image_cdn_url(url, "tiktok").as_ref().map(Url::as_str), Some(url), "{url}");
        }
    for url in [
        "https://p16-sign-va.tiktokcdn.com.evil.test/photo.jpeg?x-signature=abc",
        "https://evil-tiktokcdn.com/photo.jpeg?x-signature=abc",
        "https://127.0.0.1/photo.jpeg?x-signature=abc",
        "https://10.0.0.1/photo.jpeg?x-signature=abc",
        "http://p16-sign-va.tiktokcdn.com/photo.jpeg?x-signature=abc",
        "https://p16-sign-va.tiktokcdn.com:443/photo.jpeg?x-signature=abc",
        "https://user@p16-sign-va.tiktokcdn.com/photo.jpeg?x-signature=abc",
    ] {
        assert!(image_cdn_url(url, "tiktok").is_none(), "{url}");
    }
}

#[test]
fn tiktok_photo_metadata_yields_ordered_images_without_cover_or_audio() {
    let first = "https://p16-sign-va.tiktokcdn.com/tos-useast2a-p-0037/first~tplv-photomode-image.jpeg?x-expires=1900000000&x-signature=abc";
    let second = "https://p16-sign.tiktokcdn-us.com/tos-useast5-p-0068-tx/second~tplv-photomode-image.jpeg?x-expires=1900000000&x-signature=def";
    let info = serde_json::json!({
        "__DEFAULT_SCOPE__": {"webapp.video-detail": {"itemInfo": {"itemStruct": {
            "imagePost": {"images": [
                {"imageURL": {"urlList": [first]}},
                {"imageURL": {"urlList": ["https://evil.test/photo.jpeg"]}},
                {"imageURL": {"urlList": [second]}},
                {"imageURL": {"urlList": ["http://p16-sign-va.tiktokcdn.com/photo.jpeg"]}},
                {"imageURL": {"urlList": ["https://127.0.0.1/private.jpeg"]}}
            ]},
            "cover": "https://p16-sign-va.tiktokcdn.com/cover.jpeg",
            "music": {"playUrl": "https://p16-sign-va.tiktokcdn.com/audio.mp3"}
        }}}}
    });
    let images = collect_images(&info, "tiktok");
    assert_eq!(
        images.images.iter().map(Url::as_str).collect::<Vec<_>>(),
        vec![first, second]
    );
    assert!(!images.has_video);
}

#[test]
fn tiktok_photo_metadata_keeps_up_to_thirty_five_images() {
    let urls: Vec<String> = (0..40)
            .map(|index| format!("https://p16-sign-va.tiktokcdn.com/tos-useast2a-p-0037/{index}~tplv-photomode-image.jpeg?x-expires=1900000000&x-signature=abc"))
            .collect();
    let info = serde_json::json!({
        "__DEFAULT_SCOPE__": {"webapp.video-detail": {"itemInfo": {"itemStruct": {
            "imagePost": {"images": urls.iter()
                .map(|url| serde_json::json!({"imageURL": {"urlList": [url]}}))
                .collect::<Vec<_>>()}
        }}}}
    });
    let images = collect_images(&info, "tiktok");
    assert_eq!(images.images.len(), MAX_TIKTOK_PHOTOS);
    assert_eq!(images.skipped, urls.len() - MAX_TIKTOK_PHOTOS);
}

#[test]
fn image_metadata_uses_full_size_only_for_photo_entries() {
    let pinterest = serde_json::json!({
        "formats": [],
        "thumbnails": [
            {"url": "https://i.pinimg.com/236x/photo.jpg", "width": 236, "height": 236},
            {"url": "https://i.pinimg.com/originals/photo.jpg", "width": 1500, "height": 2200}
        ]
    });
    let pin = collect_images(&pinterest, "pinterest");
    assert!(!pin.has_video);
    assert_eq!(pin.images[0].path(), "/originals/photo.jpg");
    let instagram = serde_json::json!({
        "entries": [
            {"formats": [], "thumbnails": [
                {"url": "https://scontent.cdninstagram.com/small.jpg", "width": 150, "height": 150},
                {"url": "https://scontent.cdninstagram.com/large.jpg", "width": 1200, "height": 1200}
            ]},
            {"formats": [{"url": "https://video.example/video.mp4"}],
             "thumbnails": [{"url": "https://scontent.cdninstagram.com/cover.jpg", "width": 9000, "height": 9000}]}
        ]
    });
    let post = collect_images(&instagram, "instagram");
    assert!(post.has_video);
    assert_eq!(post.images.len(), 1);
    assert_eq!(post.images[0].path(), "/large.jpg");
}

#[test]
fn rejects_untrusted_photo_sources_and_private_dns_addresses() {
    for url in [
        "http://i.pinimg.com/photo.jpg",
        "https://i.pinimg.com:443/photo.jpg",
        "https://user@i.pinimg.com/photo.jpg",
        "https://i.pinimg.com.evil.test/photo.jpg",
        "https://127.0.0.1/photo.jpg",
    ] {
        assert!(image_cdn_url(url, "pinterest").is_none(), "{url}");
    }
    assert!(image_cdn_url("https://evil.fbcdn.net.evil.test/photo.jpg", "instagram").is_none());
    for address in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.1.1",
        "::1",
    ] {
        assert!(!permitted_ipv4(address.parse().unwrap()), "{address}");
    }
    assert!(permitted_ipv4("1.1.1.1".parse().unwrap()));
    assert!(permitted_ipv4("198.18.1.215".parse().unwrap()));
}

#[test]
fn twitter_photos_reject_unsafe_origins_and_non_images() {
    for raw in [
        "http://pbs.twimg.com/media/p.jpg",
        "https://pbs.twimg.com:443/media/p.jpg",
        "https://user@pbs.twimg.com/media/p.jpg",
        "https://pbs.twimg.com.evil.test/media/p.jpg",
        "https://pbs.twimg.com@127.0.0.1/media/p.jpg",
        "https://pbs.twimg.com/profile_images/p.jpg",
    ] {
        assert!(image_cdn_url(raw, "twitter").is_none(), "{raw}");
    }
    let url = Url::parse("https://pbs.twimg.com/media/p?format=jpg&name=orig").unwrap();
    assert_eq!(
        twitter_image_extension(&url, Some("image/jpeg")),
        Some("jpg")
    );
    assert_eq!(twitter_image_extension(&url, Some("text/html")), None);
    assert_eq!(twitter_image_extension(&url, Some("image/png")), None);
    let from_header = Url::parse("https://pbs.twimg.com/media/p").unwrap();
    assert_eq!(
        twitter_image_extension(&from_header, Some("image/png; charset=binary")),
        Some("png")
    );
    assert_eq!(twitter_image_extension(&from_header, None), None);
    assert_eq!(
        twitter_status_id(&allowed_url("https://x.com/user/status/123").unwrap()),
        Some("123")
    );
    assert_eq!(
        twitter_status_id(&Url::parse("https://x.com/user/status/abc").unwrap()),
        None
    );
}

#[cfg(unix)]
#[tokio::test]
async fn twitter_photo_only_helper_metadata_preserves_photo_without_claiming_video() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let mut settings = fake_settings(root.path(), "#!/bin/sh\nexit 1\n").await;
    let fake_python = root.path().join("fake-python");
    fs::write(&fake_python, "#!/bin/sh\nprintf '%s\\n' '{\"photos\":[\"https://pbs.twimg.com/media/one?format=jpg&name=orig\"],\"description\":null,\"has_video\":false}'\n").unwrap();
    fs::set_permissions(&fake_python, fs::Permissions::from_mode(0o700)).unwrap();
    settings.python_bin = fake_python;
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let url = allowed_url("https://x.com/user/status/123").unwrap();

    let photos = extract_twitter_images(&url, &settings, work.path(), Duration::from_secs(3))
        .await
        .unwrap();

    assert_eq!(
        photos.images,
        vec![Url::parse("https://pbs.twimg.com/media/one?format=jpg&name=orig").unwrap(),]
    );
    assert!(!photos.has_video);
}

#[cfg(unix)]
#[tokio::test]
async fn twitter_helper_metadata_rejects_missing_or_nonboolean_video_classification() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let mut settings = fake_settings(root.path(), "#!/bin/sh\nexit 1\n").await;
    let fake_python = root.path().join("fake-python");
    settings.python_bin = fake_python.clone();
    let url = allowed_url("https://x.com/user/status/123").unwrap();

    for has_video in [
        None,
        Some(serde_json::json!(null)),
        Some(serde_json::json!("false")),
        Some(serde_json::json!("true")),
        Some(serde_json::json!(0)),
        Some(serde_json::json!(1)),
        Some(serde_json::json!([])),
        Some(serde_json::json!({})),
    ] {
        let mut payload = serde_json::json!({
            "photos": ["https://pbs.twimg.com/media/one?format=jpg&name=orig"],
            "description": null,
        });
        if let Some(value) = has_video {
            payload["has_video"] = value;
        }
        fs::write(
            &fake_python,
            format!("#!/bin/sh\nprintf '%s\\n' '{payload}'\n"),
        )
        .unwrap();
        fs::set_permissions(&fake_python, fs::Permissions::from_mode(0o700)).unwrap();
        let work = tempfile::tempdir_in(root.path()).unwrap();

        let result =
            extract_twitter_images(&url, &settings, work.path(), Duration::from_secs(3)).await;

        assert!(
            result.is_err(),
            "must reject missing or nonboolean has_video: {payload}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn twitter_mixed_photos_and_videos_survive_partial_helper_failure() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let mut settings = fake_settings(
        root.path(),
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf video > "$dir/finished.mp4"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
"#,
    )
    .await;
    let fake_python = root.path().join("fake-python");
    fs::write(&fake_python, "#!/bin/sh\n[ \"$2\" = 123 ] || exit 1\nprintf '%s\\n' '{\"photos\":[\"https://pbs.twimg.com/media/one?format=jpg&name=orig\",\"https://evil.test/media/p.jpg\"],\"description\":null,\"has_video\":true}'\n").unwrap();
    fs::set_permissions(&fake_python, fs::Permissions::from_mode(0o700)).unwrap();
    settings.python_bin = fake_python.clone();
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let url = allowed_url("https://x.com/user/status/123/photo/2").unwrap();
    let photos = extract_twitter_images(&url, &settings, work.path(), Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(photos.images.len(), 1);
    assert_eq!(photos.skipped, 1);
    assert!(photos.has_video);
    let videos = download_videos(&url, &settings, work.path(), Duration::from_secs(3))
        .await
        .unwrap();
    assert!(videos.process_ok);
    assert_eq!(fs::read(&videos.selection.files[0].path).unwrap(), b"video");
    let photo = photos.images.first().unwrap();
    let ext = twitter_image_extension(photo, Some("image/jpeg")).unwrap();
    let response = mock_photo_response("200 OK", b"photo", true, "image/jpeg").await;
    save_image_response(
        response,
        work.path(),
        &format!("photo-00.{ext}"),
        100,
        100,
        None,
    )
    .await
    .unwrap();
    let manifest = work.path().join("completed-paths.txt");
    writeln!(
        fs::OpenOptions::new().append(true).open(&manifest).unwrap(),
        "{}",
        work.path().join("photo-00.jpg").display()
    )
    .unwrap();
    let selected = select_media(work.path(), &manifest, 100).unwrap();
    assert_eq!(
        selected
            .files
            .iter()
            .map(|file| file.kind)
            .collect::<Vec<_>>(),
        vec![MediaKind::Video, MediaKind::Photo]
    );
    assert_eq!(fs::read(&selected.files[1].path).unwrap(), b"photo");

    fs::write(&fake_python, "#!/bin/sh\nexit 1\n").unwrap();
    let partial = tempfile::tempdir_in(root.path()).unwrap();
    let result = download(&url, &settings, partial.path()).await.unwrap();
    assert!(!result.process_ok);
    assert!(result.twitter_photos_failed);
    assert_eq!(result.selection.files.len(), 1);
    assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"video");

    fs::write(&fake_python, "#!/bin/sh\nsleep 10\n").unwrap();
    let slow = tempfile::tempdir_in(root.path()).unwrap();
    let result = download_with_timeout(&url, &settings, slow.path(), Duration::from_millis(300))
        .await
        .unwrap();
    assert!(!result.process_ok);
    assert!(result.twitter_photos_failed);
    assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"video");
}

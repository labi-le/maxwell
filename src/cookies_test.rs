use crate::prelude::*;
use crate::test_support::*;
use crate::*;

#[test]
fn tiktok_cookies_ignore_unrelated_unicode_values() {
    let dir = tempfile::tempdir().unwrap();
    let jar = dir.path().join("cookies.txt");
    fs::write(
        &jar,
        concat!(
            "# Netscape HTTP Cookie File\n",
            ".youtube.com\tTRUE\t/\tTRUE\t4102444800\tother\tcafé;youtube_secret\n",
            ".tiktok.com\tTRUE\t/\tTRUE\t4102444800\tsessionid\tsynthetic-tiktok\n",
        ),
    )
    .unwrap();
    let page = Url::parse("https://www.tiktok.com/@user/video/111").unwrap();

    let cookies = tiktok_page_cookies(Some(&jar), &page);
    assert_eq!(
        cookies.as_ref().map(Option::as_deref),
        Ok(Some("sessionid=synthetic-tiktok"))
    );
    let header = cookies.unwrap().unwrap();
    assert!(!header.contains("café"));
    assert!(!header.contains("youtube_secret"));
}

#[test]
fn tiktok_cookies_skip_json_valued_auxiliary_cookies() {
    let dir = tempfile::tempdir().unwrap();
    let jar = dir.path().join("cookies.txt");
    fs::write(&jar, concat!(
            "# Netscape HTTP Cookie File\n",
            ".tiktok.com\tTRUE\t/\tTRUE\t4102444800\tsessionid\tsynthetic-tiktok\n",
            "www.tiktok.com\tFALSE\t/\tFALSE\t4102444800\tg_state\t{\"i_l\":0,\"i_ll\":1788446342614}\n",
        )).unwrap();
    let page = Url::parse("https://www.tiktok.com/@user/video/111").unwrap();

    assert_eq!(
        tiktok_page_cookies(Some(&jar), &page)
            .as_ref()
            .map(Option::as_deref),
        Ok(Some("sessionid=synthetic-tiktok")),
    );
}

#[test]
fn tiktok_cookies_require_usable_session_auth() {
    let dir = tempfile::tempdir().unwrap();
    let page = Url::parse("https://www.tiktok.com/@user/video/111").unwrap();
    let cases = [
        ("0", "sessionid", true),
        ("4102444800", "sessionid", true),
        ("1", "sessionid", false),
        ("4102444800", "ttwid", false),
    ];
    for (expiry, name, accepted) in cases {
        let jar = dir.path().join(format!("{name}-{expiry}.txt"));
        fs::write(&jar, format!(
                "# Netscape HTTP Cookie File\n.tiktok.com\tTRUE\t/\tTRUE\t{expiry}\t{name}\tsynthetic\n"
            )).unwrap();
        let result = tiktok_page_cookies(Some(&jar), &page);
        assert_eq!(result.is_ok(), accepted, "{name} expiry {expiry}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn writable_cookie_copies_survive_process_saves_without_touching_source() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let original = b"# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t1900000000\tfixture\tsynthetic\n";
    let source = root.path().join("mounted-cookies.txt");
    fs::write(&source, original).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o400)).unwrap();
    let script = format!(
        r#"#!/bin/sh
# CUSTOM_METADATA
while [ "$#" -gt 0 ]; do
  case "$1" in
    --cookies) shift; cookie=$1 ;;
    --output) shift; dir=${{1%/*}} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
    --dump-single-json) metadata=1 ;;
  esac
  shift
done
[ -f "$cookie" ] || exit 2
if [ "$metadata" = 1 ]; then
  printf '%s\n' "$cookie" > '{}/metadata-path'
  printf '%s\n' '# saved by yt-dlp' >> "$cookie" || exit 3
  printf '%s\n' '{{"formats":[]}}'
else
  printf '%s\n' "$cookie" > '{}/video-path'
  printf video > "$dir/finished.mp4"
  printf '%s\n' "$dir/finished.mp4" > "$manifest"
  printf '%s\n' '# saved by yt-dlp' >> "$cookie" || exit 3
fi
"#,
        root.path().display(),
        root.path().display()
    );
    let mut cfg = fake_settings(root.path(), &script).await;
    cfg.cookies_file = Some(source.clone());
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let url = allowed_url("https://pinterest.com/pin/123").unwrap();
    let metadata = extract_images(&url, &cfg, "pinterest", work.path(), Duration::from_secs(3))
        .await
        .unwrap();
    assert!(!metadata.has_video);
    let metadata_path = PathBuf::from(
        fs::read_to_string(root.path().join("metadata-path"))
            .unwrap()
            .trim(),
    );
    assert_ne!(metadata_path, source);
    assert!(!metadata_path.exists());
    assert_eq!(fs::read(&source).unwrap(), original);

    let helper = root.path().join("fake-python");
    fs::write(
        &helper,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$3" > '{}/helper-path'
printf '%s\n' '# saved by helper' >> "$3" || exit 3
printf '%s\n' '{{"photos":[],"description":null,"has_video":true}}'
"#,
            root.path().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    cfg.python_bin = helper;
    let url = allowed_url("https://x.com/user/status/123").unwrap();
    let result = download(&url, &cfg, work.path()).await.unwrap();
    assert!(result.process_ok);
    assert_eq!(result.selection.files.len(), 1);
    let video_path = PathBuf::from(
        fs::read_to_string(root.path().join("video-path"))
            .unwrap()
            .trim(),
    );
    let helper_path = PathBuf::from(
        fs::read_to_string(root.path().join("helper-path"))
            .unwrap()
            .trim(),
    );
    assert_ne!(video_path, helper_path);
    assert_ne!(video_path, metadata_path);
    assert_ne!(video_path, source);
    assert_ne!(helper_path, source);
    assert!(!video_path.exists());
    assert!(!helper_path.exists());
    assert_eq!(fs::read(&source).unwrap(), original);

    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&source, b"synthetic corrupt jar\n").unwrap();
    let error = download(&url, &cfg, work.path()).await.err().unwrap();
    assert_eq!(error, "Не удалось подготовить cookies");
    assert!(!error.contains("synthetic"));
}

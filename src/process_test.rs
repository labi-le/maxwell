use crate::test_support::*;
use crate::*;

#[cfg(unix)]
#[tokio::test]
async fn downloads_only_finished_files_on_partial_failure() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(
        root.path(),
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf one > "$dir/1.jpg"
printf two > "$dir/2.mp4"
printf fragment > "$dir/3.temp.mp4"
printf stream > "$dir/4.mp4"
printf fragment > "$dir/5.mp3.part"
printf '%s\n' "$dir/1.jpg" "$dir/2.mp4" > "$manifest"
exit 9
"#,
    )
    .await;
    let work = tempfile::tempdir_in(root.path()).unwrap();
    for link in [
        "https://www.instagram.com/p/example/",
        "https://x.com/user/status/123",
    ] {
        let result = download(&allowed_url(link).unwrap(), &settings, work.path())
            .await
            .unwrap();
        assert!(!result.process_ok);
        assert_eq!(result.selection.files.len(), 2);
        assert_eq!(result.selection.files[0].kind, MediaKind::Photo);
        assert_eq!(result.selection.files[1].kind, MediaKind::Video);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn max_downloads_exit_code_with_ten_finished_files_is_not_failure() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(
        root.path(),
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
i=1
while [ "$i" -le 10 ]; do
  printf photo > "$dir/$i.jpg"
  printf '%s\n' "$dir/$i.jpg" >> "$manifest"
  i=$((i + 1))
done
exit 101
"#,
    )
    .await;
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = download(
        &allowed_url("https://instagram.com/p/carousel/").unwrap(),
        &settings,
        work.path(),
    )
    .await
    .unwrap();
    assert!(result.process_ok);
    assert!(!result.limited);
    assert_eq!(result.selection.files.len(), MAX_FILES);
    assert_eq!(result.selection.skipped, 0);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn completed_download_does_not_kill_remaining_group_members() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(
        root.path(),
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
  esac
  shift
done
(sleep 0.2; printf survived > "$dir/descendant-finished") &
exit 0
"#,
    )
    .await;
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = download(
        &allowed_url("https://instagram.com/p/carousel/").unwrap(),
        &settings,
        work.path(),
    )
    .await
    .unwrap();
    assert!(result.process_ok);
    assert!(!result.limited);
    time::timeout(Duration::from_secs(3), async {
            loop {
                if matches!(fs::read(work.path().join("descendant-finished")), Ok(bytes) if bytes == b"survived") {
                    break;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("a normally completed process must not kill its descendant");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn timeout_and_cancellation_stop_download_descendants() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(
        root.path(),
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf photo > "$dir/done.jpg"
printf '%s\n' "$dir/done.jpg" > "$manifest"
printf fragment > "$dir/incomplete.temp.mp4"
sleep 30 &
printf '%s\n' "$!" > "$dir/descendant.pid"
wait
"#,
    )
    .await;
    let url = allowed_url("https://instagram.com/p/carousel/").unwrap();
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = download_with_timeout(&url, &settings, work.path(), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(!result.process_ok);
    assert!(!result.limited);
    assert_eq!(
        result.diagnostic.map(|diagnostic| diagnostic.detail),
        Some("Timeout"),
        "real subprocess expiry must preserve its actionable timeout cause"
    );
    assert_eq!(result.selection.files.len(), 1);
    let pid: i32 = fs::read_to_string(work.path().join("descendant.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_descendant_stopped(pid).await;

    let cancelled_work = tempfile::tempdir_in(root.path()).unwrap();
    let path = cancelled_work.path().to_path_buf();
    let task = tokio::spawn(async move {
        download_with_timeout(&url, &settings, &path, Duration::from_secs(30)).await
    });
    time::timeout(Duration::from_secs(3), async {
        while !cancelled_work.path().join("descendant.pid").exists() {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = fs::read_to_string(cancelled_work.path().join("descendant.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert_descendant_stopped(pid).await;
}

#[cfg(unix)]
#[tokio::test]
async fn disk_limit_stops_download_without_exposing_incomplete_file() {
    let root = tempfile::tempdir().unwrap();
    let settings = fake_settings(
        root.path(),
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf photo > "$dir/done.jpg"
printf '%s\n' "$dir/done.jpg" > "$manifest"
printf '%1100s' fragment > "$dir/incomplete.temp.mp4"
sleep 30
"#,
    )
    .await;
    let work = tempfile::tempdir_in(root.path()).unwrap();
    let result = download(
        &allowed_url("https://instagram.com/p/carousel/").unwrap(),
        &settings,
        work.path(),
    )
    .await
    .unwrap();
    assert!(result.limited);
    assert!(!result.process_ok);
    assert_eq!(result.selection.files.len(), 1);
    assert_eq!(result.selection.files[0].kind, MediaKind::Photo);
}

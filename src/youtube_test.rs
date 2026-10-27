use crate::prelude::*;
use crate::*;
use crate::test_support::*;


    #[test]
    fn youtube_format_selection_uses_exact_combined_sizes_and_compatible_codecs() {
        let mut info = serde_json::json!({
            "live_status": "not_live", "duration": 130, "formats": [
                {"format_id":"399","ext":"mp4","vcodec":"av01.0","acodec":"none","width":1920,"height":1080,"fps":30,"filesize":15787421},
                {"format_id":"251","ext":"webm","vcodec":"none","acodec":"opus","filesize":2136775},
                {"format_id":"137","ext":"mp4","vcodec":"avc1.640028","acodec":"none","width":1920,"height":1080,"fps":30,"filesize":48819050},
                {"format_id":"136","ext":"mp4","vcodec":"avc1.4d401f","acodec":"none","width":1280,"height":720,"fps":30,"filesize":26711969},
                {"format_id":"140","ext":"m4a","vcodec":"none","acodec":"mp4a.40.2","abr":128,"filesize":2108456}
            ]
        });
        assert_eq!(choose_youtube_mp4(&info, DEFAULT_MEDIA_LIMIT).as_deref(), Some("136+140"));
        assert_eq!(choose_youtube_mp4(&info, 52_000_000).as_deref(), Some("137+140"));
        assert_eq!(choose_youtube_mp4(&info, 28_820_425), None); // mux headroom, not just stream sum
        info["formats"][3]["filesize"] = Value::Null;
        assert_eq!(choose_youtube_mp4(&info, DEFAULT_MEDIA_LIMIT), None);
        info["formats"][3]["filesize"] = serde_json::json!(u64::MAX);
        info["formats"][2]["filesize"] = serde_json::json!(u64::MAX);
        assert_eq!(choose_youtube_mp4(&info, u64::MAX), None); // checked sum
        info["formats"][2]["filesize"] = serde_json::json!(48_819_050);
        info["formats"][3]["format_id"] = serde_json::json!("136/b");
        assert_eq!(choose_youtube_mp4(&info, DEFAULT_MEDIA_LIMIT), None);
        info["formats"][3]["format_id"] = serde_json::json!("136");
        info["formats"][3]["filesize"] = serde_json::json!(26_711_969);
        info["live_status"] = serde_json::json!("is_live");
        assert_eq!(choose_youtube_mp4(&info, DEFAULT_MEDIA_LIMIT), None);
        info["live_status"] = Value::Null;
        assert_eq!(choose_youtube_mp4(&info, DEFAULT_MEDIA_LIMIT), None);
        info["live_status"] = serde_json::json!("was_live");
        info["entries"] = serde_json::json!([]);
        assert_eq!(choose_youtube_mp4(&info, DEFAULT_MEDIA_LIMIT), None);
        info.as_object_mut().unwrap().remove("entries");
        info["formats"] = serde_json::json!([
            {"format_id":"18","ext":"mp4","vcodec":"avc1.42001E","acodec":"mp4a.40.2",
             "width":640,"height":360,"fps":30,"filesize":1_000_000}
        ]);
        assert_eq!(choose_youtube_mp4(&info, DEFAULT_MEDIA_LIMIT).as_deref(), Some("18"));
    }

    #[test]
    fn youtube_probe_is_only_for_single_video_urls_without_playlist_query() {
        for url in ["https://youtu.be/abc", "https://youtube.com/watch?v=abc",
            "https://youtube.com/shorts/abc", "https://youtube.com/live/abc"] {
            assert!(youtube_single_video(&allowed_url(url).unwrap()), "{url}");
        }
        for url in ["https://youtube.com/playlist?list=PL123",
            "https://youtu.be/abc?list=PL123", "https://youtube.com/watch?v=abc&list=PL123",
            "https://youtube.com/clip/abc", "https://instagram.com/p/abc"] {
            assert!(!youtube_single_video(&allowed_url(url).unwrap()), "{url}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn youtube_compatible_mp4_probe_drives_download_and_video_delivery() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("mounted-cookies.txt");
        let original = b"# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t1900000000\tfixture\tsynthetic\n";
        fs::write(&source, original).unwrap();
        let script = format!(r#"#!/bin/sh
# CUSTOM_METADATA
case "$*" in
  *--dump-single-json*)
    printf '%s\n' "$@" > '{0}/probe-args'
    printf '%s\n' probe >> '{0}/probes'
    case "$*" in
      *unknown*) printf '%s\n' '{{"live_status":"not_live","duration":130,"formats":[]}}' ;;
      *failed*) exit 7 ;;
      *) printf '%s\n' '{{"live_status":"not_live","duration":130,"formats":[
        {{"format_id":"137","ext":"mp4","vcodec":"avc1.640028","acodec":"none","width":1920,"height":1080,"fps":30,"filesize":48819050}},
        {{"format_id":"136","ext":"mp4","vcodec":"avc1.4d401f","acodec":"none","width":1280,"height":720,"fps":30,"filesize":26711969}},
        {{"format_id":"140","ext":"m4a","vcodec":"none","acodec":"mp4a.40.2","filesize":2108456}}]}}' ;;
    esac
    exit 0 ;;
esac
printf '%s\n' download >> '{0}/downloads'
printf '%s\n' "$@" > '{0}/download-args'
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${{1%/*}} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
  esac
  shift
done
printf mp4 > "$dir/result.mp4"
printf '%s\n' "$dir/result.mp4" > "$manifest"
"#, root.path().display());
        let mut cfg = fake_settings(root.path(), &script).await;
        cfg.cookies_file = Some(source.clone());
        cfg.max_media_bytes = DEFAULT_MEDIA_LIMIT;
        let url = allowed_url("https://youtu.be/abc").unwrap();
        let work = tempfile::tempdir_in(root.path()).unwrap();
        let selected = download_videos(&url, &cfg, work.path(), Duration::from_secs(3)).await.unwrap();
        assert!(selected.process_ok);
        assert_eq!(selected.selection.files.len(), 1);
        assert_eq!(selected.selection.files[0].kind, MediaKind::Video);
        assert_eq!(fs::read_to_string(root.path().join("downloads")).unwrap().lines().count(), 1);
        let probe_args = fs::read_to_string(root.path().join("probe-args")).unwrap();
        assert!(probe_args.contains("--dump-single-json\n"));
        assert!(probe_args.contains("--extractor-args\nyoutube:player_client=default,web_embedded\n"));
        assert!(probe_args.contains("--cookies\n"));
        assert!(!probe_args.contains("-f\n"));
        let download_args = fs::read_to_string(root.path().join("download-args")).unwrap();
        assert!(download_args.contains("-f\n136+140\n--merge-output-format\nmp4\n"));
        assert!(download_args.contains("--extractor-args\nyoutube:player_client=default,web_embedded\n"));
        let cookie_arg = |args: &str| args.lines().skip_while(|line| *line != "--cookies")
            .nth(1).unwrap().to_string();
        assert_ne!(cookie_arg(&probe_args), cookie_arg(&download_args));
        assert_ne!(cookie_arg(&download_args), source.to_string_lossy().as_ref());
        fs::copy(av_fixture(), &selected.selection.files[0].path).unwrap();
        let (bot, calls, server) = recording_bot(true, true).await;
        send_media(&bot, ChatId(1), &selected.selection.files[0], "caption",
            cfg.max_media_bytes, time::Instant::now() + Duration::from_secs(3)).await.unwrap();
        let calls = calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert!(api_method_is(&calls[0].0, "sendVideo"));
        drop(calls);
        server.abort();
        assert_eq!(fs::read(&source).unwrap(), original);

        let fallback = tempfile::tempdir_in(root.path()).unwrap();
        download_videos(&allowed_url("https://youtu.be/unknown").unwrap(), &cfg,
            fallback.path(), Duration::from_secs(3)).await.unwrap();
        assert_eq!(fs::read_to_string(root.path().join("downloads")).unwrap().lines().count(), 2);
        let metadata_error = tempfile::tempdir_in(root.path()).unwrap();
        let original_download = download_videos(&allowed_url("https://youtu.be/failed").unwrap(), &cfg,
            metadata_error.path(), Duration::from_secs(3)).await.unwrap();
        assert!(original_download.process_ok);
        let playlist = tempfile::tempdir_in(root.path()).unwrap();
        download_videos(&allowed_url("https://youtube.com/playlist?list=PL123").unwrap(), &cfg,
            playlist.path(), Duration::from_secs(3)).await.unwrap();
        assert_eq!(fs::read_to_string(root.path().join("probes")).unwrap().lines().count(), 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn youtube_selected_format_failure_retries_once_without_losing_deadline_or_completed_media() {
        let root = tempfile::tempdir().unwrap();
        let script = r#"#!/bin/sh
# CUSTOM_METADATA
case "$*" in
  *--dump-single-json*)
    printf '%s\n' probe >> 'ROOT_PATH/probes'
    case "$*" in
      *unselected*) printf '%s\n' '{"formats":[]}' ;;
      *) printf '%s\n' '{"live_status":"not_live","duration":20,"formats":[{"format_id":"18","ext":"mp4","vcodec":"avc1.42001e","acodec":"mp4a.40.2","width":640,"height":360,"filesize":20}]}' ;;
    esac
    exit 0 ;;
esac
selected=no
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; dir=${1%/*} ;;
    --print-to-file) shift; shift; manifest=$1 ;;
    -f) shift; if [ "$1" = 18 ]; then selected=yes; fi ;;
    --) shift; url=${1##*/}; break ;;
  esac
  shift
done
printf '%s\n' "$selected" >> "ROOT_PATH/$url-attempts"
if [ "$selected" = yes ]; then
  case "$url" in
    missing) printf part > "$dir/part.mp4.part"; : > "$manifest"; exit 9 ;;
    finished) printf done > "$dir/finished.mp4"; printf '%s\n' "$dir/finished.mp4" > "$manifest"; exit 9 ;;
    huge) printf '%0700001d' 0 > "$dir/huge.mp4.part"; exit 9 ;;
    slow) sleep 2; exit 9 ;;
    success) printf done > "$dir/finished.mp4"; printf '%s\n' "$dir/finished.mp4" > "$manifest"; exit 0 ;;
  esac
fi
[ "$url" != unselected ] || exit 9
[ ! -e "$dir/part.mp4.part" ] || exit 8
[ ! -e "$manifest" ] || exit 8
printf default > "$dir/finished.mp4"
printf '%s\n' "$dir/finished.mp4" > "$manifest"
"#.replace("ROOT_PATH", root.path().to_str().unwrap());
        let mut cfg = fake_settings(root.path(), &script).await;
        cfg.max_media_bytes = 70_000;
        let missing = tempfile::tempdir_in(root.path()).unwrap();
        let result = download_videos(&allowed_url("https://youtu.be/missing").unwrap(), &cfg,
            missing.path(), Duration::from_secs(3)).await.unwrap();
        assert!(result.process_ok);
        assert_eq!(result.selection.files.len(), 1);
        assert_eq!(fs::read(&result.selection.files[0].path).unwrap(), b"default");
        assert!(!missing.path().join("part.mp4.part").exists());
        assert_eq!(fs::read_to_string(root.path().join("missing-attempts")).unwrap(), "yes\nno\n");
        fs::copy(av_fixture(), &result.selection.files[0].path).unwrap();
        let (bot, calls, server) = recording_bot(true, true).await;
        send_media(&bot, ChatId(1), &result.selection.files[0], "caption",
            cfg.max_media_bytes, time::Instant::now() + Duration::from_secs(3)).await.unwrap();
        let calls = calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert!(api_method_is(&calls[0].0, "sendVideo"));
        drop(calls);
        server.abort();

        for (name, timeout) in [
            ("success", Duration::from_secs(3)),
            ("finished", Duration::from_secs(3)),
            ("huge", Duration::from_secs(3)),
            ("slow", Duration::from_millis(300)),
        ] {
            let work = tempfile::tempdir_in(root.path()).unwrap();
            let result = download_videos(&allowed_url(&format!("https://youtu.be/{name}")).unwrap(),
                &cfg, work.path(), timeout).await.unwrap();
            assert_eq!(result.process_ok, name == "success", "{name}");
            assert_eq!(result.selection.files.len(), usize::from(name == "success" || name == "finished"), "{name}");
            assert_eq!(fs::read_to_string(root.path().join(format!("{name}-attempts"))).unwrap(), "yes\n", "{name}");
        }
        let work = tempfile::tempdir_in(root.path()).unwrap();
        let result = download_videos(&allowed_url("https://youtu.be/unselected").unwrap(),
            &cfg, work.path(), Duration::from_secs(3)).await.unwrap();
        assert!(!result.process_ok);
        assert!(result.selection.files.is_empty());
        assert_eq!(fs::read_to_string(root.path().join("unselected-attempts")).unwrap(), "no\n");
        assert_eq!(fs::read_to_string(root.path().join("probes")).unwrap().lines().count(), 6);
    }

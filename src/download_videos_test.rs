use crate::prelude::*;
use crate::*;
use crate::test_support::*;

#[cfg(unix)]
    #[tokio::test]
    async fn tiktok_requests_carry_firefox_user_agent_only_for_tiktok_hosts() {
        let root = tempfile::tempdir().unwrap();
        let script = format!(r#"#!/bin/sh
printf '%s\n' download >> '{0}/calls'
printf '%s\n' "$@" > '{0}/args'
exit 1
"#, root.path().display());
        let cfg = fake_settings(root.path(), &script).await;
        let tiktok = allowed_url("https://www.tiktok.com/@name/video/111").unwrap();
        let work = tempfile::tempdir_in(root.path()).unwrap();
        let result = download_with_timeout(&tiktok, &cfg, work.path(), Duration::from_secs(3)).await.unwrap();
        assert!(!result.process_ok);
        let args = fs::read_to_string(root.path().join("args")).unwrap();
        assert!(args.contains(&format!("--add-header\nUser-Agent: {TIKTOK_USER_AGENT}\n")));
        assert!(args.contains("-S\nres,vcodec:h264\n"));
        assert!(!args.lines().any(|arg| arg == "--cookies"));
        assert_eq!(fs::read_to_string(root.path().join("calls")).unwrap().lines().count(), 1);

        let youtube = allowed_url("https://youtu.be/abc").unwrap();
        let work = tempfile::tempdir_in(root.path()).unwrap();
        let result = download_with_timeout(&youtube, &cfg, work.path(), Duration::from_secs(3)).await.unwrap();
        assert!(!result.process_ok);
        let args = fs::read_to_string(root.path().join("args")).unwrap();
        assert!(!args.lines().any(|arg| arg == "--add-header" || arg.starts_with("User-Agent:")));
        assert!(!args.lines().any(|arg| arg == "-S"));
        assert_eq!(fs::read_to_string(root.path().join("calls")).unwrap().lines().count(), 2);
    }

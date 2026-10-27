use crate::prelude::*;
use crate::*;

pub(crate) struct DownloadProcess {
    pub(crate) child: Option<tokio::process::Child>,
    #[cfg(unix)]
    pub(crate) group: i32,
    pub(crate) reaped: bool,
    pub(crate) stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
    pub(crate) stdout: Option<tokio::task::JoinHandle<Vec<(PathBuf, String)>>>,
}

pub(crate) fn stderr_detail(text: &str) -> &'static str {
    for (needle, label) in [
        ("Sign in to confirm you’re not a bot", "Authentication required"),
        ("Sign in to confirm you're not a bot", "Authentication required"),
        ("Sign in to confirm your age", "Age verification required"),
        ("age-restricted", "Age verification required"),
        ("Login required", "Authentication required"),
        ("cookies are no longer valid", "Cookies expired"),
        ("cookies have expired", "Cookies expired"),
        ("cookies are expired", "Cookies expired"),
        ("Cookies expired", "Cookies expired"),
        ("Fresh cookies", "Cookies expired"),
        ("HTTP Error 401", "Authentication required"),
        ("HTTP Error 403", "HTTP 403"), ("HTTP Error 404", "HTTP 404"),
        ("HTTP Error 429", "HTTP 429"), ("HTTP Error 500", "HTTP 500"),
        ("HTTP Error 502", "HTTP 502"), ("HTTP Error 503", "HTTP 503"),
        ("HTTP Error 504", "HTTP 504"),
        ("larger than max-filesize", "Size limit"),
        ("exceeds max-filesize", "Size limit"),
        ("Read timed out", "Timeout"), ("read timed out", "Timeout"),
        ("timed out", "Timeout"),
        ("This video is private", "Private video"),
        ("Your IP address is blocked from accessing this post", "Site access restricted"),
        ("This content isn't available to everyone", "Audience restricted"),
        ("This video is available to this channel's members", "Members only"),
        ("Join this channel", "Members only"),
        ("not made this video available in your country", "Region blocked"),
        ("available in your country", "Region blocked"),
        ("This video is not available", "Video unavailable"),
        ("Video unavailable", "Video unavailable"),
        ("Requested format is not available", "Format unavailable"),
        ("Requested format not available", "Format unavailable"),
        ("Connection reset by peer", "Network error"),
        ("Connection aborted", "Network error"),
        ("EOF occurred", "Network error"),
        ("Temporary failure in name resolution", "Network error"),
        ("Unable to download video data", "Download interrupted"),
        ("The download is incomplete", "Download interrupted"),
        ("Postprocessing:", "Processing failed"),
        ("Conversion failed", "Processing failed"),
        ("ffmpeg not found", "Processing failed"),
        ("No space left on device", "Temporary storage limit"),
        ("Unsupported URL", "Unsupported URL"),
        ("Unable to extract", "Extractor failed"),
        ("Unable to download webpage", "Webpage unavailable"),
        ("Unable to download API page", "API unavailable"),
    ] {
        if text.contains(needle) { return label; }
    }
    if text.contains("HTTP Error 5") { return "HTTP 5xx"; }
    if text.contains("HTTP Error ") { return "HTTP error"; }
    "Download failed"
}

impl DownloadProcess {
    pub(crate) fn new(mut child: tokio::process::Child) -> Self {
        let stderr = child.stderr.take().map(|mut pipe| tokio::spawn(async move {
            let mut captured = Vec::new();
            let mut buffer = [0u8; 1024];
            while let Ok(size) = pipe.read(&mut buffer).await {
                if size == 0 { break; }
                let keep = size.min(4096usize.saturating_sub(captured.len()));
                captured.extend_from_slice(&buffer[..keep]);
            }
            captured
        }));
        Self {
            #[cfg(unix)]
            group: child.id().expect("spawned child has a pid") as i32,
            child: Some(child),
            reaped: false,
            stderr,
            stdout: None,
        }
    }

    pub(crate) fn capture_stdout(&mut self) {
        self.stdout = self.child.as_mut().and_then(|child| child.stdout.take()).map(|mut pipe| tokio::spawn(async move {
            const MAX_RECORD_BYTES: usize = 64 * 1024;
            let mut records = Vec::new();
            let mut line = Vec::new();
            let mut overlong = false;
            let mut buffer = [0u8; 1024];
            let finish_line = |line: &mut Vec<u8>, records: &mut Vec<(PathBuf, String)>| {
                if let Ok(text) = std::str::from_utf8(line) {
                    if let Some((path, rest)) = text.split_once('\t') {
                        if let Some((title, description)) = rest.split_once('\t') {
                            if let (Ok(path), Ok(title), Ok(description)) = (
                                serde_json::from_str::<String>(path),
                                serde_json::from_str::<String>(title),
                                serde_json::from_str::<String>(description),
                            ) {
                                if let Some(body) = caption_text(Some(&title), Some(&description)) {
                                    records.push((PathBuf::from(path), body));
                                }
                            }
                        }
                    }
                }
                line.clear();
            };
            while let Ok(size) = pipe.read(&mut buffer).await {
                if size == 0 { break; }
                for segment in buffer[..size].split_inclusive(|byte| *byte == b'\n') {
                    let complete = segment.last() == Some(&b'\n');
                    let content = if complete { &segment[..segment.len() - 1] } else { segment };
                    if records.len() < MAX_FILES && !overlong {
                        if line.len() + content.len() <= MAX_RECORD_BYTES {
                            line.extend_from_slice(content);
                        } else {
                            line.clear();
                            overlong = true;
                        }
                    }
                    if complete {
                        if records.len() < MAX_FILES && !overlong {
                            finish_line(&mut line, &mut records);
                        }
                        line.clear();
                        overlong = false;
                    }
                }
            }
            if records.len() < MAX_FILES && !overlong && !line.is_empty() {
                finish_line(&mut line, &mut records);
            }
            records
        }));
    }

    pub(crate) async fn stdout_records(&mut self, deadline: time::Instant) -> Vec<(PathBuf, String)> {
        let Some(mut task) = self.stdout.take() else { return Vec::new() };
        match time::timeout_at(deadline.min(time::Instant::now() + Duration::from_secs(1)), &mut task).await {
            Ok(Ok(records)) => records,
            _ => { task.abort(); Vec::new() }
        }
    }

    pub(crate) fn stop(&mut self) {
        // A reaped leader no longer pins its PID/PGID; signalling it could hit another group.
        if self.reaped {
            return;
        }
        #[cfg(unix)]
        // yt-dlp owns a new process group; ffmpeg and other children inherit it.
        unsafe { libc::kill(-self.group, libc::SIGKILL); }
        #[cfg(not(unix))]
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }

    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.as_mut().expect("child is owned").wait().await?;
        self.reaped = true;
        Ok(status)
    }

    pub(crate) async fn stderr_label(&mut self, deadline: time::Instant) -> &'static str {
        let Some(mut task) = self.stderr.take() else { return "Download failed" };
        let bytes = match time::timeout_at(deadline.min(time::Instant::now() + Duration::from_secs(1)), &mut task).await {
            Ok(Ok(bytes)) => bytes,
            _ => {
                task.abort();
                return "Download failed";
            }
        };
        let text = std::str::from_utf8(&bytes).unwrap_or("");
        stderr_detail(text)
    }
}

impl Drop for DownloadProcess {
    fn drop(&mut self) {
        if let Some(task) = self.stderr.take() { task.abort(); }
        if let Some(task) = self.stdout.take() { task.abort(); }
        if !self.reaped {
            self.stop();
            if let Some(mut child) = self.child.take() {
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move { let _ = child.wait().await; });
                }
                // Without a runtime, kill_on_drop still kills the leader.
            }
        }
    }
}

use crate::prelude::*;
use crate::*;

pub(crate) fn cookie_source(url: &Url) -> &'static str {
    match url.host_str().unwrap_or("") {
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be" => {
            "youtube"
        }
        "tiktok.com" | "www.tiktok.com" | "m.tiktok.com" | "vm.tiktok.com" | "vt.tiktok.com" => {
            "tiktok"
        }
        "pinterest.com"
        | "www.pinterest.com"
        | "pinterest.ca"
        | "www.pinterest.ca"
        | "pinterest.co.uk"
        | "www.pinterest.co.uk"
        | "pin.it" => "pinterest",
        "instagram.com" | "www.instagram.com" => "instagram",
        "x.com" | "www.x.com" | "twitter.com" | "www.twitter.com" | "mobile.twitter.com" => {
            "twitter"
        }
        _ => "generic",
    }
}

pub(crate) fn is_cookie_auth_detail(detail: &str) -> bool {
    matches!(
        detail,
        "Authentication required" | "Cookies expired" | "Cookies unavailable"
    )
}

pub(crate) async fn request_cookie_refresh(
    cfg: &Settings,
    source: &str,
    deadline: time::Instant,
    events: &mut Vec<FailureDiagnostic>,
) -> bool {
    let Some(base) = &cfg.cookie_refresh_url else {
        return false;
    };
    trace_event(
        events,
        FailureDiagnostic::new("refresh", "Request submitted"),
    );
    let remaining = deadline.saturating_duration_since(time::Instant::now());
    if remaining < Duration::from_secs(1) {
        trace_event(events, FailureDiagnostic::new("refresh", "Timed out"));
        return false;
    }
    let timeout = remaining.min(COOKIE_REFRESH_TIMEOUT);
    let Ok(target) = base.join("refresh") else {
        trace_event(events, FailureDiagnostic::new("refresh", "Request failed"));
        return false;
    };
    let client = match reqwest::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            trace_event(events, FailureDiagnostic::new("refresh", "Request failed"));
            return false;
        }
    };
    let body = serde_json::json!({"source": source}).to_string();
    let mut response = match client
        .post(target)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            trace_event(events, FailureDiagnostic::new("refresh", "Timed out"));
            return false;
        }
        Err(_) => {
            trace_event(events, FailureDiagnostic::new("refresh", "Request failed"));
            return false;
        }
    };
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > 4096)
    {
        trace_event(events, FailureDiagnostic::new("refresh", "Request failed"));
        return false;
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > 4096 {
                    trace_event(events, FailureDiagnostic::new("refresh", "Request failed"));
                    return false;
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) if error.is_timeout() => {
                trace_event(events, FailureDiagnostic::new("refresh", "Timed out"));
                return false;
            }
            Err(_) => {
                trace_event(events, FailureDiagnostic::new("refresh", "Request failed"));
                return false;
            }
        }
    }
    let acknowledged = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.get("ok").and_then(Value::as_bool))
        == Some(true);
    if !acknowledged {
        trace_event(events, FailureDiagnostic::new("refresh", "Request failed"));
        return false;
    }
    trace_event(events, FailureDiagnostic::new("refresh", "Acknowledged"));
    true
}

pub(crate) async fn refresh_after_auth_failure(
    cfg: &Settings,
    source: &str,
    dir: &Path,
    deadline: time::Instant,
    events: &mut Vec<FailureDiagnostic>,
    failure: &DownloadFailure,
) -> bool {
    if cfg.cookies_file.is_none()
        || !is_cookie_auth_detail(failure.diagnostic.detail)
        || time::Instant::now() >= deadline
        || !retryable_output(dir, cfg.max_media_bytes)
    {
        return false;
    }
    request_cookie_refresh(cfg, source, deadline, events).await
}

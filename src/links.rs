use crate::prelude::*;
use crate::*;

pub(crate) fn allowed_url(raw: &str) -> Option<Url> {
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let path = url.path();
    // Reject encoded paths as well: a server might decode /%72edirect before redirecting.
    if path.contains('%') {
        return None;
    }
    let one_segment = |prefix: &str| {
        path.strip_prefix(prefix)
            .is_some_and(|id| !id.is_empty() && !id.trim_end_matches('/').contains('/'))
    };
    let query_has = |name: &str| url.query_pairs().any(|(key, value)| key == name && !value.is_empty());
    let post_path = match host {
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            (path == "/watch" && query_has("v"))
                || (path == "/playlist" && query_has("list"))
                || ["/shorts/", "/live/", "/embed/", "/clip/"].iter().any(|prefix| one_segment(prefix))
        }
        "youtu.be" | "pin.it" | "vm.tiktok.com" | "vt.tiktok.com" => one_segment("/"),
        "pinterest.com" | "www.pinterest.com" | "pinterest.ca" | "www.pinterest.ca"
        | "pinterest.co.uk" | "www.pinterest.co.uk" => one_segment("/pin/"),
        "x.com" | "www.x.com" | "twitter.com" | "www.twitter.com" | "mobile.twitter.com" => {
            twitter_status_id(&url).is_some()
        }
        "instagram.com" | "www.instagram.com" => {
            ["/p/", "/reel/", "/reels/", "/tv/"].iter().any(|prefix| one_segment(prefix))
        }
        "tiktok.com" | "www.tiktok.com" | "m.tiktok.com" => {
            let mut parts = path.trim_matches('/').split('/');
            matches!((parts.next(), parts.next(), parts.next(), parts.next()),
                (Some(user), Some("video" | "photo"), Some(id), None)
                    if user.starts_with('@') && !id.is_empty())
                || one_segment("/v/")
                || one_segment("/t/")
        }
        _ => false,
    };
    if !post_path {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

pub(crate) fn short_link_family(url: &Url) -> Option<&'static str> {
    match url.host_str()? {
        "pin.it" => Some("pinterest"),
        "vm.tiktok.com" | "vt.tiktok.com" => Some("tiktok"),
        _ => None,
    }
}

pub(crate) fn pinterest_region_host(host: &str) -> bool {
    // Pinterest short links redirect through regional domains such as www.pinterest.se.
    let valid_label = |label: &str| {
        !label.is_empty() && label.len() <= 63 && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    let suffix = host.strip_prefix("pinterest.").or_else(|| {
        let (prefix, suffix) = host.split_once(".pinterest.")?;
        valid_label(prefix).then_some(suffix)
    });
    let Some(suffix) = suffix else { return false };
    suffix == "com" || (suffix.len() == 2 && suffix.bytes().all(|byte| byte.is_ascii_lowercase()))
        || suffix.split_once('.').is_some_and(|(base, country)| {
            matches!(base, "com" | "co") && country.len() == 2
                && country.bytes().all(|byte| byte.is_ascii_lowercase())
        })
}

pub(crate) fn short_link_host_allowed(host: &str, family: &str) -> bool {
    match family {
        "pinterest" => host == "pin.it" || pinterest_region_host(host),
        "tiktok" => host == "tiktok.com" || host.ends_with(".tiktok.com"),
        _ => false,
    }
}

// Only hosts whose owner we trust are contacted while resolving a short link.
// Regional Pinterest domains may appear in a Location header (the pin id is read
// from there), but a request is never sent to them.
pub(crate) fn short_link_contact_allowed(host: &str, family: &str) -> bool {
    match family {
        "pinterest" => host == "pin.it" || host == "pinterest.com" || host.ends_with(".pinterest.com"),
        "tiktok" => host == "tiktok.com" || host.ends_with(".tiktok.com"),
        _ => false,
    }
}

pub(crate) fn canonical_short_target(url: &Url, family: &str) -> Option<Url> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || !short_link_host_allowed(url.host_str()?, family)
    {
        return None;
    }
    let mut segments = url.path().strip_prefix('/')?.split('/');
    match family {
        "pinterest" if segments.next()? == "pin" => {
            let id = segments.next()?;
            if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
            Url::parse(&format!("https://www.pinterest.com/pin/{id}/")).ok()
        }
        "tiktok" => {
            let user = segments.next()?.strip_prefix('@')?;
            let kind = segments.next()?;
            let id = segments.next()?;
            if user.is_empty() || user.contains('%') || !matches!(kind, "video" | "photo")
                || id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            Url::parse(&format!("https://www.tiktok.com/@{user}/{kind}/{id}")).ok()
        }
        _ => None,
    }
}

pub(crate) async fn resolve_short_link(url: &Url, deadline: time::Instant) -> Option<Url> {
    let family = short_link_family(url)?;
    // yt-dlp has no extractor for short-link hosts, and generic is disabled by policy.
    time::timeout_at(deadline, async {
        let mut current = url.clone();
        for redirects in 0..=5 {
            if current.scheme() != "https" || !current.username().is_empty()
                || current.password().is_some() || current.port().is_some()
            {
                return None;
            }
            if !short_link_host_allowed(current.host_str()?, family) { return None; }
            // A redirect may already point to a post; don't follow its tracking links.
            if let Some(target) = canonical_short_target(&current, family) {
                return Some(target);
            }
            if !short_link_contact_allowed(current.host_str()?, family) { return None; }
            let client = pinned_client(&current).await.ok()?;
            let response = client.get(current.clone()).send().await.ok()?;
            if !response.status().is_redirection() { return None; }
            let location = response.headers().get(reqwest::header::LOCATION)?;
            if redirects == 5 { return None; }
            let next = current.join(location.to_str().ok()?).ok()?;
            if next.scheme() != "https" || !short_link_host_allowed(next.host_str()?, family) {
                return None;
            }
            current = next;
        }
        None
    }).await.ok().flatten()
}

pub(crate) fn first_link(text: &str) -> Option<Url> {
    text.split_whitespace().find_map(|word| {
        let word = word.trim_start_matches(['(', '[', '<', '"', '\'']);
        let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\'']);
        allowed_url(word)
    })
}

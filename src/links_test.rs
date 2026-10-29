use crate::prelude::*;
use crate::*;

#[test]
fn urls_only_from_supported_public_hosts() {
    for url in [
        "https://youtu.be/abc",
        "https://www.youtube.com/watch?v=abc",
        "https://m.youtube.com/shorts/abc",
        "https://www.youtube.com/playlist?list=PL123",
        "https://www.youtube.com/live/abc",
        "https://pin.it/abc",
        "https://www.pinterest.com/pin/123",
        "https://x.com/user/status/123",
        "https://twitter.com/user/status/123",
        "https://x.com/i/status/222",
        "https://instagram.com/p/abc/",
        "https://instagram.com/reel/abc/",
        "https://vm.tiktok.com/abc/",
        "https://www.tiktok.com/@name/video/111",
    ] {
        assert!(allowed_url(url).is_some(), "{url}");
    }
    for url in [
        "https://youtube.com.evil.test/watch?v=abc",
        "https://evil.test/youtube.com",
        "https://youtube.com@127.0.0.1/pin",
        "https://127.0.0.1/",
        "https://localhost/watch",
        "http://[::1]/",
        "file:///etc/passwd",
        "https://youtube.com:8080/watch",
        "https://user:secret@youtube.com/watch",
        "https://notinstagram.com/p/abc",
        "https://instagram.com./p/abc",
        "https://youtube.com/redirect?q=http://127.0.0.1/",
        "https://youtube.com/%72edirect?q=http://127.0.0.1/",
        "https://x.com/i/redirect?url=http://127.0.0.1/",
        "https://instagram.com/accounts/login/?next=http://127.0.0.1/",
    ] {
        assert!(allowed_url(url).is_none(), "{url}");
    }
    assert_eq!(
        first_link("See (https://youtu.be/abc), now")
            .unwrap()
            .host_str(),
        Some("youtu.be")
    );
}

#[test]
fn short_links_have_exact_families_and_bounded_redirect_hosts() {
    for (link, family) in [
        ("https://pin.it/abc", Some("pinterest")),
        ("https://vm.tiktok.com/abc", Some("tiktok")),
        ("https://vt.tiktok.com/abc", Some("tiktok")),
        ("https://pin.it.evil.com/abc", None),
        ("https://www.tiktok.com/@name/video/111", None),
    ] {
        assert_eq!(
            short_link_family(&Url::parse(link).unwrap()),
            family,
            "{link}"
        );
    }
    for host in [
        "pin.it",
        "pinterest.com",
        "api.pinterest.com",
        "se.pinterest.com",
        "www.pinterest.se",
        "pinterest.ca",
        "pinterest.co.uk",
        "pinterest.com.au",
        "pinterest.com.br",
    ] {
        assert!(short_link_host_allowed(host, "pinterest"), "{host}");
    }
    for host in [
        "pinterest.evil",
        "pinterest.com.evil.test",
        "a.b.pinterest.com",
        "evilpinterest.se",
        "pinterest.ru.evil.test",
        "pinterest..com",
        "-a.pinterest.com",
        "a-.pinterest.com",
    ] {
        assert!(!short_link_host_allowed(host, "pinterest"), "{host}");
    }
    assert!(short_link_host_allowed("tiktok.com", "tiktok"));
    assert!(short_link_host_allowed("www.tiktok.com", "tiktok"));
    assert!(!short_link_host_allowed("evil-tiktok.com", "tiktok"));
    assert!(!short_link_host_allowed("tiktok.com.evil.test", "tiktok"));
    for host in [
        "pin.it",
        "pinterest.com",
        "api.pinterest.com",
        "se.pinterest.com",
    ] {
        assert!(short_link_contact_allowed(host, "pinterest"), "{host}");
    }
    for host in [
        "www.pinterest.se",
        "pinterest.se",
        "pinterest.ca",
        "pinterest.co.uk",
        "pinterest.com.evil.test",
    ] {
        assert!(!short_link_contact_allowed(host, "pinterest"), "{host}");
    }
    for host in ["vm.tiktok.com", "www.tiktok.com"] {
        assert!(short_link_contact_allowed(host, "tiktok"), "{host}");
    }
    for host in ["evil-tiktok.com", "tiktok.com.evil.test"] {
        assert!(!short_link_contact_allowed(host, "tiktok"), "{host}");
    }
}

#[test]
fn redirect_targets_canonicalize_before_following_tracking_links() {
    let expected_pin = "https://www.pinterest.com/pin/836684437062624778/";
    for link in [
        "https://www.pinterest.se/pin/836684437062624778/sent/?invite_code=x&sender=y&sfo=1",
        "https://se.pinterest.com/pin/836684437062624778/sent/?invite_code=x",
    ] {
        let target = canonical_short_target(&Url::parse(link).unwrap(), "pinterest");
        assert_eq!(target.unwrap().as_str(), expected_pin, "{link}");
    }
    let short = Url::parse("https://vm.tiktok.com/x").unwrap();
    let redirected = short
        .join("https://www.tiktok.com/@name/photo/111")
        .unwrap();
    assert_eq!(
        canonical_short_target(&redirected, "tiktok")
            .unwrap()
            .as_str(),
        "https://www.tiktok.com/@name/photo/111"
    );
    for (link, family) in [
        ("https://www.tiktok.com/?_r=1", "tiktok"),
        ("https://www.tiktok.com/user/foo/", "tiktok"),
        ("https://www.tiktok.com/@/video/111", "tiktok"),
        ("https://www.tiktok.com/@name/video/not-a-number", "tiktok"),
        ("https://www.tiktok.com:8443/@name/video/111", "tiktok"),
        ("https://user@www.tiktok.com/@name/video/111", "tiktok"),
        ("http://www.tiktok.com/@name/video/111", "tiktok"),
        ("https://www.pinterest.com/pin/not-a-number/", "pinterest"),
        ("https://www.pinterest.com:8443/pin/123/", "pinterest"),
        ("https://user@www.pinterest.com/pin/123/", "pinterest"),
        ("http://www.pinterest.com/pin/123/", "pinterest"),
    ] {
        assert!(
            canonical_short_target(&Url::parse(link).unwrap(), family).is_none(),
            "{link}"
        );
    }
}

#[test]
fn pinterest_regions_and_twitter_photo_paths_are_exact() {
    for link in [
        "https://pinterest.ca/pin/123",
        "https://www.pinterest.ca/pin/123",
        "https://pinterest.co.uk/pin/456",
        "https://www.pinterest.co.uk/pin/456",
    ] {
        let url = allowed_url(link).unwrap();
        assert_eq!(image_source(&url), Some("pinterest"), "{link}");
    }
    for link in [
        "https://pinterest.ca.evil.com/pin/123",
        "https://pinterest.co.uk.evil.com/pin/123",
        "https://evilpinterest.ca/pin/123",
        "https://pinterest.ca/pin/123/extra",
    ] {
        assert!(allowed_url(link).is_none(), "{link}");
    }
    for link in [
        "https://x.com/user/status/123/photo/1",
        "https://twitter.com/user/status/123/photo/2",
        "https://mobile.twitter.com/user/status/123/photo/09",
    ] {
        let url = allowed_url(link).unwrap();
        assert_eq!(twitter_status_id(&url), Some("123"), "{link}");
    }
    for link in [
        "https://x.com/user/status/123/photo/0",
        "https://x.com/user/status/123/photo/abc",
        "https://x.com/user/status/123/photo/1/456",
        "https://x.com/user/status/123/video/1",
        "https://x.com/user/status/123/photo/1/extra",
        "https://x.com/user/status/abc/photo/1",
    ] {
        assert!(allowed_url(link).is_none(), "{link}");
        assert!(
            twitter_status_id(&Url::parse(link).unwrap()).is_none(),
            "{link}"
        );
    }
}

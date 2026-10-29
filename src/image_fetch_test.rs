use crate::feature_support::*;
use crate::prelude::*;
use crate::test_support::*;
use crate::*;

#[tokio::test]
async fn photo_stream_rejects_redirect_and_oversize_without_completed_file() {
    let dir = tempfile::tempdir().unwrap();
    let redirected = mock_photo_response("302 Found", b"abc", true, "image/jpeg").await;
    assert!(
        save_image_response(redirected, dir.path(), "redirect.jpg", 10, 10, Some("jpg"))
            .await
            .is_err()
    );
    let oversized = mock_photo_response("200 OK", b"123456789", false, "image/jpeg").await;
    let _ = save_image_response(oversized, dir.path(), "oversized.jpg", 8, 8, Some("jpg")).await;
    assert!(!dir.path().join("redirect.jpg").exists());
    assert!(!dir.path().join("oversized.jpg").exists());
}

#[tokio::test]
async fn image_cdn_html_is_never_persisted_as_photo_or_document() {
    for (source, name) in [("pinterest", "pin.jpg"), ("instagram", "post.webp")] {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("completed-paths.txt");
        let url = if source == "pinterest" {
            "https://i.pinimg.com/originals/pin.jpg"
        } else {
            "https://scontent.cdninstagram.com/post.webp"
        };
        assert!(image_cdn_url(url, source).is_some());
        let response =
            mock_photo_response("200 OK", b"<html>not an image</html>", true, "text/html").await;
        let ext = if source == "pinterest" { "jpg" } else { "webp" };
        assert!(
            save_image_response(response, dir.path(), name, 100, 100, Some(ext))
                .await
                .is_err()
        );
        assert!(!dir.path().join(name).exists());
        assert!(!manifest.exists());
        assert!(select_media(dir.path(), &manifest, 100)
            .unwrap()
            .files
            .is_empty());
    }
}

#[tokio::test]
async fn features_photo_cdn_http_403_retains_access_rejection_without_body_secrets_or_final_file() {
    feature_photo_cdn_http_failure("403 Forbidden", "403").await;
}

#[tokio::test]
async fn features_photo_cdn_http_404_retains_missing_media_without_body_secrets_or_final_file() {
    feature_photo_cdn_http_failure("404 Not Found", "404").await;
}

#[tokio::test]
async fn features_photo_cdn_http_429_retains_rate_limit_without_body_secrets_or_final_file() {
    feature_photo_cdn_http_failure("429 Too Many Requests", "429").await;
}

#[tokio::test]
async fn features_photo_cdn_http_503_retains_unavailable_service_without_body_secrets_or_final_file(
) {
    feature_photo_cdn_http_failure("503 Service Unavailable", "503").await;
}

#[tokio::test]
async fn features_photo_cdn_advertised_size_over_file_limit_is_an_error_without_artifacts() {
    feature_photo_cdn_size_failure(true, 8, 100).await;
}

#[tokio::test]
async fn features_photo_cdn_streamed_size_over_remaining_budget_is_an_error_without_artifacts() {
    feature_photo_cdn_size_failure(false, 100, 8).await;
}

async fn feature_photo_cdn_http_failure(status: &'static str, code: &str) {
    let dir = tempfile::tempdir().unwrap();
    let body = b"CDN raw payload https://evil.invalid/?token=synthetic-secret-marker Cookie: synthetic-cookie-secret BOT_TOKEN=synthetic-bot-token";
    let response = mock_photo_response(status, body, true, "image/jpeg").await;
    let error = save_image_response(response, dir.path(), "photo.jpg", 1024, 1024, Some("jpg"))
        .await
        .unwrap_err();
    assert!(
        !dir.path().join("photo.jpg").exists(),
        "an HTTP rejection must not create sendable media"
    );
    let detail = error.to_string();
    for secret in [
        SECRET,
        "evil.invalid",
        "token=",
        "Cookie:",
        "BOT_TOKEN=",
        "synthetic-cookie-secret",
        "synthetic-bot-token",
        "CDN raw payload",
        "127.0.0.1",
    ] {
        assert!(
            !detail.contains(secret),
            "CDN response or request data leaked into the diagnostic: {detail}"
        );
    }
    assert!(
        detail.contains(code),
        "observed HTTP {code} must survive the CDN failure: {detail}"
    );
}

async fn feature_photo_cdn_size_failure(with_length: bool, max_bytes: u64, total_remaining: u64) {
    let dir = tempfile::tempdir().unwrap();
    let response = mock_photo_response("200 OK", b"123456789", with_length, "image/jpeg").await;
    let error = save_image_response(
        response,
        dir.path(),
        "photo.jpg",
        max_bytes,
        total_remaining,
        Some("jpg"),
    )
    .await
    .unwrap_err();
    assert!(
        fs::read_dir(dir.path()).unwrap().next().is_none(),
        "size rejection must leave neither completed media nor temporary photo fragments"
    );
    let detail = error.to_string().to_lowercase();
    assert!(
        ["size", "limit", "размер", "лимит"]
            .iter()
            .any(|cause| detail.contains(cause)),
        "a known CDN size rejection must expose its size/limit cause: {detail}"
    );
}

use crate::prelude::*;
use crate::read_url::*;

#[test]
fn validates_only_public_http_urls() {
    for raw in [
        "http://localhost/",
        "http://127.0.0.1/",
        "http://10.0.0.1/",
        "http://192.168.1.1/",
        "http://intranet/",
        "file:///etc/passwd",
        "ftp://example.com/",
        "http://user:pass@example.com/",
        "http://example.com:8080/",
    ] {
        let url = Url::parse(raw).unwrap();
        assert!(validate_url(&url).is_err(), "{raw} must be rejected");
    }
    assert!(validate_url(&Url::parse("https://example.com/article").unwrap()).is_ok());
    assert!(validate_url(&Url::parse("http://example.com/article").unwrap()).is_ok());
}

#[test]
fn extracts_article_and_skips_chrome() {
    let html = r#"
        <html><head><title>Заголовок статьи</title>
        <style>body { color: red }</style></head>
        <body>
          <nav>меню сайта</nav>
          <article>
            <h1>Заголовок статьи</h1>
            <p>Первый <b>абзац</b> статьи.</p>
            <p>Второй абзац.</p>
            <script>alert('x')</script>
          </article>
          <footer>подвал</footer>
        </body></html>
    "#;
    let (title, text) =
        extract_html(html, &Url::parse("https://example.com/article").unwrap()).unwrap();
    assert_eq!(title.as_deref(), Some("Заголовок статьи"));
    let text = normalize_text(&text);
    assert!(text.contains("Первый"), "{text}");
    assert!(text.contains("абзац"), "{text}");
    assert!(text.contains("статьи."), "{text}");
    assert!(text.contains("Второй абзац."), "{text}");
    assert!(!text.contains("меню сайта"), "{text}");
    assert!(!text.contains("подвал"), "{text}");
    assert!(!text.contains("alert"), "{text}");
}

#[test]
fn renders_text_with_a_limit() {
    let body = b"hello\n\n\nworld";
    let page = render_page(
        &Url::parse("https://example.com/a").unwrap(),
        "text/plain",
        body,
    )
    .unwrap();
    assert!(page.text.contains("hello"));
    assert!(page.text.contains("world"));
}

#[tokio::test]
#[ignore = "network smoke test"]
async fn live_fetch_example_page() {
    let page = read("https://example.com/").await.unwrap();
    assert!(page.text.contains("Example Domain"), "{}", page.text);
}

#[tokio::test]
#[ignore = "network smoke test"]
async fn live_fetch_article_page() {
    let page = read("https://en.wikipedia.org/wiki/Rust_(programming_language)")
        .await
        .unwrap();
    assert!(page.text.contains("Rust"), "{}", page.text);
    assert!(page.text.chars().count() <= READ_URL_MAX_CHARS + 200);
}

#[test]
fn fallback_drops_scripts_and_preserves_spacing() {
    let document = clean_document(
        "<html><body><script>alert('secret')</script><style>.x{}</style>             <p>hello <b>world</b></p></body></html>",
    );
    let text = fallback_text(&document);
    assert!(text.contains("hello world"), "{text}");
    assert!(!text.contains("secret"), "{text}");
    assert!(!text.contains(".x"), "{text}");
}

#[test]
fn oversized_titles_are_capped_and_sanitized() {
    let raw = format!("{}\n\u{0007}{}", "a".repeat(5_000), "b".repeat(5_000));
    let title = sanitize_title(&raw);
    assert!(title.chars().count() <= READ_URL_TITLE_CHARS);
    assert!(!title.chars().any(char::is_control));
}

#[test]
fn tiny_pages_fall_back_to_body_text() {
    let html = "<html><head><title>Tiny</title></head><body><p>hello</p></body></html>";
    let (title, text) =
        extract_html(html, &Url::parse("https://example.com/tiny").unwrap()).unwrap();
    assert_eq!(title.as_deref(), Some("Tiny"));
    assert!(text.contains("hello"), "{text}");
}

#[test]
fn decodes_declared_non_utf8_charsets() {
    let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("привет");
    assert_eq!(
        decode_body(&bytes, "text/plain; charset=windows-1251"),
        "привет"
    );
}

#[test]
fn only_urls_from_the_prompt_are_allowed() {
    let prompt = "прочитай https://example.com/article?x=1 и всё";
    let allowed = prompt_urls(prompt);
    assert!(allowed_target("https://example.com/article?x=1", &allowed));
    assert!(!allowed_target("https://example.com/article?x=2", &allowed));
    assert!(!allowed_target(
        "https://attacker.example/?q=chat-summary",
        &allowed
    ));
    assert!(!allowed_target(
        "https://example.com/article?x=1&q=secret",
        &allowed
    ));
}

#[test]
fn deeply_nested_html_does_not_overflow() {
    let html = format!(
        "{}deep text{}",
        "<div>".repeat(5_000),
        "</div>".repeat(5_000)
    );
    assert!(matches!(
        extract_html(&html, &Url::parse("https://example.com/deep").unwrap()),
        Err(ReadUrlError::TooComplex)
    ));
}

#[test]
fn truncation_does_not_split_utf8() {
    let (text, truncated) = truncate_chars("привет", 2);
    assert_eq!(text, "пр");
    assert!(truncated);
}

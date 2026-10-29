use crate::*;

#[test]
fn caption_escapes_every_html_attribute_character_without_changing_link_target() {
    assert_eq!(
        escape_html_attribute("a&<>\"'b"),
        "a&amp;&lt;&gt;&quot;&#39;b"
    );
}

#[test]
fn media_caption_uses_escaped_first_name_even_with_username_and_last_name() {
    let sender = serde_json::from_value::<teloxide::types::User>(serde_json::json!({
            "id": 42, "is_bot": false, "first_name": "Kitty & <\"'🐈> @username tg://user?id=99 </code><a href=\"tg://user?id=99\">Injected</a>",
            "username": "cat_sender", "last_name": "Surname"
        })).unwrap();
    let url = allowed_url("https://youtu.be/abc?x=1&next=two").unwrap();

    assert_eq!(media_caption(&sender, &url, Some("Описание & <кот>")).unwrap(),
            "Описание &amp; &lt;кот&gt;\n<code>От Kitty &amp; &lt;&quot;&#39;🐈&gt; @username tg://user?id=99 &lt;/code&gt;&lt;a href=&quot;tg://user?id=99&quot;&gt;Injected&lt;/a&gt;</code>\n<a href=\"https://youtu.be/abc?x=1&amp;next=two\">Ссылка</a>");
}

#[test]
fn caption_text_keeps_title_and_deduplicates_matching_description() {
    assert_eq!(
        caption_text(Some("Заголовок"), Some("Описание")).as_deref(),
        Some("Заголовок\n\nОписание")
    );
    assert_eq!(
        caption_text(Some("Одинаково"), Some("Одинаково")).as_deref(),
        Some("Одинаково")
    );
    assert_eq!(
        caption_text(Some("Video by someone"), Some("Описание")).as_deref(),
        Some("Описание")
    );
    assert_eq!(
        caption_text(Some("TikTok video #7680239168298978581"), Some("")).as_deref(),
        None
    );
    assert_eq!(
        caption_text(Some("TikTok video #7680239168298978581"), None).as_deref(),
        None
    );
    assert_eq!(
        caption_text(Some("Настоящий заголовок #1"), None).as_deref(),
        Some("Настоящий заголовок #1")
    );
    assert_eq!(
        caption_text(Some(""), Some("Описание")).as_deref(),
        Some("Описание")
    );
    assert_eq!(
        caption_text(Some("Заголовок"), Some("")).as_deref(),
        Some("Заголовок")
    );
    assert_eq!(caption_text(None, None), None);
}

#[test]
fn source_comment_strips_only_the_selected_link_and_keeps_line_breaks() {
    let url = allowed_url("https://youtu.be/abc").unwrap();
    assert_eq!(source_comment("https://youtu.be/abc", &url), None);
    assert_eq!(source_comment("  https://youtu.be/abc  ", &url), None);
    assert_eq!(
        source_comment("смотрите\nhttps://youtu.be/abc", &url).as_deref(),
        Some("смотрите")
    );
    assert_eq!(
        source_comment("смотрите https://youtu.be/abc до конца", &url).as_deref(),
        Some("смотрите до конца")
    );
    assert_eq!(
        source_comment("ссылка (https://youtu.be/abc).", &url).as_deref(),
        Some("ссылка")
    );
}

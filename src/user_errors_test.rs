use crate::*;

#[test]
fn user_error_messages_explain_safe_categories() {
    let format = crate::user_errors::diagnostic_message(
        FailureDiagnostic {
            stage: "yt-dlp",
            detail: "Format unavailable",
            exit_code: Some(1),
        },
        100,
    );
    assert!(format.contains("формат"), "{format}");
    let fallback = crate::user_errors::diagnostic_message(
        FailureDiagnostic {
            stage: "yt-dlp",
            detail: "Download failed",
            exit_code: Some(42),
        },
        100,
    );
    assert!(fallback.contains("42"), "{fallback}");
    assert!(fallback.contains("не распознана"), "{fallback}");
    assert!(!fallback.contains("secret"));
}

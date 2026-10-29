use crate::code_inspect::inspect;

#[test]
fn inspect_prefers_matches_and_respects_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/chat.rs"), "fn answer() {}\n").unwrap();
    std::fs::write(dir.path().join("secret.env"), "answer=secret\n").unwrap();
    let result = inspect(dir.path(), "answer", None);
    assert!(result.contains("src/chat.rs:1"), "{result}");
    assert!(!result.contains("secret"), "{result}");
}

#[cfg(unix)]
#[test]
fn inspect_does_not_follow_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.rs"), "leak_needle\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.rs"),
        dir.path().join("src/leak.rs"),
    )
    .unwrap();
    let result = inspect(dir.path(), "leak_needle", None);
    assert!(!result.contains("leak_needle"), "{result}");
}

#[test]
fn inspect_rejects_traversal_and_unknown_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    std::fs::write(dir.path().join("secret.txt"), "hello\n").unwrap();
    assert!(!inspect(dir.path(), "hello", Some("../README.md")).contains("hello"));
    assert!(!inspect(dir.path(), "hello", Some("secret.txt")).contains("hello"));
    assert!(inspect(dir.path(), "hello", Some("README.md")).contains("hello"));
}

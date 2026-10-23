//! Explicitly opt in only inside a disposable graphical session.
use std::process::Command;

const SAMPLE: &str = "Our Clipboard synthetic test\nZażółć gęślą jaźń 🦀\n第二行";

#[test]
#[ignore = "replaces the clipboard; requires a disposable graphical session"]
fn independent_process_reads_unicode_from_clipboard() {
    let mut clipboard =
        our_clipboard::clipboard::TextClipboard::new().expect("clipboard backend unavailable");
    clipboard.set_text(SAMPLE).expect("clipboard write failed");
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "clipboard_reader", "--nocapture"])
        .env("OUR_CLIPBOARD_TEST_READER", "1")
        .output()
        .expect("reader process failed to start");
    assert!(
        child.status.success(),
        "independent clipboard reader failed"
    );
    // Keep the owner alive until the other process has read the selection.
    drop(clipboard);
}

#[test]
#[ignore = "helper for the isolated clipboard integration test"]
fn clipboard_reader() {
    if std::env::var_os("OUR_CLIPBOARD_TEST_READER").is_none() {
        return;
    }
    let observed = our_clipboard::clipboard::TextClipboard::new()
        .expect("clipboard backend unavailable")
        .get_text()
        .expect("clipboard read failed");
    // Avoid assert_eq!, which would expose actual clipboard contents on failure.
    assert!(observed == SAMPLE, "clipboard contents differ (omitted)");
}

#[test]
#[ignore = "replaces the clipboard; requires a disposable graphical session"]
fn invalid_writes_leave_existing_clipboard_unchanged() {
    let mut clipboard =
        our_clipboard::clipboard::TextClipboard::new().expect("clipboard backend unavailable");
    clipboard.set_text(SAMPLE).expect("clipboard write failed");
    let oversized = "x".repeat(our_clipboard::text::MAX_TEXT_BYTES + 1);
    for invalid in ["", "before\0after", oversized.as_str()] {
        assert!(clipboard.set_text(invalid).is_err());
        let observed = clipboard.get_text().expect("clipboard read failed");
        assert!(
            observed == SAMPLE,
            "rejected write changed clipboard (omitted)"
        );
    }
}

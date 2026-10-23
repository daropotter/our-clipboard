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

#[test]
#[ignore = "seeds synthetic text; requires a disposable graphical session"]
fn clipboard_doctor_reports_backend_without_changing_contents() {
    let mut clipboard =
        our_clipboard::clipboard::TextClipboard::new().expect("clipboard backend unavailable");
    clipboard.set_text(SAMPLE).expect("clipboard write failed");
    let executable = std::env::var_os("OUR_CLIPBOARD_TEST_CLI")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_our-clipboard").into());
    let output = Command::new(executable)
        .args(["doctor", "--clipboard"])
        .output()
        .expect("doctor process failed to start");
    assert!(output.status.success(), "clipboard doctor failed");
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains(clipboard.backend_name()));
    assert!(report.contains("Read/write verification: not performed"));
    assert!(!report.contains(SAMPLE), "doctor exposed clipboard text");
    let observed = clipboard.get_text().expect("clipboard read failed");
    assert!(
        observed == SAMPLE,
        "doctor changed clipboard contents (omitted)"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "writes synthetic text; requires a disposable graphical session"]
fn native_watch_reports_changes_without_exposing_text() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let executable = std::env::var_os("OUR_CLIPBOARD_TEST_CLI")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_our-clipboard").into());
    // Seed a baseline before subscription: starting the observer must not emit it.
    let mut clipboard = our_clipboard::clipboard::TextClipboard::new().unwrap();
    clipboard.set_text(SAMPLE).unwrap();
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        Command::new(executable)
            .args(["watch-probe", "--observe-clipboard", "2"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (send, receive) = mpsc::sync_channel(4);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let ready = receive
        .recv_timeout(Duration::from_secs(3))
        .expect("watcher did not become ready");
    assert!(ready.starts_with("Watching: "));
    // A conscious copy of identical text must still be a separate native change.
    clipboard.set_text(SAMPLE).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    clipboard.set_text(SAMPLE).unwrap();
    let summary = receive
        .recv_timeout(Duration::from_secs(4))
        .expect("watcher did not finish");
    assert!(
        summary.starts_with("Clipboard notifications: 2."),
        "unexpected notification count"
    );
    assert!(!summary.contains(SAMPLE));
    assert!(child.0.wait().unwrap().success());
    reader.join().unwrap();
}

#![cfg(feature = "os-keyring")]
use std::process::Command;

#[test]
#[ignore = "writes an ephemeral credential; use disposable OS/Secret Service stores"]
fn client_identity_survives_process_restarts() {
    let mut nonce = [0; 12];
    getrandom::fill(&mut nonce).unwrap();
    let profile = format!(
        "fixture-{:x}",
        u128::from_le_bytes({
            let mut n = [0; 16];
            n[..12].copy_from_slice(&nonce);
            n
        })
    );
    let config = tempfile::tempdir().unwrap();
    let executable = std::env::var_os("OUR_CLIPBOARD_TEST_CLI")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_our-clipboard").into());
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Ok(entry) = keyring::Entry::new(
                "org.daropotter.our-clipboard",
                &format!("client-v1:{}", self.0),
            ) {
                let _ = entry.delete_credential();
            }
        }
    }
    let _cleanup = Cleanup(profile.clone());
    let call = |action| {
        Command::new(&executable)
            .args(["identity", action, "--profile", profile.as_str()])
            .env("OUR_CLIPBOARD_CONFIG_DIR", config.path())
            .output()
            .unwrap()
    };
    let first = call("init");
    assert!(
        first.status.success(),
        "credential initialization failed (details omitted)"
    );
    let second = call("show");
    assert!(
        second.status.success(),
        "identity reload failed (details omitted)"
    );
    let repeated = call("init");
    assert!(repeated.status.success());
    assert!(
        first.stdout == second.stdout && second.stdout == repeated.stdout,
        "public identity changed after restart"
    );
    assert!(
        String::from_utf8(first.stdout)
            .unwrap()
            .contains("ed25519-sha256-v1:")
    );
    // The config directory has public metadata and a lock only; secrets live in the OS store.
    let mut files = std::fs::read_dir(config.path().join(&profile))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(files, ["device.pub", "identity.lock"]);
}

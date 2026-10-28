#![cfg(feature = "os-keyring")]
use std::process::Command;

#[test]
#[ignore = "writes an ephemeral credential; use disposable OS/Secret Service stores"]
fn client_identity_survives_process_restarts() {
    check_native_identity(false);
}

#[cfg(feature = "clipboard")]
#[test]
#[ignore = "writes an ephemeral credential; use disposable OS/Secret Service stores"]
fn native_preflight_reads_persistent_identity_and_peer_trust() {
    check_native_identity(true);
}

fn check_native_identity(check_preflight: bool) {
    #[cfg(not(feature = "clipboard"))]
    assert!(!check_preflight);
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
    // Explicit peer approval is signed by the actual OS-store owner key and is
    // authenticated again by a different process. No clipboard or sockets needed.
    use our_clipboard::{
        events::SessionId,
        identity::DeviceIdentity,
        transport::outbound::{RelayIdentity, Room, Side, SignedRelayOffer},
    };
    use std::{io::Write, process::Stdio};
    let remote = DeviceIdentity::generate().unwrap();
    let room = Room::parse(&"1".repeat(64)).unwrap();
    let signed = SignedRelayOffer::new(
        &remote,
        SessionId::random().unwrap(),
        room,
        Side::B,
        &RelayIdentity::generate().unwrap().public_offer(),
    )
    .unwrap();
    let offer = config.path().join("peer.offer");
    std::fs::write(&offer, signed.to_wire()).unwrap();
    let peer_call = |action: &str, confirmation: Option<&str>| {
        let mut command = Command::new(&executable);
        command
            .args(["peer", action, "--profile", &profile, "--name", "remote"])
            .env("OUR_CLIPBOARD_CONFIG_DIR", config.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if action == "trust" {
            command.args([
                "--offer",
                offer.to_str().unwrap(),
                "--room",
                &"1".repeat(64),
                "--side",
                "b",
            ]);
        }
        let mut child = command.spawn().unwrap();
        if let Some(text) = confirmation {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("{text}\n").as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
    };
    let pin = config.path().join(&profile).join("peers/remote.pin");
    assert_eq!(peer_call("show", None).status.code(), Some(2));
    assert_eq!(peer_call("trust", Some("yes")).status.code(), Some(2));
    assert!(!pin.exists(), "failed approval created trust");
    let fingerprint = remote.id().fingerprint();
    let approved = peer_call("trust", Some(&fingerprint));
    assert!(
        approved.status.success(),
        "explicit peer approval failed (details omitted)"
    );
    let original = std::fs::read(&pin).unwrap();
    let shown = peer_call("show", None);
    assert!(shown.status.success());
    assert!(
        String::from_utf8(shown.stdout)
            .unwrap()
            .contains(&fingerprint)
    );
    #[cfg(feature = "clipboard")]
    if check_preflight {
        let launch = config.path().join("launch.json");
        let data = serde_json::json!({"version":1,"relay":"wss://relay.invalid/our-clipboard/v1","room":"1".repeat(64),"side":"a","identity_profile":profile,"peer_name":"remote","offer":"own.offer","peer_offer":"pending.offer","max_drift_ppm":1000,"clock_granularity_ms":1});
        std::fs::write(&launch, data.to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&launch, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let checked = preflight(&executable, config.path());
        assert!(
            checked.status.success(),
            "Native local preflight failed (details omitted)"
        );
        let text = String::from_utf8(checked.stdout).unwrap();
        assert!(text.contains("LOCAL PREFLIGHT PASSED") && text.contains(&fingerprint));
        assert!(!config.path().join("own.offer").exists());
        assert!(!config.path().join("pending.offer").exists());
        assert_eq!(std::fs::read(&pin).unwrap(), original);
    }
    assert_eq!(
        peer_call("trust", Some(&fingerprint)).status.code(),
        Some(2)
    );
    assert_eq!(
        std::fs::read(&pin).unwrap(),
        original,
        "existing pin was replaced"
    );
    std::fs::write(&pin, b"corrupt").unwrap();
    assert_eq!(peer_call("show", None).status.code(), Some(2));
    assert_eq!(
        peer_call("trust", Some(&fingerprint)).status.code(),
        Some(2)
    );
    assert_eq!(std::fs::read(&pin).unwrap(), b"corrupt");
    #[cfg(feature = "clipboard")]
    if check_preflight {
        let rejected = preflight(&executable, config.path());
        assert_eq!(rejected.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&rejected.stdout).contains("LOCAL PREFLIGHT PASSED"));
        assert_eq!(std::fs::read(&pin).unwrap(), b"corrupt");

        // A valid credential and approval must not cause missing metadata to be repaired.
        std::fs::write(&pin, &original).unwrap();
        let profile_dir = config.path().join(&profile);
        let lock = profile_dir.join("identity.lock");
        std::fs::remove_file(&lock).unwrap();
        assert_eq!(preflight(&executable, config.path()).status.code(), Some(2));
        assert!(!lock.exists(), "preflight recreated the profile lock");
        assert_eq!(std::fs::read(&pin).unwrap(), original);

        let saved = config.path().join("saved-profile");
        std::fs::rename(&profile_dir, &saved).unwrap();
        assert_eq!(preflight(&executable, config.path()).status.code(), Some(2));
        assert!(
            !profile_dir.exists(),
            "preflight recreated the profile directory"
        );
        assert_eq!(
            std::fs::read(saved.join("peers/remote.pin")).unwrap(),
            original
        );
        assert!(!config.path().join("own.offer").exists());
        assert!(!config.path().join("pending.offer").exists());
    }
}

#[cfg(feature = "clipboard")]
fn preflight(executable: &std::ffi::OsStr, config: &std::path::Path) -> std::process::Output {
    Command::new(executable)
        .args(["pilot", "preflight", "--file"])
        .arg(config.join("launch.json"))
        .env("OUR_CLIPBOARD_CONFIG_DIR", config)
        .env("OUR_CLIPBOARD_RELAY_TOKEN", "a".repeat(64))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap()
}

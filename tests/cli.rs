use std::process::Command;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_is_available_without_a_graphical_session() {
    let output = cli(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("Usage:"));
}

#[test]
fn version_matches_package() {
    let output = cli(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("our-clipboard {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn doctor_reports_runtime_platform() {
    let output = cli(&["doctor"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains(std::env::consts::OS)
    );
}

#[test]
fn invalid_commands_fail_without_claiming_success() {
    for args in [
        vec!["client"],
        vec!["clipboard-probe"],
        vec!["relay"],
        vec!["watch-probe"],
        vec!["watch-probe", "--observe-clipboard", "0"],
        vec!["watch-probe", "--observe-clipboard", "61"],
        vec!["watch-probe", "--observe-clipboard", "invalid"],
        vec!["doctor", "unexpected"],
        vec!["doctor", "--clipboard", "unexpected"],
    ] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
#[cfg(all(feature = "clipboard", target_os = "linux"))]
fn clipboard_probe_fails_clearly_without_a_graphical_session() {
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args(["clipboard-probe", "--replace-clipboard"])
        .env_remove("XDG_SESSION_TYPE")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env("XDG_RUNTIME_DIR", "/nonexistent-our-clipboard-test-session")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Clipboard unavailable")
    );
}

#[test]
#[cfg(all(feature = "clipboard", target_os = "linux"))]
fn unavailable_wayland_does_not_fall_back_to_x11() {
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args(["clipboard-probe", "--replace-clipboard"])
        .env("XDG_SESSION_TYPE", "wayland")
        .env("WAYLAND_DISPLAY", "our-clipboard-nonexistent-socket")
        .env("DISPLAY", ":99")
        .env("XDG_RUNTIME_DIR", "/nonexistent-our-clipboard-test-session")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("XWayland fallback is disabled")
    );
}

#[test]
#[cfg(all(feature = "clipboard", target_os = "linux"))]
fn clipboard_doctor_reports_unavailable_without_a_session() {
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args(["doctor", "--clipboard"])
        .env_remove("XDG_SESSION_TYPE")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY")
        .env("XDG_RUNTIME_DIR", "/nonexistent-our-clipboard-test-session")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Clipboard unavailable")
    );
}

#[test]
fn synthetic_transport_probe_runs_without_a_graphical_session() {
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .arg("transport-probe")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("XDG_SESSION_TYPE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "transport probe failed (details omitted)"
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("Untrusted peers rejected on both sides"));
    assert!(report.contains("Synthetic data only"));
}

#[test]
#[cfg(feature = "os-keyring")]
fn invalid_identity_profile_is_rejected_without_accessing_credentials() {
    let output = cli(&["identity", "init", "--profile", "../other"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Identity profile must contain")
    );
}

#[test]
fn signed_envelope_probe_needs_no_graphical_session_or_credential_store() {
    let output = cli(&["envelope-probe"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Tampering and duplicate delivery rejected")
    );
}

#[test]
fn freshness_probe_needs_no_display_or_credential_store() {
    let output = cli(&["freshness-probe"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("UTC-independent age checks passed")
    );
}

#[test]
fn durable_probe_uses_temporary_state_without_display_or_keyring() {
    let output = cli(&["state-probe"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("reservations survived restart")
    );
}

#[cfg(feature = "clipboard")]
#[test]
fn pilot_requires_consent_and_explicit_clock_bounds_before_touching_clipboard() {
    for args in [
        vec!["pilot"],
        vec!["pilot", "--replace-clipboard", "--stdio"],
        vec!["pilot", "--ssh", "-ProxyCommand=bad"],
    ] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[cfg(feature = "clipboard")]
#[test]
fn pilot_help_needs_no_graphical_session_or_ssh() {
    let output = cli(&["pilot", "--help"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("--max-drift-ppm")
    );
}

#[test]
fn relay_help_and_missing_access_credentials_do_not_initialize_clipboard() {
    let help = cli(&["relay", "--help"]);
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("--tls-cert")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args([
            "relay",
            "--listen",
            "127.0.0.1:0",
            "--tls-cert",
            "/nonexistent-cert",
            "--tls-key",
            "/nonexistent-key",
        ])
        .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn relay_preflight_requires_credentials_and_works_without_clipboard() {
    let help = cli(&["relay-check", "--help"]);
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("no clipboard")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args(["relay-check", "--relay", "wss://localhost/our-clipboard/v1"])
        .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("OUR_CLIPBOARD_RELAY_TOKEN")
    );
}

#[cfg(feature = "clipboard")]
#[test]
fn paused_pilot_does_not_create_offers_or_require_transport_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("pause");
    std::fs::write(&marker, []).unwrap();
    let offer = dir.path().join("own.offer");
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args([
            "pilot",
            "--replace-clipboard",
            "--relay",
            "wss://localhost/our-clipboard/v1",
            "--room",
            &"1".repeat(64),
            "--side",
            "a",
            "--offer",
            offer.to_str().unwrap(),
            "--peer-offer",
            dir.path().join("peer.offer").to_str().unwrap(),
            "--max-drift-ppm",
            "1000",
            "--clock-granularity-ms",
            "1",
            "--pause-file",
            marker.to_str().unwrap(),
        ])
        .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("PAUSED"));
    assert!(!offer.exists());
}

#[cfg(all(feature = "clipboard", not(target_os = "linux")))]
#[test]
fn logind_option_has_no_silent_fallback_on_other_platforms() {
    let directory = tempfile::tempdir().unwrap();
    let offer = directory.path().join("own.offer");
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args([
            "pilot",
            "--replace-clipboard",
            "--relay",
            "wss://localhost/our-clipboard/v1",
            "--room",
            &"1".repeat(64),
            "--side",
            "a",
            "--offer",
            offer.to_str().unwrap(),
            "--peer-offer",
            directory.path().join("peer.offer").to_str().unwrap(),
            "--max-drift-ppm",
            "1000",
            "--clock-granularity-ms",
            "1",
            "--logind-idle",
        ])
        .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("requires Linux/X11")
    );
    assert!(!offer.exists());
}

#[cfg(all(feature = "clipboard", not(target_os = "macos")))]
#[test]
fn macos_session_option_never_falls_back_on_another_os() {
    let dir = tempfile::tempdir().unwrap();
    let offer = dir.path().join("own.offer");
    let output = Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
        .args([
            "pilot",
            "--replace-clipboard",
            "--relay",
            "wss://localhost/our-clipboard/v1",
            "--room",
            &"1".repeat(64),
            "--side",
            "a",
            "--offer",
            offer.to_str().unwrap(),
            "--peer-offer",
            dir.path().join("peer.offer").to_str().unwrap(),
            "--max-drift-ppm",
            "1000",
            "--clock-granularity-ms",
            "1",
            "--macos-session-idle",
        ])
        .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("requires macOS")
    );
    assert!(!offer.exists());
}

#[cfg(all(feature = "clipboard", target_os = "macos"))]
#[test]
fn macos_session_diagnostics_never_claim_clipboard_delivery() {
    let output = cli(&["doctor", "--session"]);
    if output.status.success() {
        let status = String::from_utf8(output.stdout).unwrap();
        assert!(status.contains("Active") || status.contains("Idle"));
        assert!(status.contains("No clipboard data accessed"));
    } else {
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("Unknown")
        );
        assert!(output.stdout.is_empty());
    }
}

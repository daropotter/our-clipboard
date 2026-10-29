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

#[cfg(all(feature = "clipboard", target_os = "linux"))]
#[test]
fn signed_pilot_never_creates_an_offer_or_falls_back_when_identity_store_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join("own.offer");
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
            own.to_str().unwrap(),
            "--peer-offer",
            dir.path().join("peer.offer").to_str().unwrap(),
            "--max-drift-ppm",
            "1000",
            "--clock-granularity-ms",
            "1",
            "--identity-profile",
            "missing",
        ])
        .env("OUR_CLIPBOARD_RELAY_TOKEN", "a".repeat(64))
        .env("OUR_CLIPBOARD_CONFIG_DIR", dir.path())
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", dir.path().join("no-bus").display()),
        )
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env("XDG_SESSION_TYPE", "x11")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!own.exists());
    assert!(!dir.path().join("missing/device.pub").exists());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    #[cfg(feature = "os-keyring")]
    assert!(
        diagnostic.contains("credential store unavailable")
            || diagnostic.contains("Identity is missing")
    );
    #[cfg(not(feature = "os-keyring"))]
    assert!(diagnostic.contains("no ephemeral fallback"));
}

#[cfg(feature = "os-keyring")]
#[test]
fn peer_commands_require_explicit_context_before_opening_any_store() {
    assert!(cli(&["peer", "--help"]).status.success());
    for args in [
        vec!["peer", "trust", "--name", "remote"],
        vec!["peer", "show", "--name", "../remote"],
        vec!["peer", "show", "--name", "x", "--offer", "ignored"],
        vec!["peer", "show", "--name", "x", "--name", "y"],
    ] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[cfg(feature = "clipboard")]
#[test]
fn launch_check_is_read_only_and_runtime_consent_cannot_come_from_json() {
    let config_dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("launch.json");
    let settings = serde_json::json!({
        "version": 1, "relay": "wss://relay.invalid/our-clipboard/v1", "room": "1".repeat(64),
        "side": "a", "identity_profile": "not-initialized", "peer_name": "not-approved",
        "offer": "own.offer", "peer_offer": "peer.offer", "max_drift_ppm": 1000, "clock_granularity_ms": 1,
    });
    let save = |value: &serde_json::Value| {
        std::fs::write(&path, value.to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    };
    save(&settings);
    let call = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
            .args(args)
            .current_dir(elsewhere.path())
            .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env_remove("XDG_SESSION_TYPE")
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-our-clipboard-bus",
            )
            .env(
                "OUR_CLIPBOARD_CONFIG_DIR",
                config_dir.path().join("unused-identities"),
            )
            .output()
            .unwrap()
    };
    let checked = call(&["pilot", "config-check", "--file", path.to_str().unwrap()]);
    assert!(checked.status.success());
    assert!(
        String::from_utf8(checked.stdout)
            .unwrap()
            .contains("No credentials")
    );
    let preflight = call(&["pilot", "preflight", "--file", path.to_str().unwrap()]);
    assert_eq!(preflight.status.code(), Some(2));
    assert!(
        String::from_utf8(preflight.stderr)
            .unwrap()
            .contains("Relay credential missing or invalid")
    );
    let existing_offer = config_dir.path().join("own.offer");
    std::fs::write(&existing_offer, b"unchanged").unwrap();
    let blocked = call(&["pilot", "preflight", "--file", path.to_str().unwrap()]);
    assert_eq!(blocked.status.code(), Some(2));
    assert!(
        String::from_utf8(blocked.stderr)
            .unwrap()
            .contains("Own public offer path already exists")
    );
    assert_eq!(std::fs::read(&existing_offer).unwrap(), b"unchanged");
    std::fs::remove_file(existing_offer).unwrap();
    let missing_consent = call(&["pilot", "--config", path.to_str().unwrap()]);
    assert_eq!(missing_consent.status.code(), Some(2));
    assert!(
        String::from_utf8(missing_consent.stderr)
            .unwrap()
            .contains("explicit --replace-clipboard")
    );
    for field in ["replace_clipboard", "token"] {
        let mut invalid = settings.clone();
        invalid[field] = serde_json::json!("SECRET-OR-CONSENT-MUST-NOT-APPEAR");
        save(&invalid);
        let rejected = call(&[
            "pilot",
            "--config",
            path.to_str().unwrap(),
            "--replace-clipboard",
        ]);
        assert_eq!(rejected.status.code(), Some(2));
        assert!(
            !String::from_utf8(rejected.stderr)
                .unwrap()
                .contains("SECRET-OR-CONSENT-MUST-NOT-APPEAR")
        );
    }
    let mut paused_settings = settings.clone();
    paused_settings["pause_file"] = serde_json::json!("pause");
    save(&paused_settings);
    std::fs::write(config_dir.path().join("pause"), []).unwrap();
    let paused = call(&[
        "pilot",
        "--config",
        path.to_str().unwrap(),
        "--replace-clipboard",
    ]);
    assert!(paused.status.success());
    assert!(String::from_utf8(paused.stdout).unwrap().contains("PAUSED"));
    let paused_preflight = call(&["pilot", "preflight", "--file", path.to_str().unwrap()]);
    assert!(paused_preflight.status.success());
    assert!(
        String::from_utf8(paused_preflight.stdout)
            .unwrap()
            .contains("PAUSED")
    );
    for root in [config_dir.path(), elsewhere.path()] {
        assert!(!root.join("own.offer").exists());
        assert!(!root.join("peer.offer").exists());
    }
    assert!(!config_dir.path().join("unused-identities").exists());
}

#[test]
#[cfg(all(feature = "clipboard", unix))]
fn relay_structured_status_is_content_free_on_pause_and_setup_failure() {
    let root = tempfile::tempdir().unwrap();
    let pause = root.path().join("pause");
    let own = root.path().join("own.offer");
    let peer = root.path().join("peer.offer");
    let base = [
        "pilot",
        "--replace-clipboard",
        "--status-json",
        "--relay",
        "wss://relay.invalid/our-clipboard/v1",
        "--room",
        &"1".repeat(64),
        "--side",
        "a",
        "--offer",
        own.to_str().unwrap(),
        "--peer-offer",
        peer.to_str().unwrap(),
        "--pause-file",
        pause.to_str().unwrap(),
        "--max-drift-ppm",
        "1000",
        "--clock-granularity-ms",
        "1",
    ];
    std::fs::write(&pause, []).unwrap();
    let call = || {
        Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
            .args(base)
            .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_SESSION_TYPE")
            .output()
            .unwrap()
    };
    let paused = call();
    assert!(paused.status.success());
    assert_eq!(paused.stdout, b"{\"version\":1,\"state\":\"paused\"}\n");
    assert!(paused.stderr.is_empty());
    std::fs::remove_file(&pause).unwrap();
    let failed = call();
    assert_eq!(failed.status.code(), Some(2));
    assert_eq!(
        failed.stdout,
        b"{\"version\":1,\"state\":\"starting\"}\n{\"version\":1,\"state\":\"failed\"}\n"
    );
    assert!(!own.exists() && !peer.exists());
}

#[test]
#[cfg(feature = "clipboard")]
fn local_pilot_controls_need_no_credentials_and_never_replace_markers() {
    let config = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let path = config.path().join("launch.json");
    let pause = config.path().join("pause");
    let idle = config.path().join("idle");
    let mut data = serde_json::json!({
        "version":1, "relay":"wss://relay.invalid/our-clipboard/v1", "room":"1".repeat(64),
        "side":"a", "identity_profile":"not-initialized", "peer_name":"not-approved",
        "offer":"own.offer", "peer_offer":"peer.offer", "max_drift_ppm":1000,
        "clock_granularity_ms":1, "pause_file":"pause", "idle_file":"idle", "status_json":true
    });
    let save = |data: &serde_json::Value| {
        std::fs::write(&path, data.to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    };
    let call = |action: &str, extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
            .args(["pilot", action, "--file", path.to_str().unwrap()])
            .args(extra)
            .current_dir(cwd.path())
            .env(
                "OUR_CLIPBOARD_CONFIG_DIR",
                config.path().join("missing-identity-root"),
            )
            .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_SESSION_TYPE")
            .output()
            .unwrap()
    };
    save(&data);
    let shown = call("controls", &[]);
    assert!(shown.status.success());
    let text = String::from_utf8(shown.stdout).unwrap();
    assert!(
        text.contains("Local Pause marker: absent") && text.contains("Local Idle marker: absent")
    );
    assert!(!pause.exists() && !idle.exists());
    assert_eq!(
        call("pause", &["--replace-clipboard"]).status.code(),
        Some(2)
    );
    assert!(!pause.exists());
    let requested = call("pause", &[]);
    assert!(requested.status.success());
    assert!(
        String::from_utf8(requested.stdout)
            .unwrap()
            .contains("not acknowledgement")
    );
    assert!(pause.is_file());
    assert!(!cwd.path().join("pause").exists());
    std::fs::write(&pause, b"unrelated contents must survive").unwrap();
    std::fs::create_dir(&idle).unwrap();
    assert!(call("pause", &[]).status.success());
    assert_eq!(
        std::fs::read(&pause).unwrap(),
        b"unrelated contents must survive"
    );
    let shown = call("controls", &[]);
    assert!(shown.status.success());
    let text = String::from_utf8(shown.stdout).unwrap();
    assert!(
        text.contains("Local Pause marker: present") && text.contains("Local Idle marker: present")
    );
    assert_eq!(
        std::fs::read(&pause).unwrap(),
        b"unrelated contents must survive"
    );
    data.as_object_mut().unwrap().remove("pause_file");
    save(&data);
    assert_eq!(call("pause", &[]).status.code(), Some(2));
    let shown = call("controls", &[]);
    assert!(shown.status.success());
    assert!(
        String::from_utf8(shown.stdout)
            .unwrap()
            .contains("Local Pause marker: not configured")
    );
    data["pause_file"] = "missing-parent/pause".into();
    save(&data);
    assert_eq!(call("pause", &[]).status.code(), Some(2));
    assert!(!config.path().join("missing-parent").exists());
    assert!(!config.path().join("missing-identity-root").exists());
    assert!(
        !config.path().join("own.offer").exists() && !config.path().join("peer.offer").exists()
    );
}

#[test]
#[cfg(feature = "clipboard")]
fn experimental_wayland_is_explicit_native_only_and_rejects_logind() {
    let root = tempfile::tempdir().unwrap();
    let own = root.path().join("own.offer");
    let peer = root.path().join("peer.offer");
    let call = |flag: bool, native: bool, logind: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_our-clipboard"));
        command
            .args([
                "pilot",
                "--replace-clipboard",
                "--relay",
                "wss://relay.invalid/our-clipboard/v1",
                "--room",
                &"1".repeat(64),
                "--side",
                "a",
                "--offer",
                own.to_str().unwrap(),
                "--peer-offer",
                peer.to_str().unwrap(),
                "--max-drift-ppm",
                "1000",
                "--clock-granularity-ms",
                "1",
            ])
            .env_remove("OUR_CLIPBOARD_RELAY_TOKEN")
            .env_remove("WAYLAND_SOCKET");
        if flag {
            command.arg("--experimental-wayland");
        }
        if logind {
            command.arg("--logind-idle");
        }
        if native {
            command
                .env("WAYLAND_DISPLAY", "missing-wayland-socket")
                .env("DISPLAY", ":99")
                .env("XDG_SESSION_TYPE", "wayland");
        } else {
            command
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("DISPLAY")
                .env("XDG_SESSION_TYPE", "x11");
        }
        command.output().unwrap()
    };
    #[cfg(target_os = "linux")]
    {
        let denied = call(false, true, false);
        assert_eq!(denied.status.code(), Some(2));
        assert!(
            String::from_utf8(denied.stderr)
                .unwrap()
                .contains("explicit --experimental-wayland")
        );
        let x11 = call(true, false, false);
        assert_eq!(x11.status.code(), Some(2));
        assert!(
            String::from_utf8(x11.stderr)
                .unwrap()
                .contains("X11 fallback is disabled")
        );
        let accepted_options = call(true, true, false);
        assert_eq!(accepted_options.status.code(), Some(2));
        assert!(
            String::from_utf8(accepted_options.stderr)
                .unwrap()
                .contains("OUR_CLIPBOARD_RELAY_TOKEN")
        );
        let incompatible = call(true, true, true);
        assert_eq!(incompatible.status.code(), Some(2));
        assert!(
            String::from_utf8(incompatible.stderr)
                .unwrap()
                .contains("X11-only")
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let unsupported = call(true, true, false);
        assert_eq!(unsupported.status.code(), Some(2));
        assert!(
            String::from_utf8(unsupported.stderr)
                .unwrap()
                .contains("requires Linux native Wayland")
        );
    }
    assert!(!own.exists() && !peer.exists());
}

#[cfg(all(feature = "clipboard", target_os = "linux"))]
#[test]
fn linux_session_diagnostic_fails_closed_without_a_private_bus() {
    let dir = tempfile::tempdir().unwrap();
    for wayland in [false, true] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_our-clipboard"))
            .args(["doctor", "--session"])
            .env(
                "DBUS_SYSTEM_BUS_ADDRESS",
                format!("unix:path={}/missing-bus", dir.path().display()),
            )
            .env_remove("WAYLAND_DISPLAY")
            .env("XDG_SESSION_TYPE", if wayland { "wayland" } else { "x11" })
            .env_remove("DISPLAY")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("Unknown"));
        assert!(!error.contains(dir.path().to_str().unwrap()));
        if wayland {
            assert!(error.contains("Wayland"));
        }
    }
}

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

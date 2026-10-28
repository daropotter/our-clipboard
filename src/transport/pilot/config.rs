//! Strict local launch settings, never credentials or clipboard consent.
use super::*;
use serde::Deserialize;
use std::path::Component;
const MAX_BYTES: u64 = 64 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    version: u8,
    relay: String,
    room: String,
    side: String,
    identity_profile: String,
    peer_name: String,
    offer: String,
    peer_offer: String,
    max_drift_ppm: u32,
    clock_granularity_ms: u64,
    seconds: Option<u64>,
    ttl_ms: Option<u32>,
    relay_ca: Option<String>,
    idle_file: Option<String>,
    pause_file: Option<String>,
    #[serde(default)]
    reconnect: bool,
    #[serde(default)]
    logind_idle: bool,
    #[serde(default)]
    macos_session_idle: bool,
}
fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn path_value(base: &Path, value: &str) -> std::result::Result<String, &'static str> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("Launch paths must be nonempty, bounded and contain no control characters.");
    }
    let raw = Path::new(value);
    if !raw.is_absolute() && raw.components().any(|c| matches!(c, Component::Prefix(_))) {
        return Err("Drive-relative launch paths are not supported; use an absolute path.");
    }
    let path = base.join(raw);
    let name = path
        .file_name()
        .ok_or("Launch file paths must include a filename.")?;
    let parent = path
        .parent()
        .ok_or("Invalid launch file parent directory.")?
        .canonicalize()
        .map_err(|_| "Launch path parent directories must exist and be accessible.")?;
    if !parent.is_dir() {
        return Err("Launch path parent must be a directory.");
    }
    parent
        .join(name)
        .to_str()
        .map(str::to_owned)
        .ok_or("Resolved launch paths must be UTF-8.")
}
fn load(path: &Path) -> std::result::Result<Vec<String>, &'static str> {
    let initial = std::fs::symlink_metadata(path).map_err(|_| "Launch file unavailable.")?;
    if !initial.file_type().is_file() {
        return Err("Launch configuration must be a regular file, not a symlink.");
    }
    let file = File::open(path).map_err(|_| "Launch file unavailable.")?;
    let meta = file
        .metadata()
        .map_err(|_| "Could not inspect launch file.")?;
    if !meta.is_file() {
        return Err("Launch configuration must be a regular file.");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != rustix::process::geteuid().as_raw()
            || meta.permissions().mode() & 0o022 != 0
        {
            return Err(
                "Launch file must belong to the current user and not be group/world-writable.",
            );
        }
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "Could not resolve launch file.")?;
    let base = canonical.parent().ok_or("Invalid launch file location.")?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read launch file.")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Launch file exceeds the 64 KiB limit.");
    }
    // Never format serde's error: an unknown key or value may itself be a secret.
    let data: Launch = serde_json::from_slice(&bytes).map_err(|_| "Invalid launch JSON: unknown/duplicate fields, missing fields or wrong value types. Credentials and consent are not supported.")?;
    if data.version != 1 {
        return Err("Unsupported launch configuration version.");
    }
    if !valid_name(&data.identity_profile) || !valid_name(&data.peer_name) {
        return Err(
            "Launch identity profile and peer name must use 1-64 ASCII letters/digits/hyphens/underscores.",
        );
    }
    outbound::validate_relay_address(&data.relay).map_err(|_| "Launch relay must use wss://HOST/our-clipboard/v1 without user credentials, query or fragment.")?;
    let mut args = Vec::new();
    for (key, value) in [
        ("--relay", data.relay),
        ("--room", data.room),
        ("--side", data.side),
        ("--identity-profile", data.identity_profile),
        ("--peer-name", data.peer_name),
        ("--offer", path_value(base, &data.offer)?),
        ("--peer-offer", path_value(base, &data.peer_offer)?),
        ("--max-drift-ppm", data.max_drift_ppm.to_string()),
        (
            "--clock-granularity-ms",
            data.clock_granularity_ms.to_string(),
        ),
    ] {
        args.extend([key.to_string(), value]);
    }
    for (key, value) in [
        ("--relay-ca", data.relay_ca),
        ("--idle-file", data.idle_file),
        ("--pause-file", data.pause_file),
    ] {
        if let Some(value) = value {
            args.extend([key.to_string(), path_value(base, &value)?]);
        }
    }
    if let Some(value) = data.seconds {
        args.extend(["--seconds".into(), value.to_string()]);
    }
    if let Some(value) = data.ttl_ms {
        args.extend(["--ttl-ms".into(), value.to_string()]);
    }
    for (key, enabled) in [
        ("--reconnect", data.reconnect),
        ("--logind-idle", data.logind_idle),
        ("--macos-session-idle", data.macos_session_idle),
    ] {
        if enabled {
            args.push(key.to_string());
        }
    }
    // Pure parser validation only. This temporary flag is discarded; neither the
    // returned settings nor the JSON can authorize clipboard access.
    let mut validation = args.clone();
    validation.push("--replace-clipboard".into());
    options(&validation)?;
    Ok(args)
}
pub(super) fn check_command(args: &[String]) -> Option<std::result::Result<String, &'static str>> {
    if args.first().is_none_or(|a| a != "config-check") {
        return None;
    }
    Some(match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["config-check", "--help"] | ["config-check", "-h"] => Ok("Usage: our-clipboard pilot config-check --file FILE\nRead-only schema/options validation; no credential store, transport or clipboard access.".into()),
        ["config-check", "--file", file] => load(Path::new(file)).map(|_| "Launch configuration schema/options valid. No credentials, peer trust, platform/backend readiness, offer freshness, network or clipboard were tested.".into()),
        _ => Err("Usage: our-clipboard pilot config-check --file FILE"),
    })
}
pub(super) fn expand(args: &[String]) -> std::result::Result<Option<Vec<String>>, &'static str> {
    let config_prefix = args.first().is_some_and(|a| a == "--config")
        || (args.first().is_some_and(|a| a == "--replace-clipboard")
            && args.get(1).is_some_and(|a| a == "--config"));
    if !config_prefix {
        return Ok(None);
    }
    let file = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--config", file, "--replace-clipboard"] | ["--replace-clipboard", "--config", file] => {
            file.to_string()
        }
        _ => {
            return Err(
                "Launch mode requires exactly --config FILE and explicit --replace-clipboard; command-line overrides are not supported. Use config-check for read-only validation.",
            );
        }
    };
    let mut expanded = load(Path::new(&file))?;
    expanded.push("--replace-clipboard".into()); // Present explicitly in the caller's argv above.
    Ok(Some(expanded))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn save(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
        std::fs::write(path, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
    fn sample() -> serde_json::Value {
        serde_json::json!({"version":1,"relay":"wss://relay.invalid/our-clipboard/v1","room":"1".repeat(64),"side":"a","identity_profile":"workstation","peer_name":"remote","offer":"own.offer","peer_offer":"peer.offer","max_drift_ppm":1000,"clock_granularity_ms":1})
    }
    #[test]
    fn settings_use_config_directory_and_never_include_consent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("launch.json");
        save(&path, sample().to_string()).unwrap();
        let args = load(&path).unwrap();
        assert!(!args.iter().any(|s| s == "--replace-clipboard"));
        let own = &args[args.iter().position(|s| s == "--offer").unwrap() + 1];
        assert_eq!(
            Path::new(own),
            root.path().canonicalize().unwrap().join("own.offer")
        );
        for argv in [
            vec!["--config", path.to_str().unwrap()],
            vec![
                "--config",
                path.to_str().unwrap(),
                "--replace-clipboard",
                "--seconds",
                "100",
            ],
            vec![
                "--config",
                path.to_str().unwrap(),
                "--replace-clipboard",
                "--replace-clipboard",
            ],
        ] {
            assert!(expand(&argv.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err());
        }
        assert!(
            expand(&[
                "--config".into(),
                path.to_str().unwrap().into(),
                "--replace-clipboard".into()
            ])
            .unwrap()
            .is_some()
        );
        // A value named --config in normal mode is not treated as a new option.
        assert!(
            expand(&["--offer".into(), "--config".into()])
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn unknown_duplicate_secret_invalid_context_and_aliased_paths_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("launch.json");
        save(&path, sample().to_string()).unwrap();
        assert!(load(&path).is_ok());
        for (key, value) in [
            ("token", serde_json::json!("SECRET-MUST-NOT-APPEAR")),
            ("replace_clipboard", serde_json::json!(true)),
            ("version", serde_json::json!(2)),
            ("peer_name", serde_json::json!("../remote")),
            (
                "relay",
                serde_json::json!(
                    "wss://user:SECRET-MUST-NOT-APPEAR@relay.invalid/our-clipboard/v1"
                ),
            ),
            ("room", serde_json::json!("0".repeat(64))),
            ("ttl_ms", serde_json::json!(31_000)),
            ("seconds", serde_json::json!(20)),
            ("peer_offer", serde_json::json!("./own.offer")),
        ] {
            let mut data = sample();
            data[key] = value;
            save(&path, data.to_string()).unwrap();
            let error = load(&path).unwrap_err();
            assert!(!error.contains("SECRET-MUST-NOT-APPEAR"));
        }
        let text = sample().to_string();
        save(&path, text.replacen('{', "{\"version\":1,", 1)).unwrap();
        assert!(load(&path).is_err());
        save(&path, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(load(&path).is_err());
    }
    #[test]
    fn relay_validation_is_static_and_does_not_silently_drop_an_invalid_port() {
        for url in [
            "wss://relay.invalid/our-clipboard/v1",
            "wss://127.0.0.1:443/our-clipboard/v1",
            "wss://[::1]:8443/our-clipboard/v1",
        ] {
            assert!(outbound::validate_relay_address(url).is_ok());
        }
        for url in [
            "ws://relay.invalid/our-clipboard/v1",
            "wss://relay.invalid:65536/our-clipboard/v1",
            "wss://relay.invalid:0/our-clipboard/v1",
            "wss://relay.invalid:/our-clipboard/v1",
            "wss://relay.invalid/our-clipboard/v1?token=secret",
            "wss://relay.invalid/our-clipboard/v1#secret",
        ] {
            assert!(outbound::validate_relay_address(url).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn unsafe_local_files_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("launch.json");
        save(&path, sample().to_string()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(load(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = root.path().join("alias.json");
        symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
    }
}

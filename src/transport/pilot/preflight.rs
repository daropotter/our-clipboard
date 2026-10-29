//! Local preflight only: no transport, offer generation or clipboard backend.
use super::*;
fn check(path: &Path) -> std::result::Result<String, &'static str> {
    let mut args = config::load(path)?;
    // Pure option validation is not consent to run; this function never calls run/session.
    args.push("--replace-clipboard".into());
    let o = options(&args)?;
    platform_options(&o)?;
    if idle_requested(o.pause_file.as_deref())
        .map_err(|_| "Pause marker metadata unavailable; preflight stopped.")?
    {
        return Ok("PAUSED: local preflight skipped; no credentials, peer trust, transport or clipboard accessed.".into());
    }
    backend_options(&o)?;
    let idle = idle_requested(o.idle_file.as_deref())
        .map_err(|_| "Idle marker metadata unavailable; preflight stopped.")?;
    match std::fs::symlink_metadata(o.offer.as_ref().unwrap()) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(
                "Own public offer path already exists; choose fresh offer filenames. Nothing was removed.",
            );
        }
        Err(_) => return Err("Own public offer metadata unavailable; preflight stopped."),
    }
    let _credential = AccessToken::from_env().map_err(|_| "Relay credential missing or invalid; set OUR_CLIPBOARD_RELAY_TOKEN through an authorized secret channel.")?;
    let _roots = outbound::relay_roots(o.relay_ca.as_deref().map(Path::new))
        .map_err(|_| "Relay CA configuration invalid or inaccessible; no relay was contacted.")?;
    #[cfg(not(feature = "os-keyring"))]
    {
        let _ = idle;
        Err("Local trust preflight requires the os-keyring feature; no identity fallback.")
    }
    #[cfg(feature = "os-keyring")]
    {
        let profile = o.identity_profile.as_deref().unwrap();
        let owner = crate::identity::open_existing(profile).map_err(|e| e.message())?;
        let peer = crate::identity::load_peer(profile, o.peer_name.as_deref().unwrap(), &owner)
            .map_err(|e| e.message())?;
        Ok(format!(
            "LOCAL PREFLIGHT PASSED\nLocal device: {}\nApproved peer: {}\nLocal Idle marker: {}\nRelay credential format and CA configuration accepted locally.\nNo relay contacted, offers published/consumed or clipboard accessed. Platform/backend state, peer reachability, current sessions and delivery were not verified; this is not READY.",
            owner.id().fingerprint(),
            crate::identity::DeviceId::from_public_key(&peer).fingerprint(),
            if idle {
                "present"
            } else {
                "absent or not configured"
            }
        ))
    }
}
pub(super) fn command(args: &[String]) -> Option<std::result::Result<String, &'static str>> {
    if args.first().is_none_or(|a| a != "preflight") {
        return None;
    }
    Some(match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["preflight", "--help"] | ["preflight", "-h"] => Ok("Usage: our-clipboard pilot preflight --file FILE\nChecks local launch identity/peer approval, credential format, CA and offer/control paths. May access/unlock the OS credential service; no relay connection, offer publication or clipboard access. No --replace-clipboard consent is stored or implied.".into()),
        ["preflight", "--file", file] => check(Path::new(file)),
        _ => Err("Usage: our-clipboard pilot preflight --file FILE"),
    })
}

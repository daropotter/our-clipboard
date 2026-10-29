//! Local marker control only; never starts a pilot or accesses its credentials.
use super::*;

// Existing entries of any kind already request Pause. Never open their contents,
// follow symlinks, truncate them or remove them, even when creation races.
fn request_pause(path: &Path) -> std::result::Result<bool, &'static str> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => {
            file.sync_all().map_err(
                |_| "Could not sync Pause marker; it may already exist. Nothing was removed.",
            )?;
            #[cfg(unix)]
            File::open(path.parent().ok_or("Invalid Pause marker parent.")?)
                .and_then(|parent| parent.sync_all())
                .map_err(
                    |_| "Could not sync Pause marker directory; the marker was left in place.",
                )?;
            Ok(true)
        }
        // Windows may report an existing directory as PermissionDenied rather
        // than AlreadyExists. Metadata, not the error kind, determines the gate.
        Err(_) => match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(false),
            Err(_) => Err(
                "Could not create or confirm Pause marker. Existing entries are never replaced.",
            ),
        },
    }
}
fn marker_state(path: Option<&str>) -> std::result::Result<&'static str, &'static str> {
    match path {
        None => Ok("not configured"),
        Some(path) => idle_requested(Some(path))
            .map(|present| if present { "present" } else { "absent" })
            .map_err(|_| "Control marker metadata unavailable; state is unknown."),
    }
}
fn execute(action: &str, path: &Path) -> std::result::Result<String, &'static str> {
    let mut args = config::load(path)?;
    // Option validation only, not clipboard consent: this module never calls run/session.
    args.push("--replace-clipboard".into());
    let options = options(&args)?;
    if action == "pause" {
        let marker = options.pause_file.as_deref().ok_or(
            "Launch file has no pause_file; no marker was created. Configure it before starting a pilot.",
        )?;
        let created = request_pause(Path::new(marker))?;
        Ok(format!(
            "Pause requested: {}.\nThis is a local marker request, not acknowledgement from a running pilot. The pilot stops when it next observes the marker; restart requires fresh offers.",
            if created {
                "marker created"
            } else {
                "existing entry preserved"
            },
        ))
    } else {
        Ok(format!(
            "Local Pause marker: {}\nLocal Idle marker: {}\nLocal metadata snapshot only; process liveness, native system Idle, peer state and delivery were not checked. This is not READY or acknowledgement of Pause.",
            marker_state(options.pause_file.as_deref())?,
            marker_state(options.idle_file.as_deref())?,
        ))
    }
}
pub(super) fn command(args: &[String]) -> Option<std::result::Result<String, &'static str>> {
    let action = args.first()?.as_str();
    if action != "pause" && action != "controls" {
        return None;
    }
    Some(match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [_, "--help"] | [_, "-h"] => Ok("Usage: our-clipboard pilot pause --file FILE\n       our-clipboard pilot controls --file FILE\nPause creates only the configured pause_file marker, without replacing existing entries. Controls reads Pause/Idle marker metadata. Neither command contacts credentials, relay or clipboard, starts a session, removes markers or confirms process state.".into()),
        [_, "--file", file] => execute(action, Path::new(file)),
        _ => Err("Usage: our-clipboard pilot pause|controls --file FILE"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_create_a_private_empty_marker_and_preserve_existing_entries() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("pause");
        assert!(request_pause(&marker).unwrap());
        assert_eq!(std::fs::read(&marker).unwrap(), b"");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(&marker, b"unrelated existing data").unwrap();
        assert!(!request_pause(&marker).unwrap());
        assert_eq!(std::fs::read(&marker).unwrap(), b"unrelated existing data");
        assert!(!request_pause(root.path()).unwrap());
        assert!(request_pause(&root.path().join("missing/pause")).is_err());
        assert!(!root.path().join("missing").exists());
    }
    #[test]
    #[cfg(unix)]
    fn existing_and_dangling_symlinks_are_preserved_without_opening_targets() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let marker = root.path().join("pause");
        std::os::unix::fs::symlink(&target, &marker).unwrap();
        assert!(!request_pause(&marker).unwrap());
        assert!(!target.exists());
        std::fs::write(&target, b"preserve").unwrap();
        assert!(!request_pause(&marker).unwrap());
        assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
        assert!(
            std::fs::symlink_metadata(&marker)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    #[test]
    fn unknown_marker_state_is_not_reported_as_absent() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(marker_state(None).unwrap(), "not configured");
        assert_eq!(
            marker_state(root.path().join("pause").to_str()).unwrap(),
            "absent"
        );
        assert!(marker_state(root.path().join("missing/pause").to_str()).is_err());
    }
}

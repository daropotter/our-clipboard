//! Local, immutable peer-key approvals. No group membership or revocation protocol.
use super::{DeviceIdentity, IdentityError, storage};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
const MAGIC: &[u8; 5] = b"OCPP\x01";
const DOMAIN: &[u8] = b"our-clipboard/local-peer-pin/v1\0";
const MAX_BYTES: usize = 5 + 32 + 32 + 1 + 64 + 64;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerPinError {
    Configuration,
    InvalidName,
    Missing,
    Exists,
    Owner,
    Corrupt,
    Busy,
    InvalidPeer,
}
impl PeerPinError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Configuration => "Peer trust metadata could not be accessed safely.",
            Self::InvalidName => {
                "Peer name must contain 1-64 ASCII letters, digits, hyphens or underscores."
            }
            Self::Missing => {
                "Peer trust is missing; explicitly approve the peer with peer trust first. No automatic enrollment."
            }
            Self::Exists => "Peer name already exists. Existing trust was not replaced.",
            Self::Owner => "Peer trust does not belong to the current local device identity.",
            Self::Corrupt => {
                "Peer trust record is invalid, modified or for another name. It was not replaced."
            }
            Self::Busy => "Another process is managing this profile. Retry after it finishes.",
            Self::InvalidPeer => "Peer key must identify another device.",
        }
    }
}
pub fn validate_peer_name(name: &str) -> Result<(), PeerPinError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        Err(PeerPinError::InvalidName)
    } else {
        Ok(())
    }
}
fn encode(owner: &DeviceIdentity, name: &str, peer: [u8; 32]) -> Vec<u8> {
    let mut b = MAGIC.to_vec();
    b.extend(owner.public_key());
    b.extend(peer);
    b.push(name.len() as u8);
    b.extend(name.as_bytes());
    let mut message = DOMAIN.to_vec();
    message.extend(&b);
    b.extend(owner.sign(&message));
    b
}
fn decode(owner: &DeviceIdentity, name: &str, b: &[u8]) -> Result<[u8; 32], PeerPinError> {
    if b.len() != 134 + name.len() || b.len() > MAX_BYTES || &b[..5] != MAGIC {
        return Err(PeerPinError::Corrupt);
    }
    if b[5..37] != owner.public_key() {
        return Err(PeerPinError::Owner);
    }
    if b[69] as usize != name.len() || &b[70..b.len() - 64] != name.as_bytes() {
        return Err(PeerPinError::Corrupt);
    }
    let end = b.len() - 64;
    let mut message = DOMAIN.to_vec();
    message.extend(&b[..end]);
    DeviceIdentity::verify(
        &owner.public_key(),
        &message,
        b[end..].try_into().map_err(|_| PeerPinError::Corrupt)?,
    )
    .map_err(|_| PeerPinError::Corrupt)?;
    let peer: [u8; 32] = b[37..69].try_into().map_err(|_| PeerPinError::Corrupt)?;
    if peer == owner.public_key() || peer == [0; 32] {
        return Err(PeerPinError::InvalidPeer);
    }
    Ok(peer)
}
fn profile_lock(
    root: &Path,
    profile: &str,
    owner: &DeviceIdentity,
) -> Result<(File, PathBuf), PeerPinError> {
    let (guard, pin) = storage::lock(root, profile).map_err(|e| {
        if e == IdentityError::Busy {
            PeerPinError::Busy
        } else {
            PeerPinError::Configuration
        }
    })?;
    if storage::read_pin(&pin).map_err(|_| PeerPinError::Owner)? != Some(owner.public_key()) {
        return Err(PeerPinError::Owner);
    }
    Ok((
        guard,
        pin.parent()
            .ok_or(PeerPinError::Configuration)?
            .join("peers"),
    ))
}
fn private_directory(path: &Path, create: bool) -> Result<(), PeerPinError> {
    if create {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(PeerPinError::Configuration),
        }
    }
    let meta = fs::symlink_metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            PeerPinError::Missing
        } else {
            PeerPinError::Configuration
        }
    })?;
    if !meta.file_type().is_dir() {
        return Err(PeerPinError::Configuration);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(PeerPinError::Configuration);
        }
    }
    Ok(())
}
fn load_at(
    root: &Path,
    profile: &str,
    name: &str,
    owner: &DeviceIdentity,
) -> Result<[u8; 32], PeerPinError> {
    validate_peer_name(name)?;
    let (_guard, directory) = profile_lock(root, profile, owner)?;
    private_directory(&directory, false)?;
    let path = directory.join(format!("{name}.pin"));
    let meta = fs::symlink_metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            PeerPinError::Missing
        } else {
            PeerPinError::Configuration
        }
    })?;
    if !meta.file_type().is_file() {
        return Err(PeerPinError::Configuration);
    }
    let mut b = Vec::new();
    File::open(path)
        .map_err(|_| PeerPinError::Configuration)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut b)
        .map_err(|_| PeerPinError::Configuration)?;
    decode(owner, name, &b)
}
fn remember_at(
    root: &Path,
    profile: &str,
    name: &str,
    owner: &DeviceIdentity,
    peer: [u8; 32],
) -> Result<(), PeerPinError> {
    validate_peer_name(name)?;
    if peer == owner.public_key() || peer == [0; 32] {
        return Err(PeerPinError::InvalidPeer);
    }
    let (_guard, directory) = profile_lock(root, profile, owner)?;
    private_directory(&directory, true)?;
    let path = directory.join(format!("{name}.pin"));
    match fs::symlink_metadata(&path) {
        Ok(_) => return Err(PeerPinError::Exists),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(PeerPinError::Configuration),
    }
    let mut tmp =
        tempfile::NamedTempFile::new_in(&directory).map_err(|_| PeerPinError::Configuration)?;
    tmp.write_all(&encode(owner, name, peer))
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|_| PeerPinError::Configuration)?;
    tmp.persist_noclobber(path).map_err(|e| {
        if e.error.kind() == std::io::ErrorKind::AlreadyExists {
            PeerPinError::Exists
        } else {
            PeerPinError::Configuration
        }
    })?;
    #[cfg(unix)]
    File::open(&directory)
        .and_then(|d| d.sync_all())
        .map_err(|_| PeerPinError::Configuration)?;
    Ok(())
}
/// Load and authenticate one explicitly selected local peer approval.
#[cfg(feature = "os-keyring")]
pub fn load_peer(
    profile: &str,
    name: &str,
    owner: &DeviceIdentity,
) -> Result<[u8; 32], PeerPinError> {
    let root = storage::configuration_root().map_err(|_| PeerPinError::Configuration)?;
    load_at(&root, profile, name, owner)
}
/// Caller must obtain independent explicit approval before saving; never replaces a name.
#[cfg(feature = "os-keyring")]
pub fn remember_peer(
    profile: &str,
    name: &str,
    owner: &DeviceIdentity,
    peer: [u8; 32],
) -> Result<(), PeerPinError> {
    let root = storage::configuration_root().map_err(|_| PeerPinError::Configuration)?;
    remember_at(&root, profile, name, owner, peer)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn setup(root: &Path, owner: &DeviceIdentity) {
        let (_guard, pin) = storage::lock(root, "local").unwrap();
        let mut b = b"OCIP\x01".to_vec();
        b.extend(owner.public_key());
        fs::write(pin, b).unwrap();
    }
    #[test]
    fn immutable_named_approval_is_bound_to_owner_name_and_every_byte() {
        let root = tempfile::tempdir().unwrap();
        let owner = DeviceIdentity::generate().unwrap();
        let peer = DeviceIdentity::generate().unwrap().public_key();
        setup(root.path(), &owner);
        assert_eq!(
            load_at(root.path(), "local", "remote", &owner),
            Err(PeerPinError::Missing)
        );
        remember_at(root.path(), "local", "remote", &owner, peer).unwrap();
        assert_eq!(
            load_at(root.path(), "local", "remote", &owner).unwrap(),
            peer
        );
        let path = root.path().join("local/peers/remote.pin");
        let original = fs::read(&path).unwrap();
        assert_eq!(
            remember_at(
                root.path(),
                "local",
                "remote",
                &owner,
                DeviceIdentity::generate().unwrap().public_key()
            ),
            Err(PeerPinError::Exists)
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        for i in 0..original.len() {
            let mut bad = original.clone();
            bad[i] ^= 1;
            assert!(decode(&owner, "remote", &bad).is_err());
        }
        for n in 0..original.len() {
            assert!(decode(&owner, "remote", &original[..n]).is_err());
        }
        let mut bad = original.clone();
        bad.push(0);
        assert!(decode(&owner, "remote", &bad).is_err());
        assert!(decode(&owner, "second", &original).is_err());
        assert!(decode(&DeviceIdentity::generate().unwrap(), "remote", &original).is_err());
        fs::write(&path, b"broken").unwrap();
        assert_eq!(
            load_at(root.path(), "local", "remote", &owner),
            Err(PeerPinError::Corrupt)
        );
        assert_eq!(
            remember_at(root.path(), "local", "remote", &owner, peer),
            Err(PeerPinError::Exists)
        );
    }
    #[test]
    fn profile_lock_missing_owner_and_path_names_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let owner = DeviceIdentity::generate().unwrap();
        let peer = DeviceIdentity::generate().unwrap().public_key();
        assert_eq!(
            remember_at(root.path(), "local", "remote", &owner, peer),
            Err(PeerPinError::Owner)
        );
        setup(root.path(), &owner);
        for name in ["", "..", "../other", "a/b", "a\\b", "a.b", &"x".repeat(65)] {
            assert_eq!(
                remember_at(root.path(), "local", name, &owner, peer),
                Err(PeerPinError::InvalidName)
            );
        }
        assert_eq!(
            remember_at(root.path(), "local", "self", &owner, owner.public_key()),
            Err(PeerPinError::InvalidPeer)
        );
        let (_guard, _) = storage::lock(root.path(), "local").unwrap();
        assert_eq!(
            remember_at(root.path(), "local", "remote", &owner, peer),
            Err(PeerPinError::Busy)
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_and_exposed_directory_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let owner = DeviceIdentity::generate().unwrap();
        let peer = DeviceIdentity::generate().unwrap().public_key();
        setup(root.path(), &owner);
        let directory = root.path().join("local/peers");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            remember_at(root.path(), "local", "remote", &owner, peer),
            Err(PeerPinError::Configuration)
        );
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(root.path().join("missing"), directory.join("remote.pin")).unwrap();
        assert_eq!(
            load_at(root.path(), "local", "remote", &owner),
            Err(PeerPinError::Configuration)
        );
        assert_eq!(
            remember_at(root.path(), "local", "remote", &owner, peer),
            Err(PeerPinError::Exists)
        );
    }
}

use super::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
const SECRET_MAGIC: &[u8; 5] = b"OCSK\x01";
const PIN_MAGIC: &[u8; 5] = b"OCIP\x01";
#[cfg(feature = "os-keyring")]
const SERVICE: &str = "org.daropotter.our-clipboard";

trait SecretStore {
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, IdentityError>;
    fn save(&self, secret: &[u8]) -> Result<(), IdentityError>;
}
fn validate_profile(profile: &str) -> Result<(), IdentityError> {
    if profile.is_empty()
        || profile.len() > 64
        || !profile
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Err(IdentityError::InvalidProfile)
    } else {
        Ok(())
    }
}
fn lock(root: &Path, profile: &str) -> Result<(File, PathBuf), IdentityError> {
    validate_profile(profile)?;
    let directory = root.join(profile);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&directory)
        .map_err(|_| IdentityError::Configuration)?;
    if fs::symlink_metadata(&directory)
        .map_err(|_| IdentityError::Configuration)?
        .file_type()
        .is_symlink()
    {
        return Err(IdentityError::Configuration);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&directory)
            .map_err(|_| IdentityError::Configuration)?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(IdentityError::Configuration);
        }
    }
    let path = directory.join("identity.lock");
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(IdentityError::Configuration);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|_| IdentityError::Configuration)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => IdentityError::Busy,
        _ => IdentityError::Configuration,
    })?;
    Ok((file, directory.join("device.pub")))
}
fn read_pin(path: &Path) -> Result<Option<[u8; 32]>, IdentityError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(meta) if !meta.file_type().is_file() => return Err(IdentityError::Configuration),
        Err(_) => return Err(IdentityError::Configuration),
        _ => {}
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| IdentityError::Configuration)?
        .take(38)
        .read_to_end(&mut bytes)
        .map_err(|_| IdentityError::Configuration)?;
    if bytes.len() != 37 || &bytes[..5] != PIN_MAGIC {
        return Err(IdentityError::CorruptPin);
    }
    Ok(Some(
        bytes[5..]
            .try_into()
            .map_err(|_| IdentityError::CorruptPin)?,
    ))
}
fn write_pin(path: &Path, public: &[u8; 32]) -> Result<(), IdentityError> {
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().ok_or(IdentityError::Configuration)?)
            .map_err(|_| IdentityError::Configuration)?;
    file.write_all(PIN_MAGIC)
        .and_then(|_| file.write_all(public))
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| IdentityError::Configuration)?;
    file.persist_noclobber(path)
        .map_err(|_| IdentityError::Configuration)?;
    Ok(())
}
fn stored_identity(bytes: &[u8]) -> Result<DeviceIdentity, IdentityError> {
    if bytes.len() != 37 || &bytes[..5] != SECRET_MAGIC {
        return Err(IdentityError::CorruptSecret);
    }
    DeviceIdentity::from_seed(
        bytes[5..]
            .try_into()
            .map_err(|_| IdentityError::CorruptSecret)?,
    )
}
fn load(
    store: &impl SecretStore,
    root: &Path,
    profile: &str,
    initialize: bool,
) -> Result<DeviceIdentity, IdentityError> {
    let (_guard, path) = lock(root, profile)?;
    let pin = read_pin(&path)?;
    if let Some(secret) = store.load()? {
        let identity = stored_identity(&secret)?;
        match pin {
            Some(public) if public != identity.public_key() => {
                return Err(IdentityError::PinMismatch);
            }
            None if initialize => write_pin(&path, &identity.public_key())?,
            None => return Err(IdentityError::Missing),
            _ => {}
        }
        return Ok(identity);
    }
    if !initialize || pin.is_some() {
        return Err(IdentityError::Missing);
    }
    let mut seed = Zeroizing::new([0; 32]);
    getrandom::fill(seed.as_mut()).map_err(|_| IdentityError::StorageFailed)?;
    let identity = DeviceIdentity::from_seed(&seed)?;
    let mut encoded = Zeroizing::new(SECRET_MAGIC.to_vec());
    encoded.extend_from_slice(seed.as_ref());
    store.save(&encoded)?;
    let saved = store.load()?.ok_or(IdentityError::StorageFailed)?;
    if stored_identity(&saved)?.public_key() != identity.public_key() {
        return Err(IdentityError::StorageFailed);
    }
    write_pin(&path, &identity.public_key())?;
    Ok(identity)
}

#[cfg(feature = "os-keyring")]
struct PlatformStore(keyring::Entry);
#[cfg(feature = "os-keyring")]
impl SecretStore for PlatformStore {
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, IdentityError> {
        match self.0.get_secret() {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(IdentityError::Unavailable),
        }
    }
    fn save(&self, secret: &[u8]) -> Result<(), IdentityError> {
        self.0
            .set_secret(secret)
            .map_err(|_| IdentityError::StorageFailed)
    }
}
#[cfg(feature = "os-keyring")]
pub fn manage(action: &str, profile: &str) -> Result<String, IdentityError> {
    validate_profile(profile)?;
    let initialize = match action {
        "init" => true,
        "show" => false,
        _ => return Err(IdentityError::Configuration),
    };
    let identity = platform_identity(profile, initialize)?;
    Ok(format!(
        "Client device identity: {}\nStorage: system credential store",
        identity.id().fingerprint()
    ))
}

/// Load an existing profile only: never initializes, rotates or repairs its pin.
#[cfg(feature = "os-keyring")]
pub fn open_existing(profile: &str) -> Result<DeviceIdentity, IdentityError> {
    platform_identity(profile, false)
}
#[cfg(feature = "os-keyring")]
fn platform_identity(profile: &str, initialize: bool) -> Result<DeviceIdentity, IdentityError> {
    validate_profile(profile)?;
    let root = if let Some(directory) = std::env::var_os("OUR_CLIPBOARD_CONFIG_DIR") {
        PathBuf::from(directory)
    } else {
        directories::ProjectDirs::from("org", "daropotter", "our-clipboard")
            .ok_or(IdentityError::Configuration)?
            .config_dir()
            .to_path_buf()
    };
    let entry = keyring::Entry::new(SERVICE, &format!("client-v1:{profile}"))
        .map_err(|_| IdentityError::Unavailable)?;
    load(&PlatformStore(entry), &root, profile, initialize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    #[derive(Default)]
    struct Memory {
        secret: RefCell<Option<Zeroizing<Vec<u8>>>>,
        writes: Cell<usize>,
        unavailable: Cell<bool>,
    }
    impl SecretStore for Memory {
        fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, IdentityError> {
            if self.unavailable.get() {
                Err(IdentityError::Unavailable)
            } else {
                Ok(self
                    .secret
                    .borrow()
                    .as_ref()
                    .map(|s| Zeroizing::new(s.to_vec())))
            }
        }
        fn save(&self, secret: &[u8]) -> Result<(), IdentityError> {
            self.writes.set(self.writes.get() + 1);
            self.secret.replace(Some(Zeroizing::new(secret.to_vec())));
            Ok(())
        }
    }
    #[test]
    fn opening_a_profile_never_creates_a_key_or_recovers_a_missing_pin() {
        let root = tempfile::tempdir().unwrap();
        let store = Memory::default();
        assert!(matches!(
            load(&store, root.path(), "pilot", false),
            Err(IdentityError::Missing)
        ));
        assert_eq!(store.writes.get(), 0);
        assert!(!root.path().join("pilot/device.pub").exists());
        let initialized = tempfile::tempdir().unwrap();
        load(&store, initialized.path(), "pilot", true).unwrap();
        assert!(matches!(
            load(&store, root.path(), "pilot", false),
            Err(IdentityError::Missing)
        ));
        assert_eq!(store.writes.get(), 1);
        assert!(!root.path().join("pilot/device.pub").exists());
    }
    #[test]
    fn initializes_once_and_reloads_the_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Memory::default();
        let a = load(&store, dir.path(), "default", true).unwrap();
        let b = load(&store, dir.path(), "default", false).unwrap();
        let c = load(&store, dir.path(), "default", true).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(b.id(), c.id());
        assert_eq!(store.writes.get(), 1);
        assert_eq!(
            std::fs::metadata(dir.path().join("default/device.pub"))
                .unwrap()
                .len(),
            37
        );
    }
    #[test]
    fn missing_or_unavailable_credentials_never_replace_a_pinned_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Memory::default();
        load(&store, dir.path(), "default", true).unwrap();
        store.secret.replace(None);
        assert!(matches!(
            load(&store, dir.path(), "default", true),
            Err(IdentityError::Missing)
        ));
        store.unavailable.set(true);
        assert!(matches!(
            load(&store, dir.path(), "default", true),
            Err(IdentityError::Unavailable)
        ));
        assert_eq!(store.writes.get(), 1);
    }
    #[test]
    fn corrupt_secret_or_public_pin_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Memory::default();
        load(&store, dir.path(), "default", true).unwrap();
        store.secret.replace(Some(Zeroizing::new(vec![0; 31])));
        assert!(matches!(
            load(&store, dir.path(), "default", true),
            Err(IdentityError::CorruptSecret)
        ));
        std::fs::write(dir.path().join("default/device.pub"), b"invalid").unwrap();
        assert!(matches!(
            load(&store, dir.path(), "default", true),
            Err(IdentityError::CorruptPin)
        ));
        assert_eq!(store.writes.get(), 1);
    }
    #[test]
    fn existing_credential_recovers_missing_pin_without_key_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let store = Memory::default();
        let a = load(&store, dir.path(), "default", true).unwrap();
        let b = load(&store, other.path(), "default", true).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(store.writes.get(), 1);
    }
    #[test]
    fn concurrent_profile_access_is_rejected_before_storage_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Memory::default();
        let (_lock, _) = lock(dir.path(), "default").unwrap();
        assert!(matches!(
            load(&store, dir.path(), "default", true),
            Err(IdentityError::Busy)
        ));
        assert_eq!(store.writes.get(), 0);
        assert!(load(&store, dir.path(), "../other", true).is_err());
    }
    #[test]
    fn replacement_key_is_detected_without_overwriting_it() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let store = Memory::default();
        load(&store, dir.path(), "default", true).unwrap();
        let replacement = Memory::default();
        load(&replacement, other.path(), "default", true).unwrap();
        store.secret.replace(replacement.secret.borrow_mut().take());
        assert!(matches!(
            load(&store, dir.path(), "default", true),
            Err(IdentityError::PinMismatch)
        ));
        assert_eq!(store.writes.get(), 1);
    }
}

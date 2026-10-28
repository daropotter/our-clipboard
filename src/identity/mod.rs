//! Persistent client signing identity. Private key material is never Debug/Display/exported.
use ring::{
    digest,
    signature::{self, Ed25519KeyPair, KeyPair},
};
use zeroize::Zeroizing;
#[cfg(any(feature = "os-keyring", test))]
mod storage;
#[cfg(feature = "os-keyring")]
pub use storage::{manage, open_existing};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    Unavailable,
    StorageFailed,
    Missing,
    CorruptSecret,
    CorruptPin,
    PinMismatch,
    Busy,
    Configuration,
    InvalidProfile,
    InvalidSignature,
}
impl IdentityError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "System credential store unavailable or locked. No replacement identity was created."
            }
            Self::StorageFailed => "System credential store could not save or verify the identity.",
            Self::Missing => {
                "Identity is missing. Initialize a new profile explicitly or recover its existing credential."
            }
            Self::CorruptSecret => "Stored identity is invalid. It was not replaced.",
            Self::CorruptPin => "Saved public identity metadata is invalid. It was not replaced.",
            Self::PinMismatch => {
                "Stored private identity does not match its saved public key. It was not replaced."
            }
            Self::Busy => {
                "Another process is managing this identity profile. Retry after it finishes."
            }
            Self::Configuration => "Identity configuration could not be accessed safely.",
            Self::InvalidProfile => {
                "Identity profile must contain 1-64 ASCII letters, digits, hyphens or underscores."
            }
            Self::InvalidSignature => "Ed25519 signature verification failed.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DeviceId(pub [u8; 32]);
impl DeviceId {
    pub fn from_public_key(public: &[u8; 32]) -> Self {
        let mut hash = digest::Context::new(&digest::SHA256);
        hash.update(b"our-clipboard/device-id/v1\0");
        hash.update(public);
        Self(
            hash.finish()
                .as_ref()
                .try_into()
                .expect("SHA256 has fixed size"),
        )
    }
    pub fn fingerprint(self) -> String {
        use std::fmt::Write;
        let mut text = String::from("ed25519-sha256-v1:");
        for byte in self.0 {
            write!(text, "{byte:02x}").expect("String write cannot fail");
        }
        text
    }
}

/// No Debug/Clone or seed export. Signing-key bytes remain in the identity/store.
pub struct DeviceIdentity {
    key: Ed25519KeyPair,
}
impl DeviceIdentity {
    pub fn generate() -> Result<Self, IdentityError> {
        let mut seed = Zeroizing::new([0; 32]);
        getrandom::fill(seed.as_mut()).map_err(|_| IdentityError::StorageFailed)?;
        Self::from_seed(&seed)
    }
    fn from_seed(seed: &[u8; 32]) -> Result<Self, IdentityError> {
        Ed25519KeyPair::from_seed_unchecked(seed)
            .map(|key| Self { key })
            .map_err(|_| IdentityError::CorruptSecret)
    }
    pub fn public_key(&self) -> [u8; 32] {
        self.key
            .public_key()
            .as_ref()
            .try_into()
            .expect("Ed25519 public key has fixed size")
    }
    pub fn id(&self) -> DeviceId {
        DeviceId::from_public_key(&self.public_key())
    }
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key
            .sign(message)
            .as_ref()
            .try_into()
            .expect("Ed25519 signature has fixed size")
    }
    pub fn verify(
        public: &[u8; 32],
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), IdentityError> {
        signature::UnparsedPublicKey::new(&signature::ED25519, public)
            .verify(message, signature)
            .map_err(|_| IdentityError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_bind_message_and_device() {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let signature = a.sign(b"synthetic message");
        assert!(DeviceIdentity::verify(&a.public_key(), b"synthetic message", &signature).is_ok());
        assert!(DeviceIdentity::verify(&b.public_key(), b"synthetic message", &signature).is_err());
        assert!(DeviceIdentity::verify(&a.public_key(), b"modified message", &signature).is_err());
        assert_ne!(a.id(), b.id());
    }
}

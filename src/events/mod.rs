//! Canonical immutable signed text envelopes. Authorization and time policy are separate.
use crate::{
    identity::{DeviceId, DeviceIdentity},
    text,
};
use ring::digest;
mod replay;
pub use replay::{ReplayDecision, ReplayGuard, SessionBinding};
const MAGIC: &[u8; 5] = b"OCSE\x01";
const DOMAIN: &[u8] = b"our-clipboard/signed-text-event/v1\0";
const HEADER: usize = 158;
const SIGNATURE: usize = 64;
pub const DEFAULT_TTL_MS: u32 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    InvalidEncoding,
    InvalidContext,
    InvalidText,
    InvalidSignature,
    Entropy,
    UnknownSession,
    MembershipMismatch,
    RetiredSession,
    ConflictingEvent,
    ClockNotAdvanced,
    Capacity,
}
impl EventError {
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidEncoding => "Signed event encoding is invalid or unsupported.",
            Self::InvalidContext => "Signed event context is invalid.",
            Self::InvalidText => "Signed event text is empty, invalid or exceeds its limit.",
            Self::InvalidSignature => "Signed event does not match the trusted source key.",
            Self::Entropy => "Could not create a new event session identifier.",
            Self::UnknownSession => "Signed event has no activated authenticated session.",
            Self::MembershipMismatch => "Signed event uses a different membership context.",
            Self::RetiredSession => "Signed event belongs to an inactive session.",
            Self::ConflictingEvent => "The same event identifier has conflicting signed contents.",
            Self::ClockNotAdvanced => "Source logical clock did not advance.",
            Self::Capacity => "Replay protection is full; no existing watermark was evicted.",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub [u8; 16]);
impl SessionId {
    pub fn random() -> Result<Self, EventError> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|_| EventError::Entropy)?;
        Ok(Self(bytes))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventContext {
    pub group: [u8; 32],
    pub membership: [u8; 32],
    pub session: SessionId,
    pub sequence: u64,
    pub lamport: u64,
    pub created_unix_ms: u64,
    pub created_boot_ms: u64,
    pub ttl_ms: u32,
}
impl EventContext {
    fn validate(&self) -> Result<(), EventError> {
        if self.sequence == 0
            || self.lamport == 0
            || self.ttl_ms == 0
            || self
                .created_boot_ms
                .checked_add(u64::from(self.ttl_ms))
                .is_none()
            || self
                .created_unix_ms
                .checked_add(u64::from(self.ttl_ms))
                .is_none()
        {
            Err(EventError::InvalidContext)
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventKey {
    pub group: [u8; 32],
    pub source: DeviceId,
    pub session: SessionId,
    pub sequence: u64,
}

/// No Debug or mutable access: forwarding must preserve this envelope and its signature.
pub struct SignedTextEvent {
    context: EventContext,
    source: DeviceId,
    text: String,
    signature: [u8; 64],
}
impl SignedTextEvent {
    pub fn sign(
        context: EventContext,
        text: &str,
        identity: &DeviceIdentity,
    ) -> Result<Self, EventError> {
        context.validate()?;
        text::validate(text).map_err(|_| EventError::InvalidText)?;
        let mut event = Self {
            context,
            source: identity.id(),
            text: text.into(),
            signature: [0; 64],
        };
        event.signature = identity.sign(&event.signing_bytes());
        Ok(event)
    }
    pub fn context(&self) -> EventContext {
        self.context
    }
    pub fn source(&self) -> DeviceId {
        self.source
    }
    pub fn key(&self) -> EventKey {
        EventKey {
            group: self.context.group,
            source: self.source,
            session: self.context.session,
            sequence: self.context.sequence,
        }
    }
    fn header(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER);
        bytes.extend(MAGIC);
        bytes.push(1); // Plain UTF-8 text.
        bytes.extend(self.context.group);
        bytes.extend(self.context.membership);
        bytes.extend(self.source.0);
        bytes.extend(self.context.session.0);
        bytes.extend(self.context.sequence.to_be_bytes());
        bytes.extend(self.context.lamport.to_be_bytes());
        bytes.extend(self.context.created_unix_ms.to_be_bytes());
        bytes.extend(self.context.created_boot_ms.to_be_bytes());
        bytes.extend(self.context.ttl_ms.to_be_bytes());
        bytes.extend((self.text.len() as u32).to_be_bytes());
        bytes
    }
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(DOMAIN.len() + HEADER + self.text.len());
        bytes.extend(DOMAIN);
        bytes.extend(self.header());
        bytes.extend(self.text.as_bytes());
        bytes
    }
    pub fn to_wire(&self) -> Vec<u8> {
        let mut bytes = self.header();
        bytes.reserve(self.text.len() + SIGNATURE);
        bytes.extend(self.text.as_bytes());
        bytes.extend(self.signature);
        bytes
    }
    pub fn envelope_hash(&self) -> [u8; 32] {
        let mut hash = digest::Context::new(&digest::SHA256);
        hash.update(&self.header());
        hash.update(self.text.as_bytes());
        hash.update(&self.signature);
        hash.finish()
            .as_ref()
            .try_into()
            .expect("SHA256 has fixed size")
    }
    pub fn from_wire(bytes: &[u8]) -> Result<Self, EventError> {
        if bytes.len() < HEADER + SIGNATURE || &bytes[..5] != MAGIC || bytes[5] != 1 {
            return Err(EventError::InvalidEncoding);
        }
        let mut reader = Reader { bytes, position: 6 };
        let group = reader.take()?;
        let membership = reader.take()?;
        let source = DeviceId(reader.take()?);
        let session = SessionId(reader.take()?);
        let sequence = u64::from_be_bytes(reader.take()?);
        let lamport = u64::from_be_bytes(reader.take()?);
        let created_unix_ms = u64::from_be_bytes(reader.take()?);
        let created_boot_ms = u64::from_be_bytes(reader.take()?);
        let ttl_ms = u32::from_be_bytes(reader.take()?);
        let length = u32::from_be_bytes(reader.take()?) as usize;
        if length == 0
            || length > text::MAX_TEXT_BYTES
            || bytes.len() != HEADER + length + SIGNATURE
        {
            return Err(EventError::InvalidEncoding);
        }
        let context = EventContext {
            group,
            membership,
            session,
            sequence,
            lamport,
            created_unix_ms,
            created_boot_ms,
            ttl_ms,
        };
        context.validate()?;
        let text = std::str::from_utf8(&bytes[HEADER..HEADER + length])
            .map_err(|_| EventError::InvalidText)?;
        text::validate(text).map_err(|_| EventError::InvalidText)?;
        let signature = bytes[HEADER + length..]
            .try_into()
            .map_err(|_| EventError::InvalidEncoding)?;
        Ok(Self {
            context,
            source,
            text: text.into(),
            signature,
        })
    }
    /// The key must come from already trusted membership, never an untrusted envelope.
    pub fn verify_with_trusted_key(
        &self,
        trusted_key: &[u8; 32],
    ) -> Result<VerifiedTextEvent<'_>, EventError> {
        if DeviceId::from_public_key(trusted_key) != self.source {
            return Err(EventError::InvalidSignature);
        }
        DeviceIdentity::verify(trusted_key, &self.signing_bytes(), &self.signature)
            .map_err(|_| EventError::InvalidSignature)?;
        Ok(VerifiedTextEvent(self))
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], EventError> {
        let end = self
            .position
            .checked_add(N)
            .ok_or(EventError::InvalidEncoding)?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or(EventError::InvalidEncoding)?
            .try_into()
            .map_err(|_| EventError::InvalidEncoding)?;
        self.position = end;
        Ok(result)
    }
}
/// Only signature verification can construct this handle; it does not prove freshness.
pub struct VerifiedTextEvent<'a>(&'a SignedTextEvent);
impl VerifiedTextEvent<'_> {
    pub fn context(&self) -> EventContext {
        self.0.context
    }
    pub fn source(&self) -> DeviceId {
        self.0.source
    }
    pub fn key(&self) -> EventKey {
        self.0.key()
    }
    pub fn text(&self) -> &str {
        &self.0.text
    }
    pub fn envelope_hash(&self) -> [u8; 32] {
        self.0.envelope_hash()
    }
    pub fn order_key(&self) -> (u64, DeviceId) {
        (self.0.context.lamport, self.0.source)
    }
}

pub fn probe() -> Result<String, EventError> {
    let identity = DeviceIdentity::generate().map_err(|_| EventError::Entropy)?;
    let context = EventContext {
        group: [1; 32],
        membership: [2; 32],
        session: SessionId::random()?,
        sequence: 1,
        lamport: 1,
        created_unix_ms: 1_000,
        created_boot_ms: 1_000,
        ttl_ms: DEFAULT_TTL_MS,
    };
    let event = SignedTextEvent::sign(context, "Synthetic signed text 🦀\nSecond line", &identity)?;
    let encoded = event.to_wire();
    let decoded = SignedTextEvent::from_wire(&encoded)?;
    let verified = decoded.verify_with_trusted_key(&identity.public_key())?;
    let mut guard = ReplayGuard::new(1);
    guard.activate_authenticated_session(SessionBinding {
        group: context.group,
        membership: context.membership,
        source: identity.id(),
        session: context.session,
    })?;
    if guard.observe(&verified)? != ReplayDecision::New
        || guard.observe(&verified)? != ReplayDecision::Duplicate
    {
        return Err(EventError::ConflictingEvent);
    }
    let mut altered = encoded;
    altered[HEADER] ^= 1;
    if SignedTextEvent::from_wire(&altered)?
        .verify_with_trusted_key(&identity.public_key())
        .is_ok()
    {
        return Err(EventError::InvalidSignature);
    }
    Ok("Synthetic signed envelope round trip passed. Tampering and duplicate delivery rejected; membership and freshness must be checked separately.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn context() -> EventContext {
        EventContext {
            group: [1; 32],
            membership: [2; 32],
            session: SessionId([3; 16]),
            sequence: 1,
            lamport: 1,
            created_unix_ms: 1000,
            created_boot_ms: 1000,
            ttl_ms: 30_000,
        }
    }
    #[test]
    fn forwarding_preserves_canonical_bytes_and_all_signed_fields() {
        let identity = DeviceIdentity::generate().unwrap();
        let event = SignedTextEvent::sign(context(), " 🦀\r\nsecond line\t", &identity).unwrap();
        assert_eq!(event.header().len(), HEADER);
        let bytes = event.to_wire();
        let decoded = SignedTextEvent::from_wire(&bytes).unwrap();
        assert!(decoded.to_wire() == bytes);
        assert_eq!(event.envelope_hash(), decoded.envelope_hash());
        assert!(
            decoded
                .verify_with_trusted_key(&identity.public_key())
                .is_ok()
        );
        // A mutation in any header, text or signature byte must not verify.
        for index in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[index] ^= 1;
            if let Ok(decoded) = SignedTextEvent::from_wire(&changed) {
                assert!(
                    decoded
                        .verify_with_trusted_key(&identity.public_key())
                        .is_err()
                );
            }
        }
    }
    #[test]
    fn limits_and_noncanonical_encodings_are_rejected_before_copying_text() {
        let identity = DeviceIdentity::generate().unwrap();
        let event = SignedTextEvent::sign(context(), "synthetic", &identity).unwrap();
        let mut bytes = event.to_wire();
        bytes.push(0);
        assert!(SignedTextEvent::from_wire(&bytes).is_err());
        let mut bytes = event.to_wire();
        bytes[HEADER - 4..HEADER].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(SignedTextEvent::from_wire(&bytes).is_err());
        for length in 0..HEADER + SIGNATURE {
            assert!(SignedTextEvent::from_wire(&vec![0; length]).is_err());
        }
        assert!(SignedTextEvent::sign(context(), "bad\0text", &identity).is_err());
    }
    #[test]
    fn identical_text_new_sequence_is_a_distinct_event() {
        let identity = DeviceIdentity::generate().unwrap();
        let a = SignedTextEvent::sign(context(), "same", &identity).unwrap();
        let b = SignedTextEvent::sign(
            EventContext {
                sequence: 2,
                lamport: 2,
                ..context()
            },
            "same",
            &identity,
        )
        .unwrap();
        assert_ne!(a.key(), b.key());
        assert_ne!(a.envelope_hash(), b.envelope_hash());
        assert!(
            a.verify_with_trusted_key(&DeviceIdentity::generate().unwrap().public_key())
                .is_err()
        );
    }
}

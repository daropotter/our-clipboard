//! End-to-end application receipts. Membership and expected receivers come from authenticated state.
use crate::{
    events::{EventKey, SessionId, VerifiedTextEvent},
    freshness::DeliveryDecision,
    identity::{DeviceId, DeviceIdentity},
};
const MAGIC: &[u8; 5] = b"OCAR\x01";
const DOMAIN: &[u8] = b"our-clipboard/application-receipt/v1\0";
pub const RECEIPT_BYTES: usize = 270;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptError {
    Encoding,
    NotApplied,
    WrongContext,
    Signature,
    Capacity,
    DuplicateTarget,
    UnknownTarget,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptContext {
    pub event: EventKey,
    pub membership: [u8; 32],
    pub envelope_hash: [u8; 32],
    pub receiver: DeviceId,
    pub receiver_session: SessionId,
}
/// Immutable, domain-separated receiver signature. No text or private keys.
pub struct SignedApplicationReceipt {
    context: ReceiptContext,
    signature: [u8; 64],
}
impl SignedApplicationReceipt {
    /// The decision must come from actual completed application; the signer remains responsible
    /// for truthful reporting. A signature cannot prove an honest OS or an honest receiver.
    pub fn sign_applied(
        event: &VerifiedTextEvent<'_>,
        decision: DeliveryDecision,
        receiver: &DeviceIdentity,
        session: SessionId,
    ) -> Result<Self, ReceiptError> {
        let hash = match decision {
            DeliveryDecision::Applied { envelope_hash }
            | DeliveryDecision::AlreadyApplied { envelope_hash } => envelope_hash,
            _ => return Err(ReceiptError::NotApplied),
        };
        if hash != event.envelope_hash() {
            return Err(ReceiptError::WrongContext);
        }
        let context = ReceiptContext {
            event: event.key(),
            membership: event.context().membership,
            envelope_hash: hash,
            receiver: receiver.id(),
            receiver_session: session,
        };
        let mut receipt = Self {
            context,
            signature: [0; 64],
        };
        receipt.signature = receiver.sign(&receipt.signing_bytes());
        Ok(receipt)
    }
    pub fn context(&self) -> ReceiptContext {
        self.context
    }
    fn unsigned(&self) -> Vec<u8> {
        let c = self.context;
        let mut bytes = MAGIC.to_vec();
        bytes.extend(c.event.group);
        bytes.extend(c.membership);
        bytes.extend(c.event.source.0);
        bytes.extend(c.event.session.0);
        bytes.extend(c.event.sequence.to_be_bytes());
        bytes.extend(c.envelope_hash);
        bytes.extend(c.receiver.0);
        bytes.extend(c.receiver_session.0);
        bytes.push(1);
        bytes
    }
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = DOMAIN.to_vec();
        bytes.extend(self.unsigned());
        bytes
    }
    pub fn to_wire(&self) -> Vec<u8> {
        let mut bytes = self.unsigned();
        bytes.extend(self.signature);
        bytes
    }
    pub fn from_wire(bytes: &[u8]) -> Result<Self, ReceiptError> {
        if bytes.len() != RECEIPT_BYTES || &bytes[..5] != MAGIC {
            return Err(ReceiptError::Encoding);
        }
        let mut r = Reader { bytes, offset: 5 };
        let group = r.take()?;
        let membership = r.take()?;
        let source = DeviceId(r.take()?);
        let session = SessionId(r.take()?);
        let sequence = u64::from_be_bytes(r.take()?);
        let envelope_hash = r.take()?;
        let receiver = DeviceId(r.take()?);
        let receiver_session = SessionId(r.take()?);
        if r.take::<1>()? != [1] || sequence == 0 {
            return Err(ReceiptError::Encoding);
        }
        Ok(Self {
            context: ReceiptContext {
                event: EventKey {
                    group,
                    source,
                    session,
                    sequence,
                },
                membership,
                envelope_hash,
                receiver,
                receiver_session,
            },
            signature: r.take()?,
        })
    }
    pub fn verify_with_trusted_key(
        &self,
        key: &[u8; 32],
    ) -> Result<VerifiedApplicationReceipt<'_>, ReceiptError> {
        if DeviceId::from_public_key(key) != self.context.receiver {
            return Err(ReceiptError::WrongContext);
        }
        DeviceIdentity::verify(key, &self.signing_bytes(), &self.signature)
            .map_err(|_| ReceiptError::Signature)?;
        Ok(VerifiedApplicationReceipt(self))
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], ReceiptError> {
        let end = self.offset.checked_add(N).ok_or(ReceiptError::Encoding)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(ReceiptError::Encoding)?
            .try_into()
            .map_err(|_| ReceiptError::Encoding)?;
        self.offset = end;
        Ok(value)
    }
}
pub struct VerifiedApplicationReceipt<'a>(&'a SignedApplicationReceipt);
impl VerifiedApplicationReceipt<'_> {
    pub fn context(&self) -> ReceiptContext {
        self.0.context
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedReceiver {
    pub device: DeviceId,
    pub session: SessionId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptDecision {
    New,
    Duplicate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryProgress {
    pub expected: usize,
    pub applied: usize,
}
impl DeliveryProgress {
    pub fn complete(self) -> bool {
        self.expected == self.applied
    }
}
/// One bounded event's recipient ledger, populated only from authenticated membership/readiness.
/// No timeout marks a target applied, offline or Idle. Missing ACKs stay unconfirmed.
/// Late receipts can be recorded after text TTL expires; forwarding never changes signed bytes.
pub struct DeliveryTracker {
    event: EventKey,
    membership: [u8; 32],
    hash: [u8; 32],
    targets: Vec<(ExpectedReceiver, bool)>,
}
impl DeliveryTracker {
    pub fn new(
        event: &VerifiedTextEvent<'_>,
        targets: &[ExpectedReceiver],
    ) -> Result<Self, ReceiptError> {
        if targets.is_empty() || targets.len() > 1024 {
            return Err(ReceiptError::Capacity);
        }
        let mut seen = std::collections::HashSet::new();
        for target in targets {
            if !seen.insert(target.device) {
                return Err(ReceiptError::DuplicateTarget);
            }
        }
        Ok(Self {
            event: event.key(),
            membership: event.context().membership,
            hash: event.envelope_hash(),
            targets: targets.iter().copied().map(|t| (t, false)).collect(),
        })
    }
    pub fn observe(
        &mut self,
        receipt: &VerifiedApplicationReceipt<'_>,
    ) -> Result<ReceiptDecision, ReceiptError> {
        let c = receipt.context();
        if c.event != self.event || c.membership != self.membership || c.envelope_hash != self.hash
        {
            return Err(ReceiptError::WrongContext);
        }
        let (target, applied) = self
            .targets
            .iter_mut()
            .find(|(t, _)| t.device == c.receiver)
            .ok_or(ReceiptError::UnknownTarget)?;
        if target.session != c.receiver_session {
            return Err(ReceiptError::WrongContext);
        }
        if *applied {
            return Ok(ReceiptDecision::Duplicate);
        }
        *applied = true;
        Ok(ReceiptDecision::New)
    }
    pub fn progress(&self) -> DeliveryProgress {
        DeliveryProgress {
            expected: self.targets.len(),
            applied: self.targets.iter().filter(|(_, a)| *a).count(),
        }
    }
    pub fn is_applied(&self, device: DeviceId) -> Option<bool> {
        self.targets
            .iter()
            .find(|(t, _)| t.device == device)
            .map(|(_, a)| *a)
    }
}
#[cfg(test)]
mod tests;

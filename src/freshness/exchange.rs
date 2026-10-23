use super::*;
const REQUEST_MAGIC: &[u8; 5] = b"OCTR\x01";
const REPLY_MAGIC: &[u8; 5] = b"OCTS\x01";
const REQUEST_SIZE: usize = 197;
const REPLY_SIZE: usize = 282;
const DOMAIN: &[u8] = b"our-clipboard/clock-reply/v1\0";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockBinding {
    pub group: [u8; 32],
    pub membership: [u8; 32],
    pub source: DeviceId,
    pub source_session: SessionId,
    pub receiver: DeviceId,
    pub receiver_session: SessionId,
}
#[derive(Clone, PartialEq, Eq)]
pub struct ClockRequest {
    binding: ClockBinding,
    challenge: [u8; 32],
}
impl ClockRequest {
    pub fn binding(&self) -> ClockBinding {
        self.binding
    }
    pub fn to_wire(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(REQUEST_SIZE);
        bytes.extend(REQUEST_MAGIC);
        bytes.extend(self.binding.group);
        bytes.extend(self.binding.membership);
        bytes.extend(self.binding.source.0);
        bytes.extend(self.binding.source_session.0);
        bytes.extend(self.binding.receiver.0);
        bytes.extend(self.binding.receiver_session.0);
        bytes.extend(self.challenge);
        bytes
    }
    pub fn from_wire(bytes: &[u8]) -> Result<Self, ClockError> {
        if bytes.len() != REQUEST_SIZE || &bytes[..5] != REQUEST_MAGIC {
            return Err(ClockError::Encoding);
        }
        let mut reader = Reader { bytes, offset: 5 };
        Ok(Self {
            binding: ClockBinding {
                group: reader.take()?,
                membership: reader.take()?,
                source: DeviceId(reader.take()?),
                source_session: SessionId(reader.take()?),
                receiver: DeviceId(reader.take()?),
                receiver_session: SessionId(reader.take()?),
            },
            challenge: reader.take()?,
        })
    }
}
/// An unpredictable challenge is consumed once; old replies cannot refresh an estimate.
pub struct ClockProbe {
    request: ClockRequest,
    local_sent_ms: u64,
}
impl ClockProbe {
    pub fn start(binding: ClockBinding, local_sent_ms: u64) -> Result<Self, ClockError> {
        let mut challenge = [0; 32];
        getrandom::fill(&mut challenge).map_err(|_| ClockError::Entropy)?;
        Ok(Self {
            request: ClockRequest { binding, challenge },
            local_sent_ms,
        })
    }
    pub fn request(&self) -> &ClockRequest {
        &self.request
    }
    pub fn finish(
        self,
        reply: &ClockReply,
        trusted_source: &[u8; 32],
        local_received_ms: u64,
        policy: ClockPolicy,
    ) -> Result<ClockEstimate, ClockError> {
        if reply.request != self.request
            || DeviceId::from_public_key(trusted_source) != self.request.binding.source
        {
            return Err(ClockError::WrongContext);
        }
        DeviceIdentity::verify(trusted_source, &reply.signing_bytes(), &reply.signature)
            .map_err(|_| ClockError::Unauthenticated)?;
        let rtt = local_received_ms
            .checked_sub(self.local_sent_ms)
            .ok_or(ClockError::ClockRegressed)?;
        if rtt > policy.max_rtt_ms || reply.sent_ms < reply.received_ms {
            return Err(ClockError::InvalidSample);
        }
        let margin = i128::from(policy.granularity_ms) * 2;
        let lower = i128::from(reply.sent_ms) - i128::from(local_received_ms) - margin;
        let upper = i128::from(reply.received_ms) - i128::from(self.local_sent_ms) + margin;
        if lower > upper {
            return Err(ClockError::InvalidSample);
        }
        Ok(ClockEstimate {
            binding: self.request.binding,
            sampled_at_ms: local_received_ms,
            source_sent_ms: reply.sent_ms,
            rtt_ms: rtt,
            offset_lower: lower,
            offset_upper: upper,
            policy,
        })
    }
}
/// Signed clock metadata contains no clipboard text and is not membership authorization.
pub struct ClockReply {
    request: ClockRequest,
    received_ms: u64,
    sent_ms: u64,
    signature: [u8; 64],
}
impl ClockReply {
    /// Call only after authenticating the request's membership/session at the transport layer.
    pub fn sign(
        request: &ClockRequest,
        source: &DeviceIdentity,
        received_ms: u64,
        sent_ms: u64,
    ) -> Result<Self, ClockError> {
        if source.id() != request.binding.source || sent_ms < received_ms {
            return Err(ClockError::InvalidSample);
        }
        let mut reply = Self {
            request: request.clone(),
            received_ms,
            sent_ms,
            signature: [0; 64],
        };
        reply.signature = source.sign(&reply.signing_bytes());
        Ok(reply)
    }
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = DOMAIN.to_vec();
        bytes.extend(self.request.to_wire());
        bytes.extend(self.received_ms.to_be_bytes());
        bytes.extend(self.sent_ms.to_be_bytes());
        bytes
    }
    pub fn to_wire(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(REPLY_SIZE);
        bytes.extend(REPLY_MAGIC);
        bytes.extend(self.request.to_wire());
        bytes.extend(self.received_ms.to_be_bytes());
        bytes.extend(self.sent_ms.to_be_bytes());
        bytes.extend(self.signature);
        bytes
    }
    pub fn from_wire(bytes: &[u8]) -> Result<Self, ClockError> {
        if bytes.len() != REPLY_SIZE || &bytes[..5] != REPLY_MAGIC {
            return Err(ClockError::Encoding);
        }
        let request = ClockRequest::from_wire(&bytes[5..5 + REQUEST_SIZE])?;
        let mut reader = Reader {
            bytes,
            offset: 5 + REQUEST_SIZE,
        };
        Ok(Self {
            request,
            received_ms: u64::from_be_bytes(reader.take()?),
            sent_ms: u64::from_be_bytes(reader.take()?),
            signature: reader.take()?,
        })
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], ClockError> {
        let end = self.offset.checked_add(N).ok_or(ClockError::Encoding)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(ClockError::Encoding)?
            .try_into()
            .map_err(|_| ClockError::Encoding)?;
        self.offset = end;
        Ok(value)
    }
}

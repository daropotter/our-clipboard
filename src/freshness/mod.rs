//! Conservative age intervals over authenticated suspend-aware boot clocks, not UTC subtraction.
use crate::{
    events::{
        EventError, ReplayDecision, ReplayGuard, SessionBinding, SessionId, VerifiedTextEvent,
    },
    identity::{DeviceId, DeviceIdentity},
};
mod admission;
mod buffer;
mod delivery;
pub use buffer::{BufferError, BufferedDelivery};
mod durable;
pub use delivery::{DeliveryDecision, DeliveryError, DeliveryReceiver};
pub use durable::{DurableAdmission, StateError, probe as durable_probe};
mod exchange;
mod native_clock;
pub use admission::{Admission, AdmissionDecision, AdmissionError, LamportClock};
pub use exchange::{ClockBinding, ClockProbe, ClockReply, ClockRequest};
pub use native_clock::{BootClock, SystemBootClock};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockError {
    Unavailable,
    Entropy,
    Encoding,
    WrongContext,
    Unauthenticated,
    ClockRegressed,
    InvalidSample,
    InvalidPolicy,
    StaleCalibration,
}
impl ClockError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "A suspend-aware boot clock is unavailable; no wall-clock fallback is used."
            }
            Self::Entropy => "Could not create an authenticated clock challenge.",
            Self::Encoding => "Clock metadata encoding is invalid or unsupported.",
            Self::WrongContext => {
                "Clock metadata does not match the expected group, source or receiver session."
            }
            Self::Unauthenticated => "Clock reply does not match the trusted source key.",
            Self::ClockRegressed => {
                "Boot clock moved backwards; establish a new authenticated session."
            }
            Self::InvalidSample => "Clock sample is inconsistent or exceeds the round-trip limit.",
            Self::InvalidPolicy => {
                "Explicit usable clock drift, granularity and lifetime bounds are required."
            }
            Self::StaleCalibration => {
                "Clock calibration is too old; obtain a new authenticated sample."
            }
        }
    }
}
#[derive(Clone, Copy)]
pub struct ClockPolicy {
    max_rtt_ms: u64,
    max_sample_age_ms: u64,
    max_relative_drift_ppm: u32,
    granularity_ms: u64,
}
impl ClockPolicy {
    pub fn new(
        max_rtt_ms: u64,
        max_sample_age_ms: u64,
        drift: Option<u32>,
        granularity: Option<u64>,
    ) -> Result<Self, ClockError> {
        let (Some(max_relative_drift_ppm), Some(granularity_ms)) = (drift, granularity) else {
            return Err(ClockError::InvalidPolicy);
        };
        if max_rtt_ms == 0 || max_sample_age_ms == 0 || max_relative_drift_ppm >= 1_000_000 {
            return Err(ClockError::InvalidPolicy);
        }
        Ok(Self {
            max_rtt_ms,
            max_sample_age_ms,
            max_relative_drift_ppm,
            granularity_ms,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiverBinding {
    pub group: [u8; 32],
    pub membership: [u8; 32],
    pub receiver: DeviceId,
    pub session: SessionId,
}
/// Keep this running session across accidental reconnects; recreate for restart/manual rejoin.
#[derive(Clone)]
pub struct ReceiverSession {
    binding: ReceiverBinding,
    ready_ms: u64,
    accept_after_ms: u64,
    max_ttl_ms: u32,
}
impl ReceiverSession {
    pub fn after_authenticated_handshake(
        binding: ReceiverBinding,
        ready_ms: u64,
        max_ttl_ms: u32,
    ) -> Result<Self, ClockError> {
        if max_ttl_ms == 0 {
            return Err(ClockError::InvalidPolicy);
        }
        let accept_after_ms = ready_ms
            .checked_add(u64::from(max_ttl_ms))
            .ok_or(ClockError::InvalidPolicy)?;
        Ok(Self {
            binding,
            ready_ms,
            accept_after_ms,
            max_ttl_ms,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Muted,
    BeforeSession,
    Future,
    Expired,
    Uncertain,
}
/// Bounds stay anchored to the original authenticated measurement and never renew with forwarding.
pub struct ClockEstimate {
    binding: ClockBinding,
    sampled_at_ms: u64,
    source_sent_ms: u64,
    rtt_ms: u64,
    offset_lower: i128,
    offset_upper: i128,
    policy: ClockPolicy,
}
impl ClockEstimate {
    pub fn assess(
        &self,
        event: &VerifiedTextEvent<'_>,
        receiver: &ReceiverSession,
        now_ms: u64,
    ) -> Result<Freshness, ClockError> {
        let context = event.context();
        let binding = self.binding;
        let local = receiver.binding;
        if context.group != binding.group
            || context.membership != binding.membership
            || context.session != binding.source_session
            || event.source() != binding.source
            || local.group != binding.group
            || local.membership != binding.membership
            || local.receiver != binding.receiver
            || local.session != binding.receiver_session
        {
            return Err(ClockError::WrongContext);
        }
        let elapsed = now_ms
            .checked_sub(self.sampled_at_ms)
            .ok_or(ClockError::ClockRegressed)?;
        if now_ms < receiver.ready_ms {
            return Err(ClockError::ClockRegressed);
        }
        if now_ms < receiver.accept_after_ms {
            return Ok(Freshness::Muted);
        }
        if elapsed > self.policy.max_sample_age_ms {
            return Err(ClockError::StaleCalibration);
        }
        let distance = context.created_boot_ms.abs_diff(self.source_sent_ms);
        let horizon = u128::from(elapsed) + u128::from(self.rtt_ms) + u128::from(distance);
        let drift = u128::from(self.policy.max_relative_drift_ppm);
        let growth = (horizon * drift).div_ceil(1_000_000 - drift);
        let margin = i128::try_from(growth).map_err(|_| ClockError::InvalidPolicy)?
            + 2 * i128::from(self.policy.granularity_ms);
        let age_min =
            i128::from(now_ms) - i128::from(context.created_boot_ms) + self.offset_lower - margin;
        let age_max =
            i128::from(now_ms) - i128::from(context.created_boot_ms) + self.offset_upper + margin;
        let earliest = i128::from(now_ms) - age_max;
        let latest = i128::from(now_ms) - age_min;
        if latest < i128::from(receiver.accept_after_ms) {
            return Ok(Freshness::BeforeSession);
        }
        if earliest < i128::from(receiver.accept_after_ms) {
            return Ok(Freshness::Uncertain);
        }
        if age_max < 0 {
            return Ok(Freshness::Future);
        }
        if age_min < 0 {
            return Ok(Freshness::Uncertain);
        }
        let ttl = i128::from(context.ttl_ms.min(receiver.max_ttl_ms));
        if age_min >= ttl {
            return Ok(Freshness::Expired);
        }
        if age_max >= ttl {
            return Ok(Freshness::Uncertain);
        }
        Ok(Freshness::Fresh)
    }
}

pub fn probe() -> Result<String, ClockError> {
    let _native = SystemBootClock.now_ms()?;
    let source = DeviceIdentity::generate().map_err(|_| ClockError::Entropy)?;
    let receiver = DeviceIdentity::generate().map_err(|_| ClockError::Entropy)?;
    let binding = ClockBinding {
        group: [1; 32],
        membership: [2; 32],
        source: source.id(),
        source_session: SessionId::random().map_err(|_| ClockError::Entropy)?,
        receiver: receiver.id(),
        receiver_session: SessionId::random().map_err(|_| ClockError::Entropy)?,
    };
    let pending = ClockProbe::start(binding, 40_000)?;
    let reply = ClockReply::sign(pending.request(), &source, 50_000, 50_000)?;
    let estimate = pending.finish(
        &reply,
        &source.public_key(),
        40_000,
        ClockPolicy::new(2_000, 120_000, Some(0), Some(0))?,
    )?;
    let session = ReceiverSession::after_authenticated_handshake(
        ReceiverBinding {
            group: binding.group,
            membership: binding.membership,
            receiver: receiver.id(),
            session: binding.receiver_session,
        },
        0,
        30_000,
    )?;
    let event = crate::events::SignedTextEvent::sign(
        crate::events::EventContext {
            group: binding.group,
            membership: binding.membership,
            session: binding.source_session,
            sequence: 1,
            lamport: 1,
            created_unix_ms: 1_700_000_000_000,
            created_boot_ms: 60_000,
            ttl_ms: 30_000,
        },
        "Synthetic freshness probe",
        &source,
    )
    .map_err(|_| ClockError::InvalidSample)?;
    let verified = event
        .verify_with_trusted_key(&source.public_key())
        .map_err(|_| ClockError::Unauthenticated)?;
    if estimate.assess(&verified, &session, 50_100)? != Freshness::Fresh
        || estimate.assess(&verified, &session, 90_000)? != Freshness::Expired
    {
        return Err(ClockError::InvalidSample);
    }
    Ok("Suspend-aware boot clock available. Synthetic authenticated clock bounds, fresh/expired decisions and UTC-independent age checks passed.".into())
}
#[cfg(test)]
mod tests;

// Crash helpers briefly inherit open file locks during fork, before exec closes them.
#[cfg(test)]
pub(super) static STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

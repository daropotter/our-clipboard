use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    Clock(ClockError),
    Replay(EventError),
    LogicalClockExhausted,
    ConflictingOrder,
}
#[derive(Default, Clone)]
pub struct LamportClock {
    value: u64,
}
impl LamportClock {
    /// Seed only from trusted persisted/handshake metadata before publishing after restart.
    pub fn from_watermark(value: u64) -> Self {
        Self { value }
    }
    pub fn current(&self) -> u64 {
        self.value
    }
    pub fn tick(&mut self) -> Result<u64, AdmissionError> {
        let next = self
            .value
            .checked_add(1)
            .ok_or(AdmissionError::LogicalClockExhausted)?;
        self.value = next;
        Ok(next)
    }
    fn next_for(&self, event: &VerifiedTextEvent<'_>) -> Result<u64, AdmissionError> {
        self.value
            .max(event.context().lamport)
            .checked_add(1)
            .ok_or(AdmissionError::LogicalClockExhausted)
    }
    pub fn observe_verified(
        &mut self,
        event: &VerifiedTextEvent<'_>,
    ) -> Result<u64, AdmissionError> {
        let next = self.next_for(event)?;
        self.value = next;
        Ok(next)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    WinnerSelected,
    OlderCandidate,
    Duplicate,
    RetiredSequence,
    Skipped(Freshness),
}
#[derive(Clone)]
pub(super) struct Winner {
    pub(super) order: (u64, DeviceId),
    pub(super) hash: [u8; 32],
}
/// Logical admission only: the actuator must track actual clipboard success/ACK separately.
#[derive(Clone)]
pub struct Admission {
    pub(super) receiver: ReceiverSession,
    pub(super) replay: ReplayGuard,
    pub(super) clock: LamportClock,
    pub(super) winner: Option<Winner>,
}
impl Admission {
    pub fn new(receiver: ReceiverSession, capacity: usize, clock: LamportClock) -> Self {
        Self {
            receiver,
            replay: ReplayGuard::new(capacity),
            clock,
            winner: None,
        }
    }
    pub fn activate_authenticated_source(
        &mut self,
        binding: SessionBinding,
    ) -> Result<(), AdmissionError> {
        if binding.group != self.receiver.binding.group
            || binding.membership != self.receiver.binding.membership
        {
            return Err(AdmissionError::Replay(EventError::MembershipMismatch));
        }
        self.replay
            .activate_authenticated_session(binding)
            .map_err(AdmissionError::Replay)
    }
    pub fn clock(&self) -> u64 {
        self.clock.current()
    }
    pub fn winner_source(&self) -> Option<DeviceId> {
        self.winner.as_ref().map(|w| w.order.1)
    }
    pub fn admit(
        &mut self,
        event: &VerifiedTextEvent<'_>,
        estimate: &ClockEstimate,
        now_ms: u64,
    ) -> Result<AdmissionDecision, AdmissionError> {
        let freshness = estimate
            .assess(event, &self.receiver, now_ms)
            .map_err(AdmissionError::Clock)?;
        if freshness != Freshness::Fresh {
            return Ok(AdmissionDecision::Skipped(freshness));
        }
        let order = event.order_key();
        let hash = event.envelope_hash();
        if self
            .winner
            .as_ref()
            .is_some_and(|w| w.order == order && w.hash != hash)
        {
            return Err(AdmissionError::ConflictingOrder);
        }
        match self.replay.check(event).map_err(AdmissionError::Replay)? {
            ReplayDecision::Duplicate => return Ok(AdmissionDecision::Duplicate),
            ReplayDecision::RetiredSequence => return Ok(AdmissionDecision::RetiredSequence),
            ReplayDecision::New => {}
        }
        let next = self.clock.next_for(event)?;
        self.replay.observe(event).map_err(AdmissionError::Replay)?;
        self.clock.value = next;
        if self.winner.as_ref().is_none_or(|w| order > w.order) {
            self.winner = Some(Winner { order, hash });
            Ok(AdmissionDecision::WinnerSelected)
        } else {
            Ok(AdmissionDecision::OlderCandidate)
        }
    }
}

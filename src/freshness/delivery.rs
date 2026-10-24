//! Application receipts refer to a successful write, not continued clipboard ownership.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryError {
    State(StateError),
    Clock(ClockError),
    ApplyFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryDecision {
    Applied { envelope_hash: [u8; 32] },
    AlreadyApplied { envelope_hash: [u8; 32] },
    Skipped(Freshness),
    OlderCandidate,
    RetiredSequence,
    BufferedIdle { envelope_hash: [u8; 32] },
}
/// Single-owner serialized receiver. Keeps one successful application hash, never payload history.
/// Receipts are memory-only: a restart never fabricates a historical application ACK.
pub struct DeliveryReceiver {
    state: DurableAdmission,
    applied: Option<[u8; 32]>,
}
impl DeliveryReceiver {
    pub fn new(state: DurableAdmission) -> Self {
        Self {
            state,
            applied: None,
        }
    }
    pub fn activate_authenticated_source(
        &mut self,
        binding: SessionBinding,
    ) -> Result<(), StateError> {
        self.state.activate_authenticated_source(binding)
    }
    pub fn reserve_publication(&mut self) -> Result<(u64, u64), StateError> {
        self.state.reserve_publication()
    }
    /// Authenticate/verify first. Callers must send returned receipts only over authenticated channels.
    /// Retry by resubmitting the same envelope while fresh; failed writes do not produce receipts.
    /// The callback must report actual write success, not queue acceptance, and must not reenter.
    pub fn receive<C: BootClock>(
        &mut self,
        event: &VerifiedTextEvent<'_>,
        estimate: &ClockEstimate,
        clock: &C,
        mut apply: impl FnMut(&str) -> std::result::Result<(), ()>,
    ) -> std::result::Result<DeliveryDecision, DeliveryError> {
        self.receive_inner(event, estimate, clock, &mut apply, true)
    }
    pub(super) fn defer<C: BootClock>(
        &mut self,
        event: &VerifiedTextEvent<'_>,
        estimate: &ClockEstimate,
        clock: &C,
    ) -> Result<DeliveryDecision, DeliveryError> {
        self.receive_inner(
            event,
            estimate,
            clock,
            &mut |_| unreachable!("Idle never invokes actuator"),
            false,
        )
    }
    pub(super) fn assess(
        &self,
        event: &VerifiedTextEvent<'_>,
        estimate: &ClockEstimate,
        now: u64,
    ) -> Result<Freshness, ClockError> {
        self.state.assess(event, estimate, now)
    }
    fn receive_inner<C: BootClock>(
        &mut self,
        event: &VerifiedTextEvent<'_>,
        estimate: &ClockEstimate,
        clock: &C,
        apply: &mut impl FnMut(&str) -> Result<(), ()>,
        allow_apply: bool,
    ) -> Result<DeliveryDecision, DeliveryError> {
        let now = clock.now_ms().map_err(DeliveryError::Clock)?;
        let decision = self
            .state
            .admit(event, estimate, now)
            .map_err(DeliveryError::State)?;
        match decision {
            AdmissionDecision::Skipped(f) => return Ok(DeliveryDecision::Skipped(f)),
            AdmissionDecision::RetiredSequence => return Ok(DeliveryDecision::RetiredSequence),
            AdmissionDecision::OlderCandidate => return Ok(DeliveryDecision::OlderCandidate),
            AdmissionDecision::Duplicate | AdmissionDecision::WinnerSelected => {}
        }
        if !self.state.is_winner(event) {
            return Ok(DeliveryDecision::OlderCandidate);
        }
        let hash = event.envelope_hash();
        if self.applied == Some(hash) {
            return Ok(DeliveryDecision::AlreadyApplied {
                envelope_hash: hash,
            });
        }
        // Disk synchronization can take long enough to consume the remaining TTL.
        let now = clock.now_ms().map_err(DeliveryError::Clock)?;
        let freshness = self
            .state
            .assess(event, estimate, now)
            .map_err(DeliveryError::Clock)?;
        if freshness != Freshness::Fresh {
            return Ok(DeliveryDecision::Skipped(freshness));
        }
        if !allow_apply {
            return Ok(DeliveryDecision::BufferedIdle {
                envelope_hash: hash,
            });
        }
        apply(event.text()).map_err(|_| DeliveryError::ApplyFailed)?;
        self.applied = Some(hash);
        Ok(DeliveryDecision::Applied {
            envelope_hash: hash,
        })
    }
}
#[cfg(test)]
mod tests;

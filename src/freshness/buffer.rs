//! One latest signed envelope in memory; original bytes/TTL survive retry, never restart.
use super::*;
use crate::events::{SignedTextEvent, TEXT_EVENT_WIRE_OVERHEAD};
use zeroize::Zeroizing;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferError {
    Closed,
    Idle,
    Capacity,
    Verification(EventError),
    Delivery(DeliveryError),
    Clock(ClockError),
}
struct Pending {
    hash: [u8; 32],
    wire: Zeroizing<Vec<u8>>,
    key: [u8; 32],
    binding: SessionBinding,
}
/// Serialized session owner. Caller drives maintenance even while Idle/disconnected.
/// No clipboard reads occur here. Stop/rebaseline the native observer separately on Idle exit.
pub struct BufferedDelivery {
    delivery: DeliveryReceiver,
    pending: Option<Pending>,
    limit: usize,
    idle: bool,
    closed: bool,
}
impl BufferedDelivery {
    pub fn new(delivery: DeliveryReceiver, max_wire_bytes: usize) -> Result<Self, BufferError> {
        // Text must contain at least one UTF-8 byte in addition to header/signature.
        if !(TEXT_EVENT_WIRE_OVERHEAD + 1..=crate::text::MAX_TEXT_BYTES + TEXT_EVENT_WIRE_OVERHEAD)
            .contains(&max_wire_bytes)
        {
            return Err(BufferError::Capacity);
        }
        Ok(Self {
            delivery,
            pending: None,
            limit: max_wire_bytes,
            idle: false,
            closed: false,
        })
    }
    fn healthy(&self) -> Result<(), BufferError> {
        if self.closed {
            Err(BufferError::Closed)
        } else {
            Ok(())
        }
    }
    pub fn set_idle(&mut self, idle: bool) -> Result<(), BufferError> {
        self.healthy()?;
        self.idle = idle;
        Ok(())
    }
    pub fn is_idle(&self) -> bool {
        self.idle
    }
    pub fn close(&mut self) {
        self.pending = None;
        self.closed = true;
    }
    pub fn pending_source(&self) -> Option<SessionBinding> {
        self.pending.as_ref().map(|p| p.binding)
    }
    pub fn activate_authenticated_source(
        &mut self,
        binding: SessionBinding,
    ) -> Result<(), BufferError> {
        self.healthy()?;
        self.delivery
            .activate_authenticated_source(binding)
            .map_err(|e| BufferError::Delivery(DeliveryError::State(e)))?;
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.binding.source == binding.source && p.binding != binding)
        {
            self.pending = None;
        }
        Ok(())
    }
    pub fn reserve_publication(&mut self) -> Result<(u64, u64), BufferError> {
        self.healthy()?;
        if self.idle {
            return Err(BufferError::Idle);
        }
        self.delivery
            .reserve_publication()
            .map_err(|e| BufferError::Delivery(DeliveryError::State(e)))
    }
    pub fn record_local_event(&mut self, event: &VerifiedTextEvent<'_>) -> Result<(), BufferError> {
        self.healthy()?;
        if self.idle {
            return Err(BufferError::Idle);
        }
        self.delivery
            .record_local_event(event)
            .map_err(|e| BufferError::Delivery(DeliveryError::State(e)))?;
        self.pending = None;
        Ok(())
    }
    pub fn receive<C: BootClock>(
        &mut self,
        event: &SignedTextEvent,
        key: &[u8; 32],
        estimate: &ClockEstimate,
        clock: &C,
        apply: impl FnMut(&str) -> Result<(), ()>,
    ) -> Result<DeliveryDecision, BufferError> {
        self.healthy()?;
        let verified = event
            .verify_with_trusted_key(key)
            .map_err(BufferError::Verification)?;
        if verified.text().len() + TEXT_EVENT_WIRE_OVERHEAD > self.limit {
            return Err(BufferError::Capacity);
        }
        let decision = if self.idle {
            self.delivery.defer(&verified, estimate, clock)
        } else {
            self.delivery.receive(&verified, estimate, clock, apply)
        };
        match decision {
            Ok(DeliveryDecision::BufferedIdle { .. }) | Err(DeliveryError::ApplyFailed) => {
                let c = verified.context();
                self.pending = Some(Pending {
                    hash: verified.envelope_hash(),
                    wire: Zeroizing::new(event.to_wire()),
                    key: *key,
                    binding: SessionBinding {
                        group: c.group,
                        membership: c.membership,
                        source: verified.source(),
                        session: c.session,
                    },
                });
            }
            Ok(DeliveryDecision::Applied { .. } | DeliveryDecision::AlreadyApplied { .. }) => {
                self.pending = None
            }
            _ => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.hash == verified.envelope_hash())
                {
                    self.pending = None;
                }
            }
        }
        decision.map_err(BufferError::Delivery)
    }
    /// Drop a non-fresh candidate without touching the native clipboard or emitting an ACK.
    /// Freshness outcomes are intentionally silent; clock errors are returned after release.
    /// Call pending_source() to distinguish a retained candidate from an empty buffer.
    pub fn maintain<C: BootClock>(
        &mut self,
        estimate: &ClockEstimate,
        clock: &C,
    ) -> Result<(), BufferError> {
        self.healthy()?;
        if let Some(p) = &self.pending {
            let event = SignedTextEvent::from_wire(&p.wire).map_err(BufferError::Verification)?;
            let verified = event
                .verify_with_trusted_key(&p.key)
                .map_err(BufferError::Verification)?;
            let result = clock
                .now_ms()
                .and_then(|now| self.delivery.assess(&verified, estimate, now));
            if result != Ok(Freshness::Fresh) {
                self.pending = None;
            }
            result.map_err(BufferError::Clock)?;
        }
        Ok(())
    }
    /// Use a current authenticated estimate for pending_source(); this does not start a new session.
    pub fn retry_pending<C: BootClock>(
        &mut self,
        estimate: &ClockEstimate,
        clock: &C,
        apply: impl FnMut(&str) -> Result<(), ()>,
    ) -> Result<Option<DeliveryDecision>, BufferError> {
        self.maintain(estimate, clock)?;
        if self.idle {
            return Ok(None);
        }
        let Some(pending) = self.pending.take() else {
            return Ok(None);
        };
        let event = SignedTextEvent::from_wire(&pending.wire).map_err(BufferError::Verification)?;
        self.receive(&event, &pending.key, estimate, clock, apply)
            .map(Some)
    }
}
#[cfg(test)]
mod tests;

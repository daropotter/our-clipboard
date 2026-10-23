use super::*;
use std::collections::{HashMap, VecDeque};
const RECENT: usize = 32;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBinding {
    pub group: [u8; 32],
    pub membership: [u8; 32],
    pub source: DeviceId,
    pub session: SessionId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayDecision {
    New,
    Duplicate,
    RetiredSequence,
}
struct State {
    binding: SessionBinding,
    sequence: u64,
    lamport: u64,
    recent: VecDeque<(u64, [u8; 32])>,
}
/// Watermarks are never evicted when the small digest cache expires or capacity fills.
/// Restore authenticated session/replay state or establish fresh sessions after restart.
pub struct ReplayGuard {
    capacity: usize,
    states: HashMap<([u8; 32], DeviceId), State>,
}
impl ReplayGuard {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            states: HashMap::new(),
        }
    }
    /// Only the authenticated handshake/membership layer may call this, never an event.
    /// Re-activating the current session does not reset its watermark.
    pub fn activate_authenticated_session(
        &mut self,
        binding: SessionBinding,
    ) -> Result<(), EventError> {
        let key = (binding.group, binding.source);
        if let Some(state) = self.states.get_mut(&key) {
            if state.binding.session == binding.session {
                state.binding.membership = binding.membership;
                return Ok(());
            }
        } else if self.states.len() >= self.capacity {
            return Err(EventError::Capacity);
        }
        let previous_lamport = self.states.get(&key).map_or(0, |state| state.lamport);
        self.states.insert(
            key,
            State {
                binding,
                sequence: 0,
                lamport: previous_lamport,
                recent: VecDeque::new(),
            },
        );
        Ok(())
    }
    pub fn check(&self, event: &VerifiedTextEvent<'_>) -> Result<ReplayDecision, EventError> {
        let context = event.context();
        let state = self
            .states
            .get(&(context.group, event.source()))
            .ok_or(EventError::UnknownSession)?;
        if context.session != state.binding.session {
            return Err(EventError::RetiredSession);
        }
        if context.membership != state.binding.membership {
            return Err(EventError::MembershipMismatch);
        }
        let hash = event.envelope_hash();
        if let Some((_, prior)) = state
            .recent
            .iter()
            .find(|(sequence, _)| *sequence == context.sequence)
        {
            return if *prior == hash {
                Ok(ReplayDecision::Duplicate)
            } else {
                Err(EventError::ConflictingEvent)
            };
        }
        if context.sequence <= state.sequence {
            return Ok(ReplayDecision::RetiredSequence);
        }
        if context.lamport <= state.lamport {
            return Err(EventError::ClockNotAdvanced);
        }
        Ok(ReplayDecision::New)
    }
    pub fn observe(&mut self, event: &VerifiedTextEvent<'_>) -> Result<ReplayDecision, EventError> {
        let decision = self.check(event)?;
        if decision != ReplayDecision::New {
            return Ok(decision);
        }
        let context = event.context();
        let hash = event.envelope_hash();
        let state = self
            .states
            .get_mut(&(context.group, event.source()))
            .ok_or(EventError::UnknownSession)?;
        state.sequence = context.sequence;
        state.lamport = context.lamport;
        state.recent.push_back((context.sequence, hash));
        if state.recent.len() > RECENT {
            state.recent.pop_front();
        }
        Ok(ReplayDecision::New)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::tests::context;
    fn binding(identity: &DeviceIdentity) -> SessionBinding {
        let c = context();
        SessionBinding {
            group: c.group,
            membership: c.membership,
            source: identity.id(),
            session: c.session,
        }
    }
    #[test]
    fn dedup_cache_eviction_and_idempotent_activation_do_not_restore_replays() {
        let identity = DeviceIdentity::generate().unwrap();
        let mut guard = ReplayGuard::new(1);
        guard
            .activate_authenticated_session(binding(&identity))
            .unwrap();
        let first = SignedTextEvent::sign(context(), "first", &identity).unwrap();
        for sequence in 1..=40 {
            let event = SignedTextEvent::sign(
                EventContext {
                    sequence,
                    lamport: sequence,
                    ..context()
                },
                "text",
                &identity,
            )
            .unwrap();
            assert_eq!(
                guard.observe(
                    &event
                        .verify_with_trusted_key(&identity.public_key())
                        .unwrap()
                ),
                Ok(ReplayDecision::New)
            );
        }
        guard
            .activate_authenticated_session(binding(&identity))
            .unwrap();
        assert_eq!(
            guard.observe(
                &first
                    .verify_with_trusted_key(&identity.public_key())
                    .unwrap()
            ),
            Ok(ReplayDecision::RetiredSequence)
        );
        assert_eq!(
            guard.activate_authenticated_session(binding(&DeviceIdentity::generate().unwrap())),
            Err(EventError::Capacity)
        );
    }
    #[test]
    fn same_identifier_different_signed_payload_is_a_conflict() {
        let identity = DeviceIdentity::generate().unwrap();
        let mut guard = ReplayGuard::new(1);
        guard
            .activate_authenticated_session(binding(&identity))
            .unwrap();
        let a = SignedTextEvent::sign(context(), "one", &identity).unwrap();
        let b = SignedTextEvent::sign(context(), "two", &identity).unwrap();
        let a = a.verify_with_trusted_key(&identity.public_key()).unwrap();
        assert_eq!(guard.observe(&a), Ok(ReplayDecision::New));
        assert_eq!(guard.observe(&a), Ok(ReplayDecision::Duplicate));
        assert_eq!(
            guard.observe(&b.verify_with_trusted_key(&identity.public_key()).unwrap()),
            Err(EventError::ConflictingEvent)
        );
    }
    #[test]
    fn session_and_membership_cannot_be_learned_from_an_event() {
        let identity = DeviceIdentity::generate().unwrap();
        let mut guard = ReplayGuard::new(1);
        let event = SignedTextEvent::sign(context(), "one", &identity).unwrap();
        let verified = event
            .verify_with_trusted_key(&identity.public_key())
            .unwrap();
        assert_eq!(guard.observe(&verified), Err(EventError::UnknownSession));
        let mut current = binding(&identity);
        current.session = SessionId([8; 16]);
        guard.activate_authenticated_session(current).unwrap();
        assert_eq!(guard.observe(&verified), Err(EventError::RetiredSession));
        current.session = context().session;
        current.membership = [9; 32];
        guard.activate_authenticated_session(current).unwrap();
        assert_eq!(
            guard.observe(&verified),
            Err(EventError::MembershipMismatch)
        );
    }
}

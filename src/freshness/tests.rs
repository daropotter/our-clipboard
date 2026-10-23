use super::*;
use crate::events::{EventContext, SignedTextEvent};
struct Fixture {
    source: DeviceIdentity,
    binding: ClockBinding,
}
impl Fixture {
    fn new() -> Self {
        let source = DeviceIdentity::generate().unwrap();
        let receiver = DeviceIdentity::generate().unwrap();
        let binding = ClockBinding {
            group: [1; 32],
            membership: [2; 32],
            source: source.id(),
            source_session: SessionId([3; 16]),
            receiver: receiver.id(),
            receiver_session: SessionId([4; 16]),
        };
        Self { source, binding }
    }
    fn policy() -> ClockPolicy {
        ClockPolicy::new(100, 120_000, Some(0), Some(0)).unwrap()
    }
    fn estimate(&self) -> ClockEstimate {
        self.sample(40_000, 40_000, 50_000, 50_000)
    }
    fn sample(
        &self,
        sent: u64,
        received: u64,
        remote_received: u64,
        remote_sent: u64,
    ) -> ClockEstimate {
        let probe = ClockProbe::start(self.binding, sent).unwrap();
        let reply =
            ClockReply::sign(probe.request(), &self.source, remote_received, remote_sent).unwrap();
        probe
            .finish(&reply, &self.source.public_key(), received, Self::policy())
            .unwrap()
    }
    fn receiver(&self, ready: u64) -> ReceiverSession {
        ReceiverSession::after_authenticated_handshake(
            ReceiverBinding {
                group: self.binding.group,
                membership: self.binding.membership,
                receiver: self.binding.receiver,
                session: self.binding.receiver_session,
            },
            ready,
            30_000,
        )
        .unwrap()
    }
    fn event(&self, sequence: u64, lamport: u64, boot: u64, utc: u64) -> SignedTextEvent {
        SignedTextEvent::sign(
            EventContext {
                group: self.binding.group,
                membership: self.binding.membership,
                session: self.binding.source_session,
                sequence,
                lamport,
                created_boot_ms: boot,
                created_unix_ms: utc,
                ttl_ms: 30_000,
            },
            "synthetic test",
            &self.source,
        )
        .unwrap()
    }
    fn admission(&self) -> Admission {
        let mut a = Admission::new(self.receiver(0), 2, LamportClock::default());
        a.activate_authenticated_source(SessionBinding {
            group: self.binding.group,
            membership: self.binding.membership,
            source: self.binding.source,
            session: self.binding.source_session,
        })
        .unwrap();
        a
    }
}
#[test]
fn ttl_boundary_sleep_and_utc_are_independent() {
    let f = Fixture::new();
    let estimate = f.estimate();
    let receiver = f.receiver(0);
    for utc in [0, u64::MAX - 30_000] {
        let event = f.event(1, 1, 60_000, utc);
        let v = event
            .verify_with_trusted_key(&f.source.public_key())
            .unwrap();
        assert_eq!(estimate.assess(&v, &receiver, 79_999), Ok(Freshness::Fresh));
        assert_eq!(
            estimate.assess(&v, &receiver, 80_000),
            Ok(Freshness::Expired)
        );
        assert_eq!(
            estimate.assess(&v, &receiver, 90_000),
            Ok(Freshness::Expired)
        );
    }
}
#[test]
fn uncertain_age_is_never_fresh() {
    let f = Fixture::new();
    let e = f.sample(40_000, 40_020, 50_005, 50_015);
    let event = f.event(1, 1, 60_000, 1);
    let v = event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    assert_eq!(
        e.assess(&v, &f.receiver(0), 80_000),
        Ok(Freshness::Uncertain)
    );
}
#[test]
fn session_quiet_period_context_and_stale_estimate() {
    let f = Fixture::new();
    let e = f.estimate();
    let event = f.event(1, 1, 60_000, 1);
    let v = event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    assert_eq!(
        e.assess(&v, &f.receiver(40_000), 50_001),
        Ok(Freshness::Muted)
    );
    assert_eq!(
        e.assess(&v, &f.receiver(40_000), 70_001),
        Ok(Freshness::BeforeSession)
    );
    assert_eq!(
        e.assess(&v, &f.receiver(0), 39_999),
        Err(ClockError::ClockRegressed)
    );
    assert_eq!(
        e.assess(&v, &f.receiver(0), 160_001),
        Err(ClockError::StaleCalibration)
    );
    let mut receiver = f.receiver(0);
    receiver.binding.session = SessionId([9; 16]);
    assert_eq!(
        e.assess(&v, &receiver, 50_001),
        Err(ClockError::WrongContext)
    );
    let future = f.event(2, 2, 80_000, 1);
    assert_eq!(
        e.assess(
            &future
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &f.receiver(0),
            50_001
        ),
        Ok(Freshness::Future)
    );
}
#[test]
fn signed_clock_wire_roundtrip_and_every_byte_tamper() {
    let f = Fixture::new();
    let probe = ClockProbe::start(f.binding, 40_000).unwrap();
    let request = ClockRequest::from_wire(&probe.request().to_wire()).unwrap();
    assert!(request == *probe.request());
    let reply = ClockReply::sign(&request, &f.source, 50_000, 50_000).unwrap();
    let bytes = reply.to_wire();
    let decoded = ClockReply::from_wire(&bytes).unwrap();
    assert!(
        probe
            .finish(&decoded, &f.source.public_key(), 40_000, Fixture::policy())
            .is_ok()
    );
    for index in 0..bytes.len() {
        let probe = ClockProbe::start(f.binding, 40_000).unwrap();
        let reply = ClockReply::sign(probe.request(), &f.source, 50_000, 50_000).unwrap();
        let mut changed = reply.to_wire();
        changed[index] ^= 1;
        if let Ok(reply) = ClockReply::from_wire(&changed) {
            assert!(
                probe
                    .finish(&reply, &f.source.public_key(), 40_000, Fixture::policy())
                    .is_err()
            );
        }
    }
    assert!(ClockReply::from_wire(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = bytes;
    trailing.push(0);
    assert!(ClockReply::from_wire(&trailing).is_err());
}
#[test]
fn old_reply_wrong_key_regression_and_excess_rtt_fail() {
    let f = Fixture::new();
    let probe = ClockProbe::start(f.binding, 40_000).unwrap();
    let reply = ClockReply::sign(probe.request(), &f.source, 50_000, 50_000).unwrap();
    assert!(
        ClockProbe::start(f.binding, 40_000)
            .unwrap()
            .finish(&reply, &f.source.public_key(), 40_000, Fixture::policy())
            .is_err()
    );
    for (now, key) in [
        (39_999, f.source.public_key()),
        (40_101, f.source.public_key()),
        (40_000, DeviceIdentity::generate().unwrap().public_key()),
    ] {
        let probe = ClockProbe::start(f.binding, 40_000).unwrap();
        let reply = ClockReply::sign(probe.request(), &f.source, 50_000, 50_000).unwrap();
        assert!(probe.finish(&reply, &key, now, Fixture::policy()).is_err());
    }
    assert!(ClockPolicy::new(100, 100, None, Some(1)).is_err());
    assert!(ClockPolicy::new(100, 100, Some(1_000_000), Some(1)).is_err());
}
#[test]
fn expired_event_cannot_poison_replay_or_clock_and_duplicate_does_not_advance() {
    let f = Fixture::new();
    let e = f.estimate();
    let mut a = f.admission();
    let old = f.event(100, 1000, 40_000, 1);
    assert_eq!(
        a.admit(
            &old.verify_with_trusted_key(&f.source.public_key()).unwrap(),
            &e,
            70_001
        ),
        Ok(AdmissionDecision::Skipped(Freshness::Expired))
    );
    assert_eq!(a.clock(), 0);
    let event = f.event(1, 1, 80_000, 1);
    let v = event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    assert_eq!(
        a.admit(&v, &e, 70_001),
        Ok(AdmissionDecision::WinnerSelected)
    );
    assert_eq!(a.clock(), 2);
    assert_eq!(a.admit(&v, &e, 70_001), Ok(AdmissionDecision::Duplicate));
    assert_eq!(a.clock(), 2);
}
#[test]
fn concurrent_sources_converge_independent_of_arrival_order() {
    let f = Fixture::new();
    let mut g = Fixture::new();
    g.binding.receiver = f.binding.receiver;
    g.binding.receiver_session = f.binding.receiver_session;
    let a_event = f.event(1, 5, 60_000, 1);
    let b_event = g.event(1, 5, 60_000, 1);
    let av = a_event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    let bv = b_event
        .verify_with_trusted_key(&g.source.public_key())
        .unwrap();
    let ae = f.estimate();
    let be = g.estimate();
    for reverse in [false, true] {
        let mut a = f.admission();
        a.activate_authenticated_source(SessionBinding {
            group: g.binding.group,
            membership: g.binding.membership,
            source: g.binding.source,
            session: g.binding.source_session,
        })
        .unwrap();
        let pair = if reverse {
            [(&bv, &be), (&av, &ae)]
        } else {
            [(&av, &ae), (&bv, &be)]
        };
        for (event, estimate) in pair {
            a.admit(event, estimate, 50_001).unwrap();
        }
        assert_eq!(a.winner_source(), Some(f.source.id().max(g.source.id())));
    }
}
#[test]
fn overflow_does_not_mutate_admission() {
    let f = Fixture::new();
    let mut a = f.admission();
    let e = f.estimate();
    let huge = f.event(1, u64::MAX, 60_000, 1);
    assert_eq!(
        a.admit(
            &huge
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &e,
            50_001
        ),
        Err(AdmissionError::LogicalClockExhausted)
    );
    assert_eq!(a.clock(), 0);
    assert_eq!(a.winner_source(), None);
    let normal = f.event(1, 1, 60_000, 1);
    assert_eq!(
        a.admit(
            &normal
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &e,
            50_001
        ),
        Ok(AdmissionDecision::WinnerSelected)
    );
}

#[test]
fn explicit_drift_bound_grows_uncertainty() {
    let f = Fixture::new();
    let probe = ClockProbe::start(f.binding, 40_000).unwrap();
    let reply = ClockReply::sign(probe.request(), &f.source, 50_000, 50_000).unwrap();
    let estimate = probe
        .finish(
            &reply,
            &f.source.public_key(),
            40_000,
            ClockPolicy::new(100, 120_000, Some(1000), Some(1)).unwrap(),
        )
        .unwrap();
    let event = f.event(1, 1, 60_000, 1);
    let verified = event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    assert_eq!(
        estimate.assess(&verified, &f.receiver(0), 50_100),
        Ok(Freshness::Fresh)
    );
    assert_eq!(
        estimate.assess(&verified, &f.receiver(0), 79_999),
        Ok(Freshness::Uncertain)
    );
}

#[test]
fn source_restart_cannot_reset_its_lamport_watermark() {
    let mut f = Fixture::new();
    let mut a = f.admission();
    let event = f.event(1, 50, 60_000, 1);
    a.admit(
        &event
            .verify_with_trusted_key(&f.source.public_key())
            .unwrap(),
        &f.estimate(),
        50_001,
    )
    .unwrap();
    f.binding.source_session = SessionId([9; 16]);
    a.activate_authenticated_source(SessionBinding {
        group: f.binding.group,
        membership: f.binding.membership,
        source: f.binding.source,
        session: f.binding.source_session,
    })
    .unwrap();
    let old = f.event(1, 1, 60_000, 1);
    assert_eq!(
        a.admit(
            &old.verify_with_trusted_key(&f.source.public_key()).unwrap(),
            &f.estimate(),
            50_001
        ),
        Err(AdmissionError::Replay(EventError::ClockNotAdvanced))
    );
    let new = f.event(1, 51, 60_000, 1);
    assert_eq!(
        a.admit(
            &new.verify_with_trusted_key(&f.source.public_key()).unwrap(),
            &f.estimate(),
            50_001
        ),
        Ok(AdmissionDecision::WinnerSelected)
    );
}

#[test]
fn duplicate_remains_duplicate_when_local_lamport_is_exhausted() {
    let f = Fixture::new();
    let mut a = f.admission();
    let estimate = f.estimate();
    let event = f.event(1, u64::MAX - 1, 60_000, 1);
    let verified = event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    assert_eq!(
        a.admit(&verified, &estimate, 50_001),
        Ok(AdmissionDecision::WinnerSelected)
    );
    assert_eq!(a.clock(), u64::MAX);
    assert_eq!(
        a.admit(&verified, &estimate, 50_001),
        Ok(AdmissionDecision::Duplicate)
    );
}

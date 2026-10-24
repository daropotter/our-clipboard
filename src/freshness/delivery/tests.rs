use super::*;
use crate::freshness::tests::Fixture;
use std::cell::Cell;
struct Clock(Cell<u64>);
impl BootClock for Clock {
    fn now_ms(&self) -> Result<u64, ClockError> {
        Ok(self.0.get())
    }
}
fn setup() -> (tempfile::TempDir, Fixture, DeliveryReceiver) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let f = Fixture::new();
    let mut receiver =
        DeliveryReceiver::new(DurableAdmission::initialize(dir.path(), f.receiver(0), 2).unwrap());
    receiver
        .activate_authenticated_source(SessionBinding {
            group: f.binding.group,
            membership: f.binding.membership,
            source: f.source.id(),
            session: f.binding.source_session,
        })
        .unwrap();
    (dir, f, receiver)
}
#[test]
fn failed_write_can_retry_without_advancing_lamport_and_ack_only_follows_success() {
    let _lock = crate::freshness::STATE_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (_dir, f, mut receiver) = setup();
    let clock = Clock(Cell::new(50_001));
    let e = f.event(1, 1, 60_000, 1);
    let v = e.verify_with_trusted_key(&f.source.public_key()).unwrap();
    let estimate = f.estimate();
    assert_eq!(
        receiver.receive(&v, &estimate, &clock, |_| Err(())),
        Err(DeliveryError::ApplyFailed)
    );
    let saved_clock = receiver.state.clock();
    let mut writes = 0;
    assert_eq!(
        receiver.receive(&v, &estimate, &clock, |text| {
            assert_eq!(text, "synthetic test");
            writes += 1;
            Ok(())
        }),
        Ok(DeliveryDecision::Applied {
            envelope_hash: e.envelope_hash()
        })
    );
    assert_eq!(receiver.state.clock(), saved_clock);
    assert_eq!(
        receiver.receive(&v, &estimate, &clock, |_| {
            writes += 1;
            Ok(())
        }),
        Ok(DeliveryDecision::AlreadyApplied {
            envelope_hash: e.envelope_hash()
        })
    );
    assert_eq!(writes, 1);
}
#[test]
fn failed_write_cannot_retry_after_expiry_or_after_a_new_winner() {
    let _lock = crate::freshness::STATE_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (_dir, f, mut r) = setup();
    let clock = Clock(Cell::new(50_001));
    let estimate = f.estimate();
    let a = f.event(1, 1, 60_000, 1);
    let av = a.verify_with_trusted_key(&f.source.public_key()).unwrap();
    assert_eq!(
        r.receive(&av, &estimate, &clock, |_| Err(())),
        Err(DeliveryError::ApplyFailed)
    );
    let b = f.event(2, 2, 60_000, 1);
    let bv = b.verify_with_trusted_key(&f.source.public_key()).unwrap();
    assert!(matches!(
        r.receive(&bv, &estimate, &clock, |_| Ok(())),
        Ok(DeliveryDecision::Applied { .. })
    ));
    assert_eq!(
        r.receive(&av, &estimate, &clock, |_| panic!("old winner applied")),
        Ok(DeliveryDecision::OlderCandidate)
    );
    clock.0.set(80_000);
    assert_eq!(
        r.receive(&bv, &estimate, &clock, |_| panic!("expired applied")),
        Ok(DeliveryDecision::Skipped(Freshness::Expired))
    );
}
#[test]
fn age_is_rechecked_after_durable_commit_before_native_write() {
    let _lock = crate::freshness::STATE_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    struct Advancing(Cell<u64>);
    impl BootClock for Advancing {
        fn now_ms(&self) -> Result<u64, ClockError> {
            let t = self.0.get();
            self.0.set(80_000);
            Ok(t)
        }
    }
    let (_dir, f, mut r) = setup();
    let e = f.event(1, 1, 60_000, 1);
    let v = e.verify_with_trusted_key(&f.source.public_key()).unwrap();
    assert_eq!(
        r.receive(
            &v,
            &f.estimate(),
            &Advancing(Cell::new(50_001)),
            |_| panic!("expired during commit")
        ),
        Ok(DeliveryDecision::Skipped(Freshness::Expired))
    );
    assert_eq!(r.state.clock(), 2);
    assert_eq!(r.applied, None);
}
#[test]
fn clock_failure_or_overflow_never_calls_actuator() {
    let _lock = crate::freshness::STATE_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    struct Broken;
    impl BootClock for Broken {
        fn now_ms(&self) -> Result<u64, ClockError> {
            Err(ClockError::Unavailable)
        }
    }
    let (_dir, f, mut r) = setup();
    let e = f.event(1, 1, 60_000, 1);
    let v = e.verify_with_trusted_key(&f.source.public_key()).unwrap();
    assert_eq!(
        r.receive(&v, &f.estimate(), &Broken, |_| panic!("unavailable clock")),
        Err(DeliveryError::Clock(ClockError::Unavailable))
    );
    // Logical overflow is rejected before application, as are failed durable reservations.
    let huge = f.event(1, u64::MAX, 60_000, 1);
    assert_eq!(
        r.receive(
            &huge
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &f.estimate(),
            &Clock(Cell::new(50_001)),
            |_| panic!("overflow applied")
        ),
        Err(DeliveryError::State(StateError::Admission(
            AdmissionError::LogicalClockExhausted
        )))
    );
}

#[test]
fn failed_commit_produces_no_write_or_receipt() {
    let _lock = crate::freshness::STATE_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (_dir, f, mut r) = setup();
    r.state.fail_writes_for_test();
    let e = f.event(1, 1, 60_000, 1);
    let v = e.verify_with_trusted_key(&f.source.public_key()).unwrap();
    assert_eq!(
        r.receive(&v, &f.estimate(), &Clock(Cell::new(50_001)), |_| panic!(
            "uncommitted applied"
        )),
        Err(DeliveryError::State(StateError::Storage))
    );
    assert_eq!(r.applied, None);
    assert_eq!(
        r.receive(&v, &f.estimate(), &Clock(Cell::new(50_001)), |_| panic!(
            "poisoned applied"
        )),
        Err(DeliveryError::State(StateError::Poisoned))
    );
}
#[test]
fn reopen_does_not_fabricate_an_application_receipt() {
    let _lock = crate::freshness::STATE_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (dir, mut f, mut r) = setup();
    let e = f.event(1, 1, 60_000, 1);
    let v = e.verify_with_trusted_key(&f.source.public_key()).unwrap();
    r.receive(&v, &f.estimate(), &Clock(Cell::new(50_001)), |_| Ok(()))
        .unwrap();
    drop(r);
    f.binding.receiver_session = SessionId::random().unwrap();
    let mut r =
        DeliveryReceiver::new(DurableAdmission::open(dir.path(), f.receiver(0), 2).unwrap());
    r.activate_authenticated_source(SessionBinding {
        group: f.binding.group,
        membership: f.binding.membership,
        source: f.source.id(),
        session: f.binding.source_session,
    })
    .unwrap();
    assert_eq!(
        r.receive(&v, &f.estimate(), &Clock(Cell::new(50_001)), |_| panic!(
            "restart replay"
        )),
        Ok(DeliveryDecision::RetiredSequence)
    );
}

use super::*;
use crate::freshness::tests::Fixture;
use std::cell::Cell;
struct Clock(Cell<u64>);
impl BootClock for Clock {
    fn now_ms(&self) -> Result<u64, ClockError> {
        Ok(self.0.get())
    }
}
fn setup() -> (tempfile::TempDir, Fixture, BufferedDelivery) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let f = Fixture::new();
    let mut b = BufferedDelivery::new(
        DeliveryReceiver::new(DurableAdmission::initialize(dir.path(), f.receiver(0), 2).unwrap()),
        1000,
    )
    .unwrap();
    b.activate_authenticated_source(SessionBinding {
        group: f.binding.group,
        membership: f.binding.membership,
        source: f.source.id(),
        session: f.binding.source_session,
    })
    .unwrap();
    (dir, f, b)
}
#[test]
fn idle_keeps_only_latest_and_resumes_without_new_ttl_or_session() {
    let _l = STATE_TEST_LOCK.lock().unwrap();
    let (_d, f, mut b) = setup();
    let clock = Clock(Cell::new(50_001));
    let estimate = f.estimate();
    b.set_idle(true).unwrap();
    for n in 1..=3 {
        let e = f.event(n, n, 60_000, 1);
        assert_eq!(
            b.receive(&e, &f.source.public_key(), &estimate, &clock, |_| panic!(
                "Idle write"
            )),
            Ok(DeliveryDecision::BufferedIdle {
                envelope_hash: e.envelope_hash()
            })
        );
    }
    assert_eq!(b.reserve_publication(), Err(BufferError::Idle));
    clock.0.set(75_000);
    b.set_idle(false).unwrap();
    let mut count = 0;
    assert!(matches!(
        b.retry_pending(&estimate, &clock, |_| {
            count += 1;
            Ok(())
        }),
        Ok(Some(DeliveryDecision::Applied { .. }))
    ));
    assert_eq!(count, 1);
    assert_eq!(b.pending_source(), None);
}
#[test]
fn expiry_close_and_invalid_inputs_never_replay_payload() {
    let _l = STATE_TEST_LOCK.lock().unwrap();
    let (_d, f, mut b) = setup();
    let clock = Clock(Cell::new(50_001));
    let estimate = f.estimate();
    let e = f.event(1, 1, 60_000, 1);
    b.set_idle(true).unwrap();
    b.receive(&e, &f.source.public_key(), &estimate, &clock, |_| panic!())
        .unwrap();
    assert!(
        b.receive(&e, &[9; 32], &estimate, &clock, |_| panic!())
            .is_err()
    );
    assert!(b.pending_source().is_some());
    clock.0.set(80_000);
    b.maintain(&estimate, &clock).unwrap();
    assert_eq!(b.pending_source(), None);
    b.set_idle(false).unwrap();
    assert_eq!(b.retry_pending(&estimate, &clock, |_| panic!()), Ok(None));
    b.close();
    assert_eq!(
        b.receive(&e, &f.source.public_key(), &estimate, &clock, |_| panic!()),
        Err(BufferError::Closed)
    );
}
#[test]
fn failed_write_retries_identical_envelope_and_capacity_precedes_mutation() {
    let _l = STATE_TEST_LOCK.lock().unwrap();
    let (_d, f, mut b) = setup();
    let clock = Clock(Cell::new(50_001));
    let estimate = f.estimate();
    let e = f.event(1, 1, 60_000, 1);
    assert_eq!(
        b.receive(&e, &f.source.public_key(), &estimate, &clock, |_| Err(())),
        Err(BufferError::Delivery(DeliveryError::ApplyFailed))
    );
    let wire = b.pending.as_ref().unwrap().wire.to_vec();
    assert!(matches!(
        b.retry_pending(&estimate, &clock, |_| Ok(())),
        Ok(Some(DeliveryDecision::Applied { .. }))
    ));
    assert_eq!(wire, e.to_wire());
    let (_d, f, mut b) = setup();
    b.limit = 223;
    assert_eq!(
        b.receive(
            &f.event(100, 100, 60_000, 1),
            &f.source.public_key(),
            &f.estimate(),
            &clock,
            |_| panic!()
        ),
        Err(BufferError::Capacity)
    );
    assert_eq!(b.delivery.reserve_publication(), Ok((1, 1)));
}

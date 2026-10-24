use super::*;
use crate::{
    events::{EventContext, SignedTextEvent},
    freshness::Freshness,
};
fn event(source: &DeviceIdentity) -> SignedTextEvent {
    SignedTextEvent::sign(
        EventContext {
            group: [1; 32],
            membership: [2; 32],
            session: SessionId([3; 16]),
            sequence: 1,
            lamport: 1,
            created_unix_ms: 1,
            created_boot_ms: 1,
            ttl_ms: 30_000,
        },
        "synthetic receipt test",
        source,
    )
    .unwrap()
}
fn receipt(
    event: &VerifiedTextEvent<'_>,
    receiver: &DeviceIdentity,
    session: SessionId,
) -> SignedApplicationReceipt {
    SignedApplicationReceipt::sign_applied(
        event,
        DeliveryDecision::Applied {
            envelope_hash: event.envelope_hash(),
        },
        receiver,
        session,
    )
    .unwrap()
}
#[test]
fn every_byte_tampering_wrong_key_and_noncanonical_wire_fail() {
    let s = DeviceIdentity::generate().unwrap();
    let r = DeviceIdentity::generate().unwrap();
    let e = event(&s);
    let v = e.verify_with_trusted_key(&s.public_key()).unwrap();
    let a = receipt(&v, &r, SessionId([4; 16]));
    let bytes = a.to_wire();
    assert_eq!(bytes.len(), RECEIPT_BYTES);
    let parsed = SignedApplicationReceipt::from_wire(&bytes).unwrap();
    assert_eq!(
        parsed
            .verify_with_trusted_key(&r.public_key())
            .unwrap()
            .context(),
        a.context()
    );
    for index in 0..bytes.len() {
        let mut b = bytes.clone();
        b[index] ^= 1;
        if let Ok(a) = SignedApplicationReceipt::from_wire(&b) {
            assert!(a.verify_with_trusted_key(&r.public_key()).is_err());
        }
    }
    assert!(a.verify_with_trusted_key(&s.public_key()).is_err());
    assert!(SignedApplicationReceipt::from_wire(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = bytes;
    trailing.push(0);
    assert!(SignedApplicationReceipt::from_wire(&trailing).is_err());
}
#[test]
fn non_application_decisions_and_wrong_hash_cannot_be_signed() {
    let s = DeviceIdentity::generate().unwrap();
    let r = DeviceIdentity::generate().unwrap();
    let e = event(&s);
    let v = e.verify_with_trusted_key(&s.public_key()).unwrap();
    for d in [
        DeliveryDecision::BufferedIdle {
            envelope_hash: v.envelope_hash(),
        },
        DeliveryDecision::OlderCandidate,
        DeliveryDecision::RetiredSequence,
        DeliveryDecision::Skipped(Freshness::Expired),
    ] {
        assert!(matches!(
            SignedApplicationReceipt::sign_applied(&v, d, &r, SessionId([4; 16])),
            Err(ReceiptError::NotApplied)
        ));
    }
    assert!(matches!(
        SignedApplicationReceipt::sign_applied(
            &v,
            DeliveryDecision::Applied {
                envelope_hash: [9; 32]
            },
            &r,
            SessionId([4; 16])
        ),
        Err(ReceiptError::WrongContext)
    ));
}
#[test]
fn neighbor_ack_does_not_confirm_other_targets_and_forwarded_original_is_accepted() {
    let a = DeviceIdentity::generate().unwrap();
    let b = DeviceIdentity::generate().unwrap();
    let c = DeviceIdentity::generate().unwrap();
    let e = event(&a);
    let v = e.verify_with_trusted_key(&a.public_key()).unwrap();
    let session = SessionId([4; 16]);
    let mut ledger = DeliveryTracker::new(
        &v,
        &[
            ExpectedReceiver {
                device: b.id(),
                session,
            },
            ExpectedReceiver {
                device: c.id(),
                session,
            },
        ],
    )
    .unwrap();
    let br = receipt(&v, &b, session);
    let bv = br.verify_with_trusted_key(&b.public_key()).unwrap();
    assert_eq!(ledger.observe(&bv), Ok(ReceiptDecision::New));
    assert_eq!(ledger.observe(&bv), Ok(ReceiptDecision::Duplicate));
    assert!(!ledger.progress().complete());
    assert_eq!(ledger.is_applied(c.id()), Some(false));
    // B cannot replace C's signature or count its own ACK again on C's behalf.
    assert!(br.verify_with_trusted_key(&c.public_key()).is_err());
    let cr = receipt(&v, &c, session);
    let original = cr.to_wire();
    let forwarded = SignedApplicationReceipt::from_wire(&original).unwrap();
    assert_eq!(forwarded.to_wire(), original);
    assert_eq!(
        ledger.observe(&forwarded.verify_with_trusted_key(&c.public_key()).unwrap()),
        Ok(ReceiptDecision::New)
    );
    assert!(ledger.progress().complete());
}
#[test]
fn wrong_receiver_session_event_membership_and_source_do_not_mutate_ledger() {
    let s = DeviceIdentity::generate().unwrap();
    let r = DeviceIdentity::generate().unwrap();
    let other = DeviceIdentity::generate().unwrap();
    let e = event(&s);
    let v = e.verify_with_trusted_key(&s.public_key()).unwrap();
    let session = SessionId([4; 16]);
    let mut ledger = DeliveryTracker::new(
        &v,
        &[ExpectedReceiver {
            device: r.id(),
            session,
        }],
    )
    .unwrap();
    for (who, sess) in [(&r, SessionId([5; 16])), (&other, session)] {
        let a = receipt(&v, who, sess);
        assert!(
            ledger
                .observe(&a.verify_with_trusted_key(&who.public_key()).unwrap())
                .is_err()
        );
    }
    for change in 0..5 {
        let mut context = e.context();
        match change {
            0 => context.group = [9; 32],
            1 => context.membership = [9; 32],
            2 => context.session = SessionId([9; 16]),
            3 => context.sequence = 2,
            _ => context.lamport = 2,
        }
        let other_event = SignedTextEvent::sign(context, "other", &s).unwrap();
        let v = other_event
            .verify_with_trusted_key(&s.public_key())
            .unwrap();
        let a = receipt(&v, &r, session);
        assert_eq!(
            ledger.observe(&a.verify_with_trusted_key(&r.public_key()).unwrap()),
            Err(ReceiptError::WrongContext)
        );
    }
    assert_eq!(
        ledger.progress(),
        DeliveryProgress {
            expected: 1,
            applied: 0
        }
    );
}
#[test]
fn bounded_targets_are_fixed_and_duplicate_devices_rejected() {
    let s = DeviceIdentity::generate().unwrap();
    let e = event(&s);
    let v = e.verify_with_trusted_key(&s.public_key()).unwrap();
    let target = ExpectedReceiver {
        device: s.id(),
        session: SessionId([4; 16]),
    };
    assert!(matches!(
        DeliveryTracker::new(&v, &[]),
        Err(ReceiptError::Capacity)
    ));
    assert!(matches!(
        DeliveryTracker::new(&v, &vec![target; 1025]),
        Err(ReceiptError::Capacity)
    ));
    assert!(matches!(
        DeliveryTracker::new(&v, &[target, target]),
        Err(ReceiptError::DuplicateTarget)
    ));
}

#[test]
fn other_source_and_late_original_receipts_are_distinct() {
    let source = DeviceIdentity::generate().unwrap();
    let other = DeviceIdentity::generate().unwrap();
    let receiver = DeviceIdentity::generate().unwrap();
    let e = event(&source);
    let v = e.verify_with_trusted_key(&source.public_key()).unwrap();
    let session = SessionId([4; 16]);
    let mut ledger = DeliveryTracker::new(
        &v,
        &[ExpectedReceiver {
            device: receiver.id(),
            session,
        }],
    )
    .unwrap();
    let other_event = event(&other);
    let other_verified = other_event
        .verify_with_trusted_key(&other.public_key())
        .unwrap();
    let wrong = receipt(&other_verified, &receiver, session);
    assert_eq!(
        ledger.observe(
            &wrong
                .verify_with_trusted_key(&receiver.public_key())
                .unwrap()
        ),
        Err(ReceiptError::WrongContext)
    );
    // TTL governs text application, not arrival of an original historical ACK.
    // The ledger intentionally has no current-time input or timeout-to-success transition.
    let original = receipt(&v, &receiver, session).to_wire();
    let delayed = SignedApplicationReceipt::from_wire(&original).unwrap();
    assert_eq!(
        ledger.observe(
            &delayed
                .verify_with_trusted_key(&receiver.public_key())
                .unwrap()
        ),
        Ok(ReceiptDecision::New)
    );
    assert!(ledger.progress().complete());
}

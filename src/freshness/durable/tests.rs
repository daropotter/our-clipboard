// Forked crash helpers can briefly inherit other test threads' locks before exec.
// Serialize this module so a test never mistakes that inheritance for a second owner.
use super::super::STATE_TEST_LOCK as SERIAL;
fn private_directory() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    d
}
use super::*;
use crate::events::{EventContext, SignedTextEvent};
struct Fixture {
    source: DeviceIdentity,
    receiver: DeviceId,
    session: SessionId,
}
impl Fixture {
    fn new() -> Self {
        Self {
            source: DeviceIdentity::generate().unwrap(),
            receiver: DeviceIdentity::generate().unwrap().id(),
            session: SessionId([1; 16]),
        }
    }
    fn gate(&self) -> ReceiverSession {
        ReceiverSession::after_authenticated_handshake(
            ReceiverBinding {
                group: [1; 32],
                membership: [2; 32],
                receiver: self.receiver,
                session: self.session,
            },
            0,
            30_000,
        )
        .unwrap()
    }
    fn source(&self) -> SessionBinding {
        SessionBinding {
            group: [1; 32],
            membership: [2; 32],
            source: self.source.id(),
            session: SessionId([3; 16]),
        }
    }
    fn estimate(&self) -> ClockEstimate {
        let p = ClockProbe::start(
            ClockBinding {
                group: [1; 32],
                membership: [2; 32],
                source: self.source.id(),
                source_session: self.source().session,
                receiver: self.receiver,
                receiver_session: self.session,
            },
            40_000,
        )
        .unwrap();
        let r = ClockReply::sign(p.request(), &self.source, 50_000, 50_000).unwrap();
        p.finish(
            &r,
            &self.source.public_key(),
            40_000,
            ClockPolicy::new(100, 120_000, Some(0), Some(0)).unwrap(),
        )
        .unwrap()
    }
    fn event(&self, sequence: u64, lamport: u64, boot: u64) -> SignedTextEvent {
        SignedTextEvent::sign(
            EventContext {
                group: [1; 32],
                membership: [2; 32],
                session: self.source().session,
                sequence,
                lamport,
                created_boot_ms: boot,
                created_unix_ms: 1,
                ttl_ms: 30_000,
            },
            "synthetic payload never stored",
            &self.source,
        )
        .unwrap()
    }
    fn restart(&mut self) {
        self.session = SessionId::random().unwrap();
    }
}
#[test]
fn restart_keeps_watermarks_and_requires_reauthentication() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let d = private_directory();
    let mut f = Fixture::new();
    let mut s = DurableAdmission::initialize(d.path(), f.gate(), 2).unwrap();
    s.activate_authenticated_source(f.source()).unwrap();
    let event = f.event(1, 10, 60_000);
    let v = event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    assert_eq!(
        s.admit(&v, &f.estimate(), 50_001),
        Ok(AdmissionDecision::WinnerSelected)
    );
    assert_eq!(s.reserve_publication(), Ok((1, 12)));
    drop(s);
    f.restart();
    let mut s = DurableAdmission::open(d.path(), f.gate(), 2).unwrap();
    assert_eq!(s.clock(), 12);
    assert_eq!(s.winner_source(), Some(f.source.id()));
    assert_eq!(
        s.admit(&v, &f.estimate(), 50_001),
        Err(StateError::Admission(AdmissionError::Replay(
            EventError::UnknownSession
        )))
    );
    s.activate_authenticated_source(f.source()).unwrap();
    assert_eq!(
        s.admit(&v, &f.estimate(), 50_001),
        Ok(AdmissionDecision::RetiredSequence)
    );
    assert_eq!(s.reserve_publication(), Ok((2, 13)));
    let later = f.event(2, 14, 60_000);
    assert_eq!(
        s.admit(
            &later
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &f.estimate(),
            50_001
        ),
        Ok(AdmissionDecision::WinnerSelected)
    );
    drop(s);
    let bytes = fs::read(d.path().join("admission.sqlite3")).unwrap();
    assert!(
        !bytes
            .windows(b"synthetic payload never stored".len())
            .any(|w| w == b"synthetic payload never stored")
    );
}
#[test]
fn missing_corrupt_wrong_scope_and_existing_state_never_reset() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let d = private_directory();
    let mut f = Fixture::new();
    assert!(matches!(
        DurableAdmission::open(d.path(), f.gate(), 2),
        Err(StateError::Missing)
    ));
    let s = DurableAdmission::initialize(d.path(), f.gate(), 2).unwrap();
    assert!(matches!(
        DurableAdmission::open(d.path(), f.gate(), 2),
        Err(StateError::Busy)
    ));
    drop(s);
    assert!(matches!(
        DurableAdmission::initialize(d.path(), f.gate(), 2),
        Err(StateError::AlreadyExists)
    ));
    assert!(matches!(
        DurableAdmission::open(d.path(), f.gate(), 2),
        Err(StateError::WrongScope)
    ));
    f.restart();
    let device = f.receiver;
    f.receiver = DeviceId([8; 32]);
    assert_eq!(
        DurableAdmission::open(d.path(), f.gate(), 2).err(),
        Some(StateError::WrongScope)
    );
    f.receiver = device;
    assert!(matches!(
        DurableAdmission::open(d.path(), f.gate(), 1),
        Err(StateError::InvalidState)
    ));
    fs::write(d.path().join("admission.sqlite3"), b"corrupt").unwrap();
    assert!(DurableAdmission::open(d.path(), f.gate(), 2).is_err());
    assert_eq!(
        fs::read(d.path().join("admission.sqlite3")).unwrap(),
        b"corrupt"
    );
}
#[test]
fn write_failure_is_transactional_and_poisoned_until_reopen() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let d = private_directory();
    let mut f = Fixture::new();
    let mut s = DurableAdmission::initialize(d.path(), f.gate(), 2).unwrap();
    s.activate_authenticated_source(f.source()).unwrap();
    // A deterministic database write failure after opening, without changing host permissions.
    s.connection.execute_batch("PRAGMA query_only=ON").unwrap();
    let event = f.event(1, 10, 60_000);
    let v = event
        .verify_with_trusted_key(&f.source.public_key())
        .unwrap();
    assert_eq!(s.admit(&v, &f.estimate(), 50_001), Err(StateError::Storage));
    assert_eq!(s.clock(), 0);
    assert_eq!(s.winner_source(), None);
    assert_eq!(s.reserve_publication(), Err(StateError::Poisoned));
    assert_eq!(
        s.activate_authenticated_source(f.source()),
        Err(StateError::Poisoned)
    );
    drop(s);
    f.restart();
    let mut s = DurableAdmission::open(d.path(), f.gate(), 2).unwrap();
    s.activate_authenticated_source(f.source()).unwrap();
    assert_eq!(
        s.admit(&v, &f.estimate(), 50_001),
        Ok(AdmissionDecision::WinnerSelected)
    );
}
#[test]
fn rejected_event_does_not_write_even_when_storage_is_unwritable() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let d = private_directory();
    let f = Fixture::new();
    let mut s = DurableAdmission::initialize(d.path(), f.gate(), 1).unwrap();
    s.activate_authenticated_source(f.source()).unwrap();
    s.connection.execute_batch("PRAGMA query_only=ON").unwrap();
    let event = f.event(1, 50, 60_000);
    assert_eq!(
        s.admit(
            &event
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &f.estimate(),
            90_000
        ),
        Ok(AdmissionDecision::Skipped(Freshness::Expired))
    );
    assert_eq!(s.clock(), 0);
    assert!(!s.poisoned);
}
#[test]
fn restart_retains_source_clock_across_session_change_and_capacity() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let d = private_directory();
    let mut f = Fixture::new();
    let mut s = DurableAdmission::initialize(d.path(), f.gate(), 1).unwrap();
    s.activate_authenticated_source(f.source()).unwrap();
    let event = f.event(1, 50, 60_000);
    s.admit(
        &event
            .verify_with_trusted_key(&f.source.public_key())
            .unwrap(),
        &f.estimate(),
        50_001,
    )
    .unwrap();
    drop(s);
    f.restart();
    let mut s = DurableAdmission::open(d.path(), f.gate(), 1).unwrap();
    let mut binding = f.source();
    binding.session = SessionId([8; 16]);
    s.activate_authenticated_source(binding).unwrap();
    assert_eq!(
        s.state.replay.check(
            &event
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap()
        ),
        Err(EventError::RetiredSession)
    );
    let new_source = SessionBinding {
        source: DeviceId([9; 32]),
        ..binding
    };
    assert_eq!(
        s.activate_authenticated_source(new_source),
        Err(StateError::Admission(AdmissionError::Replay(
            EventError::Capacity
        )))
    );
    let mut c = event.context();
    c.session = binding.session;
    c.sequence = 1;
    c.lamport = 1;
    let small = SignedTextEvent::sign(c, "synthetic", &f.source).unwrap();
    assert_eq!(
        s.state.replay.check(
            &small
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap()
        ),
        Err(EventError::ClockNotAdvanced)
    );
}
#[test]
fn full_unsigned_counter_range_and_overflow_fail_without_mutation() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let d = private_directory();
    let mut f = Fixture::new();
    let mut s = DurableAdmission::initialize(d.path(), f.gate(), 1).unwrap();
    s.state.clock = LamportClock::from_watermark(u64::MAX - 1);
    s.sequence = u64::MAX - 1;
    s.save(&s.state.clone(), s.sequence, 1).unwrap();
    assert_eq!(s.reserve_publication(), Ok((u64::MAX, u64::MAX)));
    assert_eq!(s.reserve_publication(), Err(StateError::SequenceExhausted));
    drop(s);
    f.restart();
    let mut s = DurableAdmission::open(d.path(), f.gate(), 1).unwrap();
    assert_eq!(s.clock(), u64::MAX);
    assert_eq!(s.reserve_publication(), Err(StateError::SequenceExhausted));
}
#[cfg(unix)]
#[test]
fn unsafe_directories_and_symlinks_are_rejected() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    use std::os::unix::fs::{PermissionsExt, symlink};
    let d = private_directory();
    let f = Fixture::new();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        DurableAdmission::initialize(d.path(), f.gate(), 1),
        Err(StateError::InvalidDirectory)
    ));
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let other = private_directory();
    let alias = d.path().join("alias");
    symlink(other.path(), &alias).unwrap();
    assert!(matches!(
        DurableAdmission::initialize(&alias, f.gate(), 1),
        Err(StateError::InvalidDirectory)
    ));
    symlink(
        other.path().join("missing"),
        d.path().join("admission.sqlite3"),
    )
    .unwrap();
    assert!(DurableAdmission::open(d.path(), f.gate(), 1).is_err());
    assert!(!other.path().join("missing").exists());
}

#[test]
#[ignore = "subprocess helper; invoked by crash_recovery_preserves_commits_and_rolls_back_incomplete_transactions"]
fn crash_helper() {
    let path = std::env::var_os("OUR_CLIPBOARD_STATE_TEST_DIRECTORY").expect("test directory");
    let mut f = Fixture::new();
    f.receiver = DeviceId([7; 32]);
    f.session = SessionId([2; 16]);
    let mut s = DurableAdmission::open(Path::new(&path), f.gate(), 1).unwrap();
    assert_eq!(s.reserve_publication(), Ok((1, 1)));
    let tx = s.connection.transaction().unwrap();
    tx.execute(
        "UPDATE local SET sequence=?1,clock=?1 WHERE id=1",
        [99_u64.to_be_bytes().as_slice()],
    )
    .unwrap();
    // Force uncommitted dirty pages through SQLite's pager to exercise hot-journal recovery.
    tx.cache_flush().unwrap();
    assert!(Path::new(&path).join("admission.sqlite3-journal").is_file());
    // Skip all Rust destructors, including transaction rollback and file-lock release.
    // Recovery in the next process must use SQLite's hot journal and OS lock cleanup.
    std::process::exit(0);
}
#[test]
fn crash_recovery_preserves_commits_and_rolls_back_incomplete_transactions() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let d = private_directory();
    let mut f = Fixture::new();
    f.receiver = DeviceId([7; 32]);
    drop(DurableAdmission::initialize(d.path(), f.gate(), 1).unwrap());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "freshness::durable::tests::crash_helper",
            "--ignored",
        ])
        .env("OUR_CLIPBOARD_STATE_TEST_DIRECTORY", d.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "crash helper failed; details omitted"
    );
    assert!(d.path().join("admission.sqlite3-journal").is_file());
    f.session = SessionId([3; 16]);
    let mut s = DurableAdmission::open(d.path(), f.gate(), 1).unwrap();
    assert_eq!(s.clock(), 1);
    assert_eq!(s.reserve_publication(), Ok((2, 2)));
}

#[test]
fn malformed_version_counters_and_partial_winner_metadata_fail_closed() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for sql in [
        "UPDATE local SET version=2",
        "UPDATE local SET clock=x'00'",
        "UPDATE local SET winner_hash=zeroblob(32)",
        "UPDATE local SET sequence=x'ffffffffffffffff'",
    ] {
        let d = private_directory();
        let mut f = Fixture::new();
        let s = DurableAdmission::initialize(d.path(), f.gate(), 1).unwrap();
        s.connection.execute_batch(sql).unwrap();
        drop(s);
        f.restart();
        assert_eq!(
            DurableAdmission::open(d.path(), f.gate(), 1).err(),
            Some(StateError::InvalidState)
        );
    }
}

#[test]
fn local_copy_is_selected_before_publication_and_survives_restart() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = private_directory();
    let identity = DeviceIdentity::generate().unwrap();
    let mut f = Fixture::new();
    f.receiver = identity.id();
    let mut state = DurableAdmission::initialize(dir.path(), f.gate(), 1).unwrap();
    state.activate_authenticated_source(f.source()).unwrap();
    let remote = f.event(1, 1, 60_000);
    state
        .admit(
            &remote
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &f.estimate(),
            50_001,
        )
        .unwrap();
    let (sequence, lamport) = state.reserve_publication().unwrap();
    let c = EventContext {
        group: [1; 32],
        membership: [2; 32],
        session: f.session,
        sequence,
        lamport,
        created_unix_ms: 1,
        created_boot_ms: 50_000,
        ttl_ms: 30_000,
    };
    let local = SignedTextEvent::sign(c, "new local copy", &identity).unwrap();
    let v = local
        .verify_with_trusted_key(&identity.public_key())
        .unwrap();
    state.record_local_event(&v).unwrap();
    state.record_local_event(&v).unwrap();
    assert_eq!(state.winner_source(), Some(identity.id()));
    let older = f.event(2, 2, 60_000);
    assert_eq!(
        state.admit(
            &older
                .verify_with_trusted_key(&f.source.public_key())
                .unwrap(),
            &f.estimate(),
            50_001
        ),
        Ok(AdmissionDecision::OlderCandidate)
    );
    let conflict = SignedTextEvent::sign(c, "different local copy", &identity).unwrap();
    assert!(
        state
            .record_local_event(
                &conflict
                    .verify_with_trusted_key(&identity.public_key())
                    .unwrap()
            )
            .is_err()
    );
    drop(state);
    f.restart();
    let state = DurableAdmission::open(dir.path(), f.gate(), 1).unwrap();
    assert_eq!(state.winner_source(), Some(identity.id()));
}
#[test]
fn unreserved_or_foreign_local_event_cannot_change_winner() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = private_directory();
    let f = Fixture::new();
    let mut s = DurableAdmission::initialize(dir.path(), f.gate(), 1).unwrap();
    let e = f.event(1, 1, 60_000);
    assert!(
        s.record_local_event(&e.verify_with_trusted_key(&f.source.public_key()).unwrap())
            .is_err()
    );
    assert_eq!(s.clock(), 0);
    assert_eq!(s.winner_source(), None);
}

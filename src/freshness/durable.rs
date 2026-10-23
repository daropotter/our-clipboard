//! Transactional metadata only; no clipboard text or private signing keys.
use super::*;
use rusqlite::{Connection, OpenFlags, params};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};
const MAX_SOURCES: usize = 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    Storage,
    InvalidState,
    WrongScope,
    Busy,
    InvalidDirectory,
    AlreadyExists,
    Missing,
    Poisoned,
    Admission(AdmissionError),
    SequenceExhausted,
}
impl StateError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Storage => {
                "Durable state could not be committed; reopen and recover before continuing."
            }
            Self::InvalidState => "Durable state is corrupt or unsupported; it was not reset.",
            Self::WrongScope => {
                "Durable state belongs to another device/group or requires a fresh receiver session."
            }
            Self::Busy => "Another process holds the durable state lock.",
            Self::InvalidDirectory => "Durable state requires a private, non-symlink directory.",
            Self::AlreadyExists => "Durable state already exists; open it instead of initializing.",
            Self::Missing => {
                "Durable state is missing; recover it rather than silently resetting counters."
            }
            Self::Poisoned => {
                "Durable state is unavailable after a storage failure; reopen before continuing."
            }
            Self::Admission(_) => "Event admission or logical clock validation failed.",
            Self::SequenceExhausted => "The durable publication sequence is exhausted.",
        }
    }
}
/// One owner per dedicated directory. Initialize explicitly once; open never recreates state.
/// Successful decisions/reservations are committed before they become visible to the caller.
pub struct DurableAdmission {
    connection: Connection,
    _lock: File,
    state: Admission,
    sequence: u64,
    capacity: usize,
    poisoned: bool,
}
impl DurableAdmission {
    pub fn initialize(
        directory: &Path,
        receiver: ReceiverSession,
        capacity: usize,
    ) -> Result<Self, StateError> {
        if capacity == 0 || capacity > MAX_SOURCES {
            return Err(StateError::InvalidState);
        }
        let lock = lock(directory)?;
        let path = directory.join("admission.sqlite3");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                StateError::AlreadyExists
            } else {
                StateError::Storage
            }
        })?;
        let connection = connect(&path)?;
        connection
            .execute_batch(
                "CREATE TABLE local (
                id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL,
                group_id BLOB NOT NULL, receiver BLOB NOT NULL, receiver_session BLOB NOT NULL,
                capacity INTEGER NOT NULL, sequence BLOB NOT NULL, clock BLOB NOT NULL,
                winner_clock BLOB, winner_source BLOB, winner_hash BLOB
            );
            CREATE TABLE sources (
                source BLOB PRIMARY KEY, membership BLOB NOT NULL, session BLOB NOT NULL,
                sequence BLOB NOT NULL, lamport BLOB NOT NULL
            );",
            )
            .map_err(|_| StateError::Storage)?;
        let mut store = Self {
            connection,
            _lock: lock,
            state: Admission::new(receiver, capacity, LamportClock::default()),
            sequence: 0,
            capacity,
            poisoned: false,
        };
        store.save(&store.state.clone(), 0, capacity)?;
        // Persist the new database directory entry as well as SQLite's EXTRA-sync transaction.
        #[cfg(unix)]
        File::open(directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| StateError::Storage)?;
        Ok(store)
    }
    pub fn open(
        directory: &Path,
        receiver: ReceiverSession,
        capacity: usize,
    ) -> Result<Self, StateError> {
        if capacity == 0 || capacity > MAX_SOURCES {
            return Err(StateError::InvalidState);
        }
        let lock = lock(directory)?;
        let path = directory.join("admission.sqlite3");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(StateError::Missing),
            Ok(m) if m.file_type().is_file() => {}
            _ => return Err(StateError::InvalidState),
        }
        let connection = connect(&path)?;
        let integrity: String = connection
            .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
            .map_err(|_| StateError::InvalidState)?;
        if integrity != "ok" {
            return Err(StateError::InvalidState);
        }
        let mut statement = connection
            .prepare(
                "SELECT version,group_id,receiver,receiver_session,capacity,sequence,clock,
                    winner_clock,winner_source,winner_hash FROM local WHERE id=1",
            )
            .map_err(|_| StateError::InvalidState)?;
        let mut rows = statement.query([]).map_err(|_| StateError::InvalidState)?;
        let row = rows
            .next()
            .map_err(|_| StateError::InvalidState)?
            .ok_or(StateError::InvalidState)?;
        let version: i64 = row.get(0).map_err(|_| StateError::InvalidState)?;
        let group = blob::<32>(row, 1)?;
        let device = blob::<32>(row, 2)?;
        let previous_session = blob::<16>(row, 3)?;
        let saved_capacity: u32 = row.get(4).map_err(|_| StateError::InvalidState)?;
        if version != 1 || saved_capacity as usize != capacity {
            return Err(StateError::InvalidState);
        }
        if group != receiver.binding.group
            || device != receiver.binding.receiver.0
            || previous_session == receiver.binding.session.0
        {
            return Err(StateError::WrongScope);
        }
        let sequence = u64::from_be_bytes(blob(row, 5)?);
        let clock = u64::from_be_bytes(blob(row, 6)?);
        let mut state = Admission::new(receiver, capacity, LamportClock::from_watermark(clock));
        let winner_clock = optional_blob::<8>(row, 7)?;
        let winner_source = optional_blob::<32>(row, 8)?;
        let winner_hash = optional_blob::<32>(row, 9)?;
        match (winner_clock, winner_source, winner_hash) {
            (None, None, None) => {}
            (Some(c), Some(source), Some(hash)) => {
                let c = u64::from_be_bytes(c);
                if c == 0 || c >= clock {
                    return Err(StateError::InvalidState);
                }
                state.winner = Some(admission::Winner {
                    order: (c, DeviceId(source)),
                    hash,
                });
            }
            _ => return Err(StateError::InvalidState),
        }
        drop(rows);
        drop(statement);
        if sequence > clock {
            return Err(StateError::InvalidState);
        }
        let mut statement = connection
            .prepare("SELECT source,membership,session,sequence,lamport FROM sources LIMIT 1025")
            .map_err(|_| StateError::InvalidState)?;
        let mut rows = statement.query([]).map_err(|_| StateError::InvalidState)?;
        while let Some(row) = rows.next().map_err(|_| StateError::InvalidState)? {
            let source = DeviceId(blob(row, 0)?);
            let membership = blob(row, 1)?;
            let session = SessionId(blob(row, 2)?);
            let seq = u64::from_be_bytes(blob(row, 3)?);
            let lamport = u64::from_be_bytes(blob(row, 4)?);
            if (lamport >= clock && lamport != 0) || (seq != 0 && lamport == 0) {
                return Err(StateError::InvalidState);
            }
            state
                .replay
                .restore_watermark(
                    SessionBinding {
                        group,
                        membership,
                        source,
                        session,
                    },
                    seq,
                    lamport,
                )
                .map_err(|_| StateError::InvalidState)?;
        }
        drop(rows);
        drop(statement);
        let mut store = Self {
            connection,
            _lock: lock,
            state,
            sequence,
            capacity,
            poisoned: false,
        };
        store.save(&store.state.clone(), sequence, capacity)?;
        Ok(store)
    }
    pub fn clock(&self) -> u64 {
        self.state.clock()
    }
    pub fn winner_source(&self) -> Option<DeviceId> {
        self.state.winner_source()
    }
    fn healthy(&self) -> Result<(), StateError> {
        if self.poisoned {
            Err(StateError::Poisoned)
        } else {
            Ok(())
        }
    }
    /// Reserve before signing/sending. Crashes may leave gaps; reserved numbers are never reused.
    pub fn reserve_publication(&mut self) -> Result<(u64, u64), StateError> {
        self.healthy()?;
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(StateError::SequenceExhausted)?;
        let mut next = self.state.clone();
        let clock = next.clock.tick().map_err(StateError::Admission)?;
        self.commit(next, sequence)?;
        Ok((sequence, clock))
    }
    pub fn activate_authenticated_source(
        &mut self,
        binding: SessionBinding,
    ) -> Result<(), StateError> {
        self.healthy()?;
        let mut next = self.state.clone();
        next.activate_authenticated_source(binding)
            .map_err(StateError::Admission)?;
        self.commit(next, self.sequence)
    }
    pub fn admit(
        &mut self,
        event: &VerifiedTextEvent<'_>,
        estimate: &ClockEstimate,
        now_ms: u64,
    ) -> Result<AdmissionDecision, StateError> {
        self.healthy()?;
        let mut next = self.state.clone();
        let decision = next
            .admit(event, estimate, now_ms)
            .map_err(StateError::Admission)?;
        if matches!(
            decision,
            AdmissionDecision::WinnerSelected | AdmissionDecision::OlderCandidate
        ) {
            self.commit(next, self.sequence)?;
        }
        Ok(decision)
    }
    fn commit(&mut self, next: Admission, sequence: u64) -> Result<(), StateError> {
        let capacity = self.capacity;
        if let Err(e) = self.save(&next, sequence, capacity) {
            self.poisoned = true;
            return Err(e);
        }
        self.state = next;
        self.sequence = sequence;
        Ok(())
    }
    fn save(
        &mut self,
        state: &Admission,
        sequence: u64,
        capacity: usize,
    ) -> Result<(), StateError> {
        let tx = self
            .connection
            .transaction()
            .map_err(|_| StateError::Storage)?;
        let binding = state.receiver.binding;
        let (wc, ws, wh) = match &state.winner {
            Some(w) => (
                Some(w.order.0.to_be_bytes().to_vec()),
                Some(w.order.1.0.to_vec()),
                Some(w.hash.to_vec()),
            ),
            None => (None, None, None),
        };
        tx.execute(
            "INSERT OR REPLACE INTO local VALUES (1,1,?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                binding.group.as_slice(),
                binding.receiver.0.as_slice(),
                binding.session.0.as_slice(),
                capacity as u32,
                sequence.to_be_bytes().as_slice(),
                state.clock().to_be_bytes().as_slice(),
                wc,
                ws,
                wh
            ],
        )
        .map_err(|_| StateError::Storage)?;
        tx.execute("DELETE FROM sources", [])
            .map_err(|_| StateError::Storage)?;
        for (binding, sequence, lamport) in state.replay.watermarks() {
            tx.execute(
                "INSERT INTO sources VALUES (?1,?2,?3,?4,?5)",
                params![
                    binding.source.0.as_slice(),
                    binding.membership.as_slice(),
                    binding.session.0.as_slice(),
                    sequence.to_be_bytes().as_slice(),
                    lamport.to_be_bytes().as_slice()
                ],
            )
            .map_err(|_| StateError::Storage)?;
        }
        tx.commit().map_err(|_| StateError::Storage)?;
        Ok(())
    }
}
fn array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], StateError> {
    bytes.try_into().map_err(|_| StateError::InvalidState)
}
fn blob<const N: usize>(row: &rusqlite::Row<'_>, column: usize) -> Result<[u8; N], StateError> {
    let b = row.get_ref(column).map_err(|_| StateError::InvalidState)?;
    array(b.as_blob().map_err(|_| StateError::InvalidState)?)
}
fn optional_blob<const N: usize>(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> Result<Option<[u8; N]>, StateError> {
    match row.get_ref(column).map_err(|_| StateError::InvalidState)? {
        rusqlite::types::ValueRef::Null => Ok(None),
        rusqlite::types::ValueRef::Blob(bytes) => Ok(Some(array(bytes)?)),
        _ => Err(StateError::InvalidState),
    }
}
fn connect(path: &Path) -> Result<Connection, StateError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| StateError::Storage)?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA trusted_schema=OFF; PRAGMA fullfsync=ON;",
        )
        .map_err(|_| StateError::Storage)?;
    Ok(connection)
}
fn lock(directory: &Path) -> Result<File, StateError> {
    let metadata = fs::symlink_metadata(directory).map_err(|_| StateError::InvalidDirectory)?;
    if !metadata.file_type().is_dir() {
        return Err(StateError::InvalidDirectory);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(StateError::InvalidDirectory);
        }
    }
    let path = directory.join("admission.lock");
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_file()) {
        return Err(StateError::InvalidDirectory);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| StateError::Storage)?;
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => StateError::Busy,
        _ => StateError::Storage,
    })?;
    Ok(file)
}
#[cfg(test)]
mod tests;

/// Uses only temporary metadata and synthetic identities; never opens the user's state/store.
pub fn probe() -> Result<String, StateError> {
    let directory = tempfile::tempdir().map_err(|_| StateError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .map_err(|_| StateError::Storage)?;
    }
    let receiver = DeviceIdentity::generate()
        .map_err(|_| StateError::Storage)?
        .id();
    let binding = ReceiverBinding {
        group: [1; 32],
        membership: [2; 32],
        receiver,
        session: SessionId::random().map_err(|_| StateError::Storage)?,
    };
    let gate = |binding| {
        ReceiverSession::after_authenticated_handshake(binding, 0, 30_000)
            .map_err(|_| StateError::InvalidState)
    };
    let mut state = DurableAdmission::initialize(directory.path(), gate(binding)?, 2)?;
    if state.reserve_publication()? != (1, 1) {
        return Err(StateError::InvalidState);
    }
    drop(state);
    let restarted = ReceiverBinding {
        session: SessionId::random().map_err(|_| StateError::Storage)?,
        ..binding
    };
    let mut state = DurableAdmission::open(directory.path(), gate(restarted)?, 2)?;
    if state.reserve_publication()? != (2, 2) {
        return Err(StateError::InvalidState);
    }
    Ok("Temporary durable state reopened. Publication sequence and Lamport reservations survived restart. No clipboard text or private keys stored.".into())
}

//! One disposable transport generation; application protocol state stays in the owner loop.
use super::*;
use outbound::{RelayError, RelayLifetime, RelayParts};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    mpsc::{Receiver, SyncSender, TrySendError},
};
fn code(error: Error) -> u8 {
    match error {
        Error::Io => 1,
        Error::Authentication => 2,
        _ => 3,
    }
}
pub(super) struct Link {
    pub incoming: Receiver<Result<(Kind, Vec<u8>)>>,
    writer: SyncSender<(Kind, Vec<u8>)>,
    fault: Arc<AtomicU8>,
    _guard: Option<RelayLifetime>,
}
impl Link {
    pub fn open(
        input: impl Read + Send + 'static,
        mut output: impl Write + Send + 'static,
        own: PublicPeer,
        expected: Option<PublicPeer>,
        guard: Option<RelayLifetime>,
        control: Option<&outbound::SetupControl>,
    ) -> Result<(Self, PublicPeer)> {
        own.send(&mut output)?;
        let (hello_tx, hello_rx) = mpsc::sync_channel(1);
        let (incoming_tx, incoming) = mpsc::sync_channel(1);
        let fault = Arc::new(AtomicU8::new(0));
        let failed = fault.clone();
        thread::spawn(move || {
            let mut input = input;
            let peer = PublicPeer::receive(&mut input);
            let valid = peer.is_ok();
            if hello_tx.send(peer).is_err() || !valid {
                return;
            }
            loop {
                let frame = wire::read_any(&mut input);
                let stop = frame.is_err();
                if let Err(error) = frame.as_ref() {
                    failed.fetch_max(code(*error), Ordering::Relaxed);
                }
                if incoming_tx.send(frame).is_err() || stop {
                    break;
                }
            }
        });
        let until = control.map_or(Instant::now() + Duration::from_secs(15), |c| {
            c.until.min(Instant::now() + Duration::from_secs(15))
        });
        let peer = loop {
            if control.is_some_and(|c| c.cancel.load(Ordering::Relaxed)) || Instant::now() >= until
            {
                return Err(Error::Io);
            }
            match hello_rx.recv_timeout(Duration::from_millis(25)) {
                Ok(peer) => break peer?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Io),
            }
        };
        if expected.is_some_and(|e| !same_peer(e, peer)) {
            return Err(Error::Authentication);
        }
        let (writer, write_rx) = mpsc::sync_channel::<(Kind, Vec<u8>)>(1);
        let failed = fault.clone();
        thread::spawn(move || {
            while let Ok((kind, bytes)) = write_rx.recv() {
                if let Err(error) = wire::send(&mut output, kind, &bytes) {
                    failed.fetch_max(code(error), Ordering::Relaxed);
                    break;
                }
            }
        });
        Ok((
            Self {
                incoming,
                writer,
                fault,
                _guard: guard,
            },
            peer,
        ))
    }
    pub fn failure(&self) -> Option<Error> {
        match self.fault.load(Ordering::Relaxed) {
            0 => None,
            1 => Some(Error::Io),
            2 => Some(Error::Authentication),
            _ => Some(Error::Frame),
        }
    }
    pub fn disconnected(&self) {
        self.fault.fetch_max(1, Ordering::Relaxed);
    }
    pub fn queue(&self, kind: Kind, bytes: Vec<u8>) -> Result<bool> {
        if let Some(error) = self.failure() {
            return if error == Error::Io {
                Ok(false)
            } else {
                Err(error)
            };
        }
        match self.writer.try_send((kind, bytes)) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Disconnected(_)) => {
                self.fault.fetch_max(1, Ordering::Relaxed);
                Ok(false)
            }
        }
    }
}
fn same_peer(expected: PublicPeer, actual: PublicPeer) -> bool {
    expected.key == actual.key && expected.session == actual.session
}
pub(super) struct Reconnector {
    pub identity: RelayIdentity,
    pub peer: PublicOffer,
    pub url: String,
    pub room: Room,
    pub side: Side,
    pub token: Arc<AccessToken>,
    pub outer: Arc<ClientConfig>,
    pub cancel: Arc<AtomicBool>,
}
pub(super) struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
pub(super) enum AttemptError {
    Retry,
    Terminal(Error),
}
impl Reconnector {
    fn options(&self) -> outbound::RelayOptions<'_> {
        outbound::RelayOptions {
            url: &self.url,
            room: self.room,
            side: self.side,
            token: &self.token,
            peer: &self.peer,
            outer: self.outer.clone(),
        }
    }
    pub fn initial(&self) -> std::result::Result<RelayParts, RelayError> {
        outbound::connect(&self.identity, self.options())
    }
    pub fn cancel_on_drop(&self) -> Cancel {
        Cancel(self.cancel.clone())
    }
    pub fn spawn(
        self: &Arc<Self>,
        own: PublicPeer,
        peer: PublicPeer,
        budget: Duration,
    ) -> Receiver<std::result::Result<Link, AttemptError>> {
        let (tx, rx) = mpsc::sync_channel(1);
        let config = self.clone();
        thread::spawn(move || {
            let result = (|| {
                let control = outbound::SetupControl {
                    until: Instant::now() + budget.min(Duration::from_secs(10)),
                    cancel: config.cancel.clone(),
                    finished: Arc::new(AtomicBool::new(false)),
                };
                let (input, output, guard) = outbound::connect_controlled(
                    &config.identity,
                    config.options(),
                    Some(&control),
                )
                .map_err(attempt_error)?;
                Link::open(input, output, own, Some(peer), Some(guard), Some(&control))
                    .map(|(link, _)| {
                        control.finished.store(true, Ordering::Relaxed);
                        link
                    })
                    .map_err(|e| {
                        if e == Error::Io {
                            AttemptError::Retry
                        } else {
                            AttemptError::Terminal(e)
                        }
                    })
            })();
            let _ = tx.send(result);
        });
        rx
    }
}
fn attempt_error(error: RelayError) -> AttemptError {
    match error {
        RelayError::Transport | RelayError::Timeout | RelayError::Capacity => AttemptError::Retry,
        RelayError::Authentication | RelayError::PeerAuthentication => {
            AttemptError::Terminal(Error::Authentication)
        }
        RelayError::Configuration => AttemptError::Terminal(Error::Setup),
        RelayError::Protocol => AttemptError::Terminal(Error::Frame),
    }
}
pub(super) fn clock_stalled(now: u64, last: u64) -> bool {
    now < last || now - last > wire::CLOCK_SAMPLE_LIFETIME_MS + PROBE_TIMEOUT_MS
}
pub(super) fn delay_ms(failures: u32, jitter: u8) -> u64 {
    (250u64.saturating_mul(1u64 << failures.min(4)) + u64::from(jitter)).min(4000)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_same_public_key_and_session_can_resume() {
        let a = SigningPeer::new().unwrap().public();
        let b = SigningPeer::new().unwrap().public();
        assert!(same_peer(a, a));
        assert!(!same_peer(a, b));
        assert!(!same_peer(
            a,
            PublicPeer {
                key: a.key,
                session: b.session
            }
        ));
        assert!(!same_peer(
            a,
            PublicPeer {
                key: b.key,
                session: a.session
            }
        ));
    }
    #[test]
    fn silent_connection_loss_cannot_keep_a_stale_clock_link_alive_forever() {
        let limit = wire::CLOCK_SAMPLE_LIFETIME_MS + PROBE_TIMEOUT_MS;
        assert!(!clock_stalled(100, 100));
        assert!(!clock_stalled(100 + limit, 100));
        assert!(clock_stalled(101 + limit, 100));
        assert!(clock_stalled(99, 100));
    }
    #[test]
    fn changed_session_hello_is_rejected_before_any_frame_dispatch() {
        let own = SigningPeer::new().unwrap().public();
        let expected = SigningPeer::new().unwrap().public();
        let changed = PublicPeer {
            key: expected.key,
            session: own.session,
        };
        let mut hello = Vec::new();
        changed.send(&mut hello).unwrap();
        assert!(matches!(
            Link::open(
                std::io::Cursor::new(hello),
                Vec::new(),
                own,
                Some(expected),
                None,
                None
            ),
            Err(Error::Authentication)
        ));
    }
    #[test]
    fn retry_never_masks_auth_or_protocol_errors_and_backoff_is_bounded() {
        for error in [
            RelayError::Authentication,
            RelayError::PeerAuthentication,
            RelayError::Configuration,
            RelayError::Protocol,
        ] {
            assert!(matches!(attempt_error(error), AttemptError::Terminal(_)));
        }
        for error in [
            RelayError::Transport,
            RelayError::Timeout,
            RelayError::Capacity,
        ] {
            assert!(matches!(attempt_error(error), AttemptError::Retry));
        }
        assert_eq!(delay_ms(0, 0), 250);
        assert_eq!(delay_ms(1, 0), 500);
        assert_eq!(delay_ms(100, 255), 4000);
    }
}

//! Bounded synthetic protocol; public signing keys/sessions come from trusted setup pipes.
use super::*;
use crate::freshness::{BufferError, BufferedDelivery};
use crate::receipts::{
    DeliveryTracker, ExpectedReceiver, RECEIPT_BYTES, ReceiptDecision, SignedApplicationReceipt,
};
use crate::session::ClipboardSession;
use crate::{
    events::{EventContext, SessionBinding, SessionId, SignedTextEvent, TEXT_EVENT_WIRE_OVERHEAD},
    freshness::{
        BootClock, ClockBinding, ClockEstimate, ClockPolicy, ClockProbe, ClockReply, ClockRequest,
        DeliveryDecision, DeliveryError, DeliveryReceiver, DurableAdmission, ReceiverBinding,
        ReceiverSession, SystemBootClock,
    },
    identity::{DeviceId, DeviceIdentity},
};
pub(super) const CLOCK_MAX_RTT_MS: u64 = 100;
pub(super) const CLOCK_SAMPLE_LIFETIME_MS: u64 = 20_000;
// Avoid creating the first event exactly on the uncertain quiet-period boundary.
// Every candidate still undergoes the full interval-based freshness check.
const QUIET_BOUNDARY_MARGIN_MS: u64 = 100;
const TTL: u32 = 2000;
const GROUP: [u8; 32] = [1; 32];
const MEMBERSHIP: [u8; 32] = [2; 32];
const FRAME_MAGIC: &[u8; 5] = b"OCdf\x01";
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Request = 1,
    Reply = 2,
    Event = 3,
    Applied = 4,
    Ready = 5,
    Retry = 6,
    Rejected = 7,
}
fn valid(kind: Kind, n: usize) -> bool {
    match kind {
        Kind::Request => n == 197,
        Kind::Reply => n == 282,
        Kind::Event => (TEXT_EVENT_WIRE_OVERHEAD + 1
            ..=crate::text::MAX_TEXT_BYTES + TEXT_EVENT_WIRE_OVERHEAD)
            .contains(&n),
        Kind::Applied => n == RECEIPT_BYTES,
        Kind::Retry | Kind::Rejected => n == 32,
        Kind::Ready => n == 1,
    }
}
pub(super) fn send(stream: &mut impl Write, kind: Kind, bytes: &[u8]) -> Result<()> {
    if !valid(kind, bytes.len()) {
        return Err(Error::Frame);
    }
    stream.write_all(FRAME_MAGIC).map_err(io_error)?;
    stream.write_all(&[kind as u8]).map_err(io_error)?;
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .map_err(io_error)?;
    stream.write_all(bytes).map_err(io_error)?;
    stream.flush().map_err(io_error)
}
pub(super) fn receive(stream: &mut impl Read, kind: Kind) -> Result<Vec<u8>> {
    let mut header = [0; 10];
    stream.read_exact(&mut header).map_err(io_error)?;
    let n = u32::from_be_bytes(header[6..].try_into().map_err(|_| Error::Frame)?) as usize;
    if &header[..5] != FRAME_MAGIC || header[5] != kind as u8 || !valid(kind, n) {
        return Err(Error::Frame);
    }
    let mut bytes = vec![0; n];
    stream.read_exact(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}
pub(super) fn read_any(stream: &mut impl Read) -> Result<(Kind, Vec<u8>)> {
    let mut h = [0; 10];
    stream.read_exact(&mut h).map_err(io_error)?;
    let kind = match h[5] {
        1 => Kind::Request,
        2 => Kind::Reply,
        3 => Kind::Event,
        4 => Kind::Applied,
        5 => Kind::Ready,
        6 => Kind::Retry,
        7 => Kind::Rejected,
        _ => return Err(Error::Frame),
    };
    let n = u32::from_be_bytes(h[6..].try_into().map_err(|_| Error::Frame)?) as usize;
    if &h[..5] != FRAME_MAGIC || !valid(kind, n) {
        return Err(Error::Frame);
    }
    let mut bytes = vec![0; n];
    stream.read_exact(&mut bytes).map_err(io_error)?;
    Ok((kind, bytes))
}
#[derive(Clone, Copy)]
pub(super) struct PublicPeer {
    pub key: [u8; 32],
    pub session: SessionId,
}
impl PublicPeer {
    pub fn send(&self, out: &mut impl Write) -> Result<()> {
        out.write_all(&self.key).map_err(io_error)?;
        out.write_all(&self.session.0).map_err(io_error)?;
        out.flush().map_err(io_error)
    }
    pub fn receive(input: &mut impl Read) -> Result<Self> {
        let mut key = [0; 32];
        let mut session = [0; 16];
        input.read_exact(&mut key).map_err(io_error)?;
        input.read_exact(&mut session).map_err(io_error)?;
        Ok(Self {
            key,
            session: SessionId(session),
        })
    }
}
pub(super) struct SigningPeer {
    identity: DeviceIdentity,
    public: PublicPeer,
}
impl SigningPeer {
    pub fn new() -> Result<Self> {
        let identity = DeviceIdentity::generate().map_err(|_| Error::Setup)?;
        let public = PublicPeer {
            key: identity.public_key(),
            session: SessionId::random().map_err(|_| Error::Setup)?,
        };
        Ok(Self { identity, public })
    }
    pub fn public(&self) -> PublicPeer {
        self.public
    }
}
#[derive(Clone, Copy)]
pub(super) struct Settings {
    pub group: [u8; 32],
    pub membership: [u8; 32],
    pub ttl: u32,
    pub policy: ClockPolicy,
}
pub(super) struct Protocol {
    signing: SigningPeer,
    peer: PublicPeer,
    delivery: BufferedDelivery,
    settings: Settings,
    ready_at: u64,
    _directory: tempfile::TempDir,
}
impl Protocol {
    pub fn new(signing: SigningPeer, peer: PublicPeer) -> Result<Self> {
        Self::with_settings(
            signing,
            peer,
            Settings {
                group: GROUP,
                membership: MEMBERSHIP,
                ttl: TTL,
                policy: ClockPolicy::new(
                    CLOCK_MAX_RTT_MS,
                    CLOCK_SAMPLE_LIFETIME_MS,
                    Some(1000),
                    Some(1),
                )
                .map_err(|_| Error::Setup)?,
            },
        )
    }
    pub fn with_settings(
        signing: SigningPeer,
        peer: PublicPeer,
        settings: Settings,
    ) -> Result<Self> {
        let directory = tempfile::tempdir().map_err(io_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(io_error)?;
        }
        let now = now()?;
        let gate = ReceiverSession::after_authenticated_handshake(
            ReceiverBinding {
                group: settings.group,
                membership: settings.membership,
                receiver: signing.identity.id(),
                session: signing.public.session,
            },
            now,
            settings.ttl,
        )
        .map_err(|_| Error::Frame)?;
        let state =
            DurableAdmission::initialize(directory.path(), gate, 1).map_err(|_| Error::Worker)?;
        let mut delivery = BufferedDelivery::new(
            DeliveryReceiver::new(state),
            crate::text::MAX_TEXT_BYTES + TEXT_EVENT_WIRE_OVERHEAD,
        )
        .map_err(|_| Error::Setup)?;
        delivery
            .activate_authenticated_source(SessionBinding {
                group: settings.group,
                membership: settings.membership,
                source: DeviceId::from_public_key(&peer.key),
                session: peer.session,
            })
            .map_err(|_| Error::Frame)?;
        Ok(Self {
            signing,
            peer,
            delivery,
            ready_at: now + u64::from(settings.ttl) + QUIET_BOUNDARY_MARGIN_MS,
            settings,
            _directory: directory,
        })
    }
    fn incoming_binding(&self) -> ClockBinding {
        ClockBinding {
            group: self.settings.group,
            membership: self.settings.membership,
            source: DeviceId::from_public_key(&self.peer.key),
            source_session: self.peer.session,
            receiver: self.signing.identity.id(),
            receiver_session: self.signing.public.session,
        }
    }
    pub fn calibrate(&self, stream: &mut (impl Read + Write)) -> Result<ClockEstimate> {
        let probe = ClockProbe::start(self.incoming_binding(), now()?).map_err(|_| Error::Frame)?;
        send(stream, Kind::Request, &probe.request().to_wire())?;
        let reply =
            ClockReply::from_wire(&receive(stream, Kind::Reply)?).map_err(|_| Error::Frame)?;
        // Explicit test-host assumptions; not production-wide bounds.
        probe
            .finish(&reply, &self.peer.key, now()?, self.settings.policy)
            .map_err(|_| Error::Frame)
    }
    pub fn answer_clock(&self, stream: &mut (impl Read + Write)) -> Result<()> {
        let request =
            ClockRequest::from_wire(&receive(stream, Kind::Request)?).map_err(|_| Error::Frame)?;
        let received = now()?;
        let b = request.binding();
        let expected = self.incoming_binding();
        if b.group != expected.group
            || b.membership != expected.membership
            || b.source != expected.receiver
            || b.source_session != expected.receiver_session
            || b.receiver != expected.source
            || b.receiver_session != expected.source_session
        {
            return Err(Error::Frame);
        }
        let reply = ClockReply::sign(&request, &self.signing.identity, received, now()?)
            .map_err(|_| Error::Frame)?;
        send(stream, Kind::Reply, &reply.to_wire())
    }
    pub fn wait_ready(&self) -> Result<()> {
        while now()? < self.ready_at {
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    pub fn sign(&mut self, text: &str) -> Result<SignedTextEvent> {
        let (sequence, lamport) = self
            .delivery
            .reserve_publication()
            .map_err(|_| Error::Worker)?;
        let utc = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::Setup)?
            .as_millis();
        SignedTextEvent::sign(
            EventContext {
                group: self.settings.group,
                membership: self.settings.membership,
                session: self.signing.public.session,
                sequence,
                lamport,
                created_unix_ms: u64::try_from(utc).map_err(|_| Error::Setup)?,
                created_boot_ms: now()?,
                ttl_ms: self.settings.ttl,
            },
            text,
            &self.signing.identity,
        )
        .map_err(|_| Error::Frame)
    }
    pub fn ready(&self) -> Result<bool> {
        Ok(now()? >= self.ready_at)
    }
    pub fn receive_live(
        &mut self,
        event: &SignedTextEvent,
        estimate: &ClockEstimate,
        session: &mut ClipboardSession,
    ) -> Result<Option<DeliveryDecision>> {
        match self
            .delivery
            .receive(event, &self.peer.key, estimate, &SystemBootClock, |text| {
                session.apply_received_text(text).map_err(|_| ())
            }) {
            Ok(d) => Ok(Some(d)),
            Err(BufferError::Delivery(DeliveryError::ApplyFailed)) => Ok(None),
            Err(_) => Err(Error::Frame),
        }
    }
    pub fn start_probe(&self) -> Result<ClockProbe> {
        ClockProbe::start(self.incoming_binding(), now()?).map_err(|_| Error::Frame)
    }
    pub fn finish_probe(&self, probe: ClockProbe, bytes: &[u8]) -> Result<ClockEstimate> {
        let reply = ClockReply::from_wire(bytes).map_err(|_| Error::Frame)?;
        probe
            .finish(&reply, &self.peer.key, now()?, self.settings.policy)
            .map_err(|_| Error::Frame)
    }
    pub fn clock_reply(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        let request = ClockRequest::from_wire(bytes).map_err(|_| Error::Frame)?;
        let b = request.binding();
        let e = self.incoming_binding();
        let received = now()?;
        if b.group != e.group
            || b.membership != e.membership
            || b.source != e.receiver
            || b.receiver != e.source
            || b.source_session != e.receiver_session
            || b.receiver_session != e.source_session
        {
            return Err(Error::Frame);
        }
        Ok(
            ClockReply::sign(&request, &self.signing.identity, received, now()?)
                .map_err(|_| Error::Frame)?
                .to_wire(),
        )
    }
    pub fn record_local(&mut self, event: &SignedTextEvent) -> Result<()> {
        let v = event
            .verify_with_trusted_key(&self.signing.public.key)
            .map_err(|_| Error::Frame)?;
        self.delivery
            .record_local_event(&v)
            .map_err(|_| Error::Worker)
    }
    pub fn receipt_bytes(
        &self,
        event: &SignedTextEvent,
        decision: DeliveryDecision,
    ) -> Result<Vec<u8>> {
        let v = event
            .verify_with_trusted_key(&self.peer.key)
            .map_err(|_| Error::Frame)?;
        Ok(SignedApplicationReceipt::sign_applied(
            &v,
            decision,
            &self.signing.identity,
            self.signing.public.session,
        )
        .map_err(|_| Error::Frame)?
        .to_wire())
    }
    pub fn observe_receipt(
        &self,
        bytes: &[u8],
        tracker: &mut DeliveryTracker,
    ) -> Result<ReceiptDecision> {
        let r = SignedApplicationReceipt::from_wire(bytes).map_err(|_| Error::Frame)?;
        let v = r
            .verify_with_trusted_key(&self.peer.key)
            .map_err(|_| Error::Authentication)?;
        tracker.observe(&v).map_err(|_| Error::Frame)
    }
    pub fn retry(
        &mut self,
        estimate: &ClockEstimate,
        session: &mut ClipboardSession,
    ) -> Result<Option<(SignedTextEvent, DeliveryDecision)>> {
        self.delivery
            .maintain(estimate, &SystemBootClock)
            .map_err(|_| Error::Frame)?;
        let Some(event) = self.delivery.pending_event().map_err(|_| Error::Frame)? else {
            return Ok(None);
        };
        self.receive_live(&event, estimate, session)
            .map(|d| d.map(|d| (event, d)))
    }
    pub fn tracker(&self, event: &SignedTextEvent) -> Result<DeliveryTracker> {
        let verified = event
            .verify_with_trusted_key(&self.signing.public.key)
            .map_err(|_| Error::Authentication)?;
        DeliveryTracker::new(
            &verified,
            &[ExpectedReceiver {
                device: DeviceId::from_public_key(&self.peer.key),
                session: self.peer.session,
            }],
        )
        .map_err(|_| Error::Frame)
    }
    pub fn send_receipt(
        &self,
        stream: &mut impl Write,
        decision: DeliveryDecision,
        event: &SignedTextEvent,
        tamper: bool,
    ) -> Result<()> {
        let verified = event
            .verify_with_trusted_key(&self.peer.key)
            .map_err(|_| Error::Authentication)?;
        let receipt = SignedApplicationReceipt::sign_applied(
            &verified,
            decision,
            &self.signing.identity,
            self.signing.public.session,
        )
        .map_err(|_| Error::Frame)?;
        let mut bytes = receipt.to_wire();
        if tamper {
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
        }
        send(stream, Kind::Applied, &bytes)
    }
    pub fn expect_applied(
        &self,
        stream: &mut impl Read,
        tracker: &mut DeliveryTracker,
    ) -> Result<ReceiptDecision> {
        let receipt = SignedApplicationReceipt::from_wire(&receive(stream, Kind::Applied)?)
            .map_err(|_| Error::Frame)?;
        let verified = receipt
            .verify_with_trusted_key(&self.peer.key)
            .map_err(|_| Error::Authentication)?;
        tracker.observe(&verified).map_err(|_| Error::Frame)
    }
    pub fn sign_expired(&mut self, text: &str) -> Result<SignedTextEvent> {
        let event = self.sign(text)?;
        let mut context = event.context();
        context.created_boot_ms = now()?
            .checked_sub(u64::from(self.settings.ttl) + 500)
            .ok_or(Error::Setup)?;
        SignedTextEvent::sign(context, text, &self.signing.identity).map_err(|_| Error::Frame)
    }
    pub fn apply(
        &mut self,
        event: &SignedTextEvent,
        estimate: &ClockEstimate,
        session: &mut ClipboardSession,
        fail: bool,
    ) -> Result<Option<DeliveryDecision>> {
        event
            .verify_with_trusted_key(&self.peer.key)
            .map_err(|_| Error::Authentication)?;
        let decision =
            self.delivery
                .receive(event, &self.peer.key, estimate, &SystemBootClock, |text| {
                    if fail {
                        return Err(());
                    }
                    session.apply_received_text(text).map_err(|_| ())
                });
        match decision {
            Err(BufferError::Delivery(DeliveryError::ApplyFailed)) if fail => Ok(None),
            Ok(d) => Ok(Some(d)),
            Err(_) => Err(Error::Frame),
        }
    }
}
pub(super) fn now() -> Result<u64> {
    SystemBootClock.now_ms().map_err(|_| Error::Setup)
}
pub(super) fn read_event(stream: &mut impl Read) -> Result<SignedTextEvent> {
    SignedTextEvent::from_wire(&receive(stream, Kind::Event)?).map_err(|_| Error::Frame)
}
pub(super) fn send_event(stream: &mut impl Write, event: &SignedTextEvent) -> Result<()> {
    // Give these controlled localhost samples age beyond clock granularity/RTT uncertainty.
    thread::sleep(Duration::from_millis(100));
    send(stream, Kind::Event, &event.to_wire())
}
pub(super) fn expect_control_hash(
    stream: &mut impl Read,
    kind: Kind,
    event: &SignedTextEvent,
) -> Result<()> {
    if receive(stream, kind)?.as_slice() == event.envelope_hash() {
        Ok(())
    } else {
        Err(Error::Frame)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_limits_are_checked_before_reading_or_allocating_payload() {
        for (kind, n) in [
            (Kind::Event, 0),
            (Kind::Event, 10_000_223),
            (Kind::Reply, 283),
            (Kind::Applied, 31),
        ] {
            let mut header = FRAME_MAGIC.to_vec();
            header.push(kind as u8);
            header.extend((n as u32).to_be_bytes());
            assert!(matches!(
                receive(&mut header.as_slice(), kind),
                Err(Error::Frame)
            ));
        }
    }
    #[test]
    fn wrong_type_version_and_truncated_frames_are_rejected() {
        let mut bytes = vec![];
        send(&mut bytes, Kind::Applied, &[1; RECEIPT_BYTES]).unwrap();
        assert!(receive(&mut bytes.as_slice(), Kind::Retry).is_err());
        let mut wrong = bytes.clone();
        wrong[4] = 2;
        assert!(receive(&mut wrong.as_slice(), Kind::Applied).is_err());
        assert!(receive(&mut &bytes[..bytes.len() - 1], Kind::Applied).is_err());
    }
    #[test]
    fn hash_only_ack_cannot_be_mistaken_for_signed_application_receipt() {
        assert!(send(&mut vec![], Kind::Applied, &[0; 32]).is_err());
        let mut bytes = vec![];
        send(&mut bytes, Kind::Retry, &[0; 32]).unwrap();
        assert!(receive(&mut bytes.as_slice(), Kind::Applied).is_err());
    }
}

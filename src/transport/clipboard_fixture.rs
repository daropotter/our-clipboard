//! Controlled integration fixture, not a user-facing synchronization daemon or pairing protocol.
use super::*;
use crate::{clipboard::TextClipboard, session::ClipboardSession};
use std::{
    process::{Child, Command, Stdio},
    sync::mpsc,
};
const RESPONSE: &str = "Synthetic reply from the second clipboard 🦀\nWith another line";
const TARGET_VERIFIED: &str = "synthetic target independently verified";
use super::peer_protocol as protocol;
use protocol::{Kind, Protocol, PublicPeer, SigningPeer};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait(child: &mut Child) -> Result<()> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            return if status.success() {
                Ok(())
            } else {
                Err(Error::Worker)
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Worker);
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn subprocess(mode: &str, display: &str) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe().map_err(io_error)?);
    command
        .args(["--replace-clipboard", mode])
        .env("DISPLAY", display)
        .env("XDG_SESSION_TYPE", "x11")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET");
    Ok(command)
}
fn inspect(display: &str, response: bool) -> Result<()> {
    let mut child = ChildGuard(
        subprocess(
            if response {
                "inspect-response"
            } else {
                "inspect-request"
            },
            display,
        )?
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io_error)?,
    );
    wait(&mut child.0)
}
fn send_certificate(out: &mut impl Write, certificate: &CertificateDer<'static>) -> Result<()> {
    out.write_all(&(certificate.len() as u32).to_be_bytes())
        .map_err(io_error)?;
    out.write_all(certificate.as_ref()).map_err(io_error)?;
    out.flush().map_err(io_error)
}
fn receive_certificate(input: &mut impl Read) -> Result<CertificateDer<'static>> {
    let mut length = [0; 4];
    input.read_exact(&mut length).map_err(io_error)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > 32_768 {
        return Err(Error::Frame);
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).map_err(io_error)?;
    Ok(bytes.into())
}
fn receiver() -> Result<()> {
    // Only public certificates travel through the trusted parent/child test pipes.
    let peer = receive_certificate(&mut std::io::stdin())?;
    let peer_signing = PublicPeer::receive(&mut std::io::stdin())?;
    let signing = SigningPeer::new()?;
    let identity = Identity::new()?;
    let config = server_config(&identity, &peer)?;
    let mut raw = TextClipboard::new().map_err(|_| Error::Worker)?;
    raw.set_text("Preexisting synthetic receiver clipboard")
        .map_err(|_| Error::Worker)?;
    let mut session = ClipboardSession::new().map_err(|_| Error::Worker)?;
    if session
        .next_local_text(Duration::from_millis(100))
        .map_err(|_| Error::Worker)?
        .is_some()
    {
        return Err(Error::Frame);
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(io_error)?;
    std::io::stdout()
        .write_all(
            &listener
                .local_addr()
                .map_err(io_error)?
                .port()
                .to_be_bytes(),
        )
        .map_err(io_error)?;
    send_certificate(&mut std::io::stdout(), &identity.cert)?;
    signing.public().send(&mut std::io::stdout())?;
    let mut socket = DeadlineStream::with_timeout(accept(&listener)?, Duration::from_secs(10))?;
    let mut connection = ServerConnection::new(config).map_err(|_| Error::Setup)?;
    while connection.is_handshaking() {
        connection.complete_io(&mut socket).map_err(io_error)?;
    }
    let mut stream = StreamOwned::new(connection, socket);
    let mut protocol = Protocol::new(signing, peer_signing)?;
    protocol.answer_clock(&mut stream)?;
    let estimate = protocol.calibrate(&mut stream)?;
    protocol.wait_ready()?;
    protocol::send(&mut stream, Kind::Ready, &[1])?;
    let tampered = protocol::read_event(&mut stream)?;
    if !matches!(
        protocol.apply(&tampered, &estimate, &mut session, false),
        Err(Error::Authentication)
    ) {
        return Err(Error::Frame);
    }
    if raw.get_text().map_err(|_| Error::Worker)? != "Preexisting synthetic receiver clipboard" {
        return Err(Error::Frame);
    }
    protocol::send(&mut stream, Kind::Rejected, &tampered.envelope_hash())?;
    let expired = protocol::read_event(&mut stream)?;
    if !matches!(
        protocol.apply(&expired, &estimate, &mut session, false)?,
        Some(crate::freshness::DeliveryDecision::Skipped(_))
    ) {
        return Err(Error::Frame);
    }
    if raw.get_text().map_err(|_| Error::Worker)? != "Preexisting synthetic receiver clipboard" {
        return Err(Error::Frame);
    }
    protocol::send(&mut stream, Kind::Rejected, &expired.envelope_hash())?;
    let event = protocol::read_event(&mut stream)?;
    // Inject one failure before invoking the native write; no application receipt may follow.
    if protocol
        .apply(&event, &estimate, &mut session, true)?
        .is_some()
    {
        return Err(Error::Frame);
    }
    protocol::send(&mut stream, Kind::Retry, &event.envelope_hash())?;
    let retry = protocol::read_event(&mut stream)?;
    if retry.envelope_hash() != event.envelope_hash() {
        return Err(Error::Frame);
    }
    let decision = protocol
        .apply(&retry, &estimate, &mut session, false)?
        .ok_or(Error::Frame)?;
    if !matches!(decision, crate::freshness::DeliveryDecision::Applied { .. }) {
        return Err(Error::Frame);
    }
    if raw.get_text().map_err(|_| Error::Worker)? != SAMPLE {
        return Err(Error::Frame);
    }
    if session
        .next_local_text(Duration::from_millis(100))
        .map_err(|_| Error::Worker)?
        .is_some()
    {
        return Err(Error::Frame);
    }
    protocol.send_receipt(&mut stream, decision, &event, true)?;
    protocol.send_receipt(&mut stream, decision, &event, false)?;
    let duplicate = protocol::read_event(&mut stream)?;
    let decision = protocol
        .apply(&duplicate, &estimate, &mut session, false)?
        .ok_or(Error::Frame)?;
    if !matches!(
        decision,
        crate::freshness::DeliveryDecision::AlreadyApplied { .. }
    ) {
        return Err(Error::Frame);
    }
    protocol.send_receipt(&mut stream, decision, &event, false)?;
    if receive_text(&mut stream)? != TARGET_VERIFIED {
        return Err(Error::Frame);
    }
    raw.set_text(RESPONSE).map_err(|_| Error::Worker)?;
    let response = session
        .next_local_text(Duration::from_secs(1))
        .map_err(|_| Error::Worker)?
        .ok_or(Error::Frame)?;
    if response != RESPONSE {
        return Err(Error::Frame);
    }
    let response = protocol.sign(&response)?;
    protocol::send_event(&mut stream, &response)?;
    let mut tracker = protocol.tracker(&response)?;
    if protocol.expect_applied(&mut stream, &mut tracker)? != crate::receipts::ReceiptDecision::New
        || !tracker.progress().complete()
    {
        return Err(Error::Frame);
    }
    if session
        .next_local_text(Duration::from_millis(100))
        .map_err(|_| Error::Worker)?
        .is_some()
    {
        return Err(Error::Frame);
    }
    stream.conn.send_close_notify();
    stream.flush().map_err(io_error)?;
    let mut trailing = [0; 1];
    if stream.read(&mut trailing).map_err(io_error)? != 0 {
        return Err(Error::Frame);
    }
    let _ = stream.sock.shutdown(Shutdown::Write);
    Ok(())
}
fn controller() -> Result<()> {
    let source = std::env::var("DISPLAY").map_err(|_| Error::Setup)?;
    let destination =
        std::env::var("OUR_CLIPBOARD_TEST_TARGET_DISPLAY").map_err(|_| Error::Setup)?;
    if source.is_empty() || destination.is_empty() || source == destination {
        return Err(Error::Setup);
    }
    let identity = Identity::new()?;
    let signing = SigningPeer::new()?;
    let mut child = ChildGuard(
        subprocess("receiver", &destination)?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(io_error)?,
    );
    let mut input = child.0.stdin.take().ok_or(Error::Worker)?;
    send_certificate(&mut input, &identity.cert)?;
    signing.public().send(&mut input)?;
    let mut output = child.0.stdout.take().ok_or(Error::Worker)?;
    let (send, receive) = mpsc::channel();
    let reader = thread::Builder::new()
        .name("fixture-public-config".into())
        .spawn(move || {
            let mut read = || -> Result<_> {
                let mut port = [0; 2];
                output.read_exact(&mut port).map_err(io_error)?;
                let certificate = receive_certificate(&mut output)?;
                Ok((
                    u16::from_be_bytes(port),
                    certificate,
                    PublicPeer::receive(&mut output)?,
                ))
            };
            let _ = send.send(read());
        })
        .map_err(|_| Error::Worker)?;
    let config = receive.recv_timeout(DEADLINE);
    if config.is_err() {
        let _ = child.0.kill();
        let _ = child.0.wait();
    }
    reader.join().map_err(|_| Error::Worker)?;
    let (port, certificate, peer_signing) = config.map_err(|_| Error::Worker)??;
    let config = client_config(&identity, &certificate)?;
    let address = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let mut socket = DeadlineStream::with_timeout(
        TcpStream::connect_timeout(&address, DEADLINE).map_err(io_error)?,
        Duration::from_secs(10),
    )?;
    let mut connection =
        ClientConnection::new(config, "localhost".try_into().map_err(|_| Error::Setup)?)
            .map_err(|_| Error::Setup)?;
    while connection.is_handshaking() {
        connection.complete_io(&mut socket).map_err(io_error)?;
    }
    let mut stream = StreamOwned::new(connection, socket);
    let mut protocol = Protocol::new(signing, peer_signing)?;
    let estimate = protocol.calibrate(&mut stream)?;
    protocol.answer_clock(&mut stream)?;
    if protocol::receive(&mut stream, Kind::Ready)? != [1] {
        return Err(Error::Frame);
    }
    protocol.wait_ready()?;
    let mut raw = TextClipboard::new().map_err(|_| Error::Worker)?;
    raw.set_text("Preexisting synthetic source clipboard")
        .map_err(|_| Error::Worker)?;
    let mut session = ClipboardSession::new().map_err(|_| Error::Worker)?;
    if session
        .next_local_text(Duration::from_millis(100))
        .map_err(|_| Error::Worker)?
        .is_some()
    {
        return Err(Error::Frame);
    }
    raw.set_text(SAMPLE).map_err(|_| Error::Worker)?;
    let text = session
        .next_local_text(Duration::from_secs(1))
        .map_err(|_| Error::Worker)?
        .ok_or(Error::Frame)?;
    if text != SAMPLE {
        return Err(Error::Frame);
    }
    let event = protocol.sign(&text)?;
    let mut tampered = event.to_wire();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    let tampered =
        crate::events::SignedTextEvent::from_wire(&tampered).map_err(|_| Error::Frame)?;
    protocol::send_event(&mut stream, &tampered)?;
    protocol::expect_control_hash(&mut stream, Kind::Rejected, &tampered)?;
    let expired = protocol.sign_expired(&text)?;
    protocol::send_event(&mut stream, &expired)?;
    protocol::expect_control_hash(&mut stream, Kind::Rejected, &expired)?;

    protocol::send_event(&mut stream, &event)?;
    protocol::expect_control_hash(&mut stream, Kind::Retry, &event)?;
    protocol::send_event(&mut stream, &event)?;
    let mut tracker = protocol.tracker(&event)?;
    let before = tracker.progress();
    if protocol.expect_applied(&mut stream, &mut tracker).is_ok() || tracker.progress() != before {
        return Err(Error::Frame);
    }
    if protocol.expect_applied(&mut stream, &mut tracker)? != crate::receipts::ReceiptDecision::New
        || !tracker.progress().complete()
    {
        return Err(Error::Frame);
    }
    protocol::send_event(&mut stream, &event)?;
    if protocol.expect_applied(&mut stream, &mut tracker)?
        != crate::receipts::ReceiptDecision::Duplicate
        || tracker.progress().applied != 1
    {
        return Err(Error::Frame);
    }
    inspect(&destination, false)?;
    send_text(&mut stream, TARGET_VERIFIED)?;
    let response = protocol::read_event(&mut stream)?;
    let decision = protocol
        .apply(&response, &estimate, &mut session, false)?
        .ok_or(Error::Frame)?;
    if !matches!(decision, crate::freshness::DeliveryDecision::Applied { .. }) {
        return Err(Error::Frame);
    }
    inspect(&source, true)?;
    if session
        .next_local_text(Duration::from_millis(100))
        .map_err(|_| Error::Worker)?
        .is_some()
    {
        return Err(Error::Frame);
    }
    protocol.send_receipt(&mut stream, decision, &response, false)?;
    stream.conn.send_close_notify();
    stream.flush().map_err(io_error)?;
    let mut trailing = [0; 1];
    if stream.read(&mut trailing).map_err(io_error)? != 0 {
        return Err(Error::Frame);
    }
    let _ = stream.sock.shutdown(Shutdown::Write);
    wait(&mut child.0)
}

/// Fixed synthetic integration roles, invoked only by the explicitly destructive example.
pub fn run(mode: &str) -> std::result::Result<String, &'static str> {
    if !cfg!(target_os = "linux") || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err(
            "This fixture requires disposable Linux X11 sessions, without Wayland fallback.",
        );
    }
    let action = || -> Result<()> {
        match mode {
            "controller" => controller(),
            "receiver" => receiver(),
            "inspect-request" | "inspect-response" => {
                let observed = TextClipboard::new()
                    .map_err(|_| Error::Worker)?
                    .get_text()
                    .map_err(|_| Error::Worker)?;
                if observed
                    == if mode == "inspect-response" {
                        RESPONSE
                    } else {
                        SAMPLE
                    }
                {
                    Ok(())
                } else {
                    Err(Error::Frame)
                }
            }
            "seed-request" | "seed-response" => {
                let mut clipboard = TextClipboard::new().map_err(|_| Error::Worker)?;
                clipboard
                    .set_text(if mode == "seed-request" {
                        SAMPLE
                    } else {
                        RESPONSE
                    })
                    .map_err(|_| Error::Worker)?;
                // Keep synthetic ownership through a 30-second quiet period and
                // the long-outage reconnect regression; the fixture kills helpers.
                thread::sleep(Duration::from_secs(90));
                Ok(())
            }
            "check-display" => TextClipboard::new().map(|_| ()).map_err(|_| Error::Worker),
            _ => Err(Error::Setup),
        }
    };
    action().map_err(
        |_| "Synthetic two-clipboard fixture failed; content and key material are omitted.",
    )?;
    Ok(if mode == "controller" {
        "Two isolated X11 clipboards exchanged synthetic text over mutual TLS. Signed admission, durable reservations, tamper/age rejection, failed-write retry, receiver-signed receipts, per-target progress, tampered ACK rejection, independent application checks and no-echo checks passed.".into()
    } else {
        String::new()
    })
}

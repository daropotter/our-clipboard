//! Experimental TLS transport probe. Only synthetic data, loopback sockets and ephemeral keys.
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
};
use std::{
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(3);
const MAGIC: &[u8; 4] = b"OCp0";
const SAMPLE: &str = "Synthetic Our Clipboard transport probe — Zażółć 🦀\nSecond line";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Error {
    Setup,
    Io,
    Authentication,
    Frame,
    Worker,
}
type Result<T> = std::result::Result<T, Error>;

// No Debug implementation: this owns private key bytes.
struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}
impl Identity {
    fn new() -> Result<Self> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).map_err(|_| Error::Setup)?;
        let mut params =
            rcgen::CertificateParams::new(vec!["localhost".into()]).map_err(|_| Error::Setup)?;
        params.extended_key_usages = vec![
            rcgen::ExtendedKeyUsagePurpose::ServerAuth,
            rcgen::ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let cert = params.self_signed(&key).map_err(|_| Error::Setup)?;
        Ok(Self {
            cert: cert.der().clone(),
            key: key.serialize_der().into(),
        })
    }
}
fn roots(peer: &CertificateDer<'static>) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    roots.add(peer.clone()).map_err(|_| Error::Setup)?;
    Ok(roots)
}
fn configs(
    server: &Identity,
    client: &Identity,
    trusted_server: &Identity,
    trusted_client: &Identity,
) -> Result<(Arc<ServerConfig>, Arc<ClientConfig>)> {
    Ok((
        server_config(server, &trusted_client.cert)?,
        client_config(client, &trusted_server.cert)?,
    ))
}
fn server_config(
    server: &Identity,
    trusted_client: &CertificateDer<'static>,
) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots(trusted_client)?),
        provider.clone(),
    )
    .build()
    .map_err(|_| Error::Setup)?;
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| Error::Setup)?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![server.cert.clone()], server.key.clone_key().into())
        .map_err(|_| Error::Setup)?;
    Ok(Arc::new(config))
}
fn client_config(
    client: &Identity,
    trusted_server: &CertificateDer<'static>,
) -> Result<Arc<ClientConfig>> {
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| Error::Setup)?
            .with_root_certificates(roots(trusted_server)?)
            .with_client_auth_cert(vec![client.cert.clone()], client.key.clone_key().into())
            .map_err(|_| Error::Setup)?;
    Ok(Arc::new(config))
}

fn io_error(error: std::io::Error) -> Error {
    if error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
        .is_some()
    {
        Error::Authentication
    } else if error.kind() == std::io::ErrorKind::InvalidData {
        Error::Frame
    } else {
        Error::Io
    }
}

fn send_text(stream: &mut impl Write, text: &str) -> Result<()> {
    crate::text::validate(text).map_err(|_| Error::Frame)?;
    stream.write_all(MAGIC).map_err(io_error)?;
    stream
        .write_all(&(text.len() as u32).to_be_bytes())
        .map_err(io_error)?;
    stream.write_all(text.as_bytes()).map_err(io_error)?;
    stream.flush().map_err(io_error)
}
fn receive_text(stream: &mut impl Read) -> Result<String> {
    let mut header = [0; 8];
    stream.read_exact(&mut header).map_err(io_error)?;
    let length = u32::from_be_bytes(header[4..].try_into().map_err(|_| Error::Frame)?) as usize;
    if &header[..4] != MAGIC || length == 0 || length > crate::text::MAX_TEXT_BYTES {
        return Err(Error::Frame);
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).map_err(io_error)?;
    let text = String::from_utf8(payload).map_err(|_| Error::Frame)?;
    crate::text::validate(&text).map_err(|_| Error::Frame)?;
    Ok(text)
}

/// Nonblocking I/O retries share one absolute deadline on every supported OS.
struct DeadlineStream {
    socket: TcpStream,
    deadline: Instant,
}
impl DeadlineStream {
    fn new(socket: TcpStream) -> Result<Self> {
        Self::with_timeout(socket, DEADLINE)
    }
    fn with_timeout(socket: TcpStream, timeout: Duration) -> Result<Self> {
        socket.set_nonblocking(true).map_err(io_error)?;
        Ok(Self {
            socket,
            deadline: Instant::now() + timeout,
        })
    }
    fn check(&self) -> std::io::Result<()> {
        if Instant::now() >= self.deadline {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "transport deadline exceeded",
            ))
        } else {
            Ok(())
        }
    }
    fn pause(&self) {
        thread::sleep(
            Duration::from_millis(2).min(self.deadline.saturating_duration_since(Instant::now())),
        );
    }
    fn shutdown(&self, direction: Shutdown) -> std::io::Result<()> {
        self.socket.shutdown(direction)
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.socket.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    self.pause()
                }
                result => return result,
            }
        }
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.socket.write(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    self.pause()
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.socket.flush()
    }
}
fn accept(listener: &TcpListener) -> Result<TcpStream> {
    listener.set_nonblocking(true).map_err(io_error)?;
    let deadline = Instant::now() + DEADLINE;
    loop {
        match listener.accept() {
            Ok((socket, _)) => return Ok(socket),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5))
            }
            Err(error) => return Err(io_error(error)),
        }
    }
}
fn server(listener: TcpListener, config: Arc<ServerConfig>) -> Result<()> {
    let mut socket = DeadlineStream::new(accept(&listener)?)?;
    let mut connection = ServerConnection::new(config).map_err(|_| Error::Setup)?;
    while connection.is_handshaking() {
        connection.complete_io(&mut socket).map_err(io_error)?;
    }
    if connection.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
        return Err(Error::Setup);
    }
    let mut stream = StreamOwned::new(connection, socket);
    let observed = receive_text(&mut stream)?;
    if observed != SAMPLE {
        return Err(Error::Frame);
    }
    send_text(&mut stream, SAMPLE)?;
    stream.conn.send_close_notify();
    stream.flush().map_err(io_error)?;
    let mut trailing = [0; 1];
    if stream.read(&mut trailing).map_err(io_error)? != 0 {
        return Err(Error::Frame);
    }
    let _ = stream.sock.shutdown(Shutdown::Write);
    Ok(())
}
fn client(address: SocketAddr, config: Arc<ClientConfig>) -> Result<()> {
    let mut socket =
        DeadlineStream::new(TcpStream::connect_timeout(&address, DEADLINE).map_err(io_error)?)?;
    let name = "localhost".try_into().map_err(|_| Error::Setup)?;
    let mut connection = ClientConnection::new(config, name).map_err(|_| Error::Setup)?;
    while connection.is_handshaking() {
        connection.complete_io(&mut socket).map_err(io_error)?;
    }
    if connection.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
        return Err(Error::Setup);
    }
    let mut stream = StreamOwned::new(connection, socket);
    send_text(&mut stream, SAMPLE)?;
    let observed = receive_text(&mut stream)?;
    if observed != SAMPLE {
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
fn pump(from: TcpStream, to: TcpStream, capture: Arc<Mutex<Vec<u8>>>) -> Result<()> {
    let mut from = DeadlineStream::new(from)?;
    let mut to = DeadlineStream::new(to)?;
    let mut chunk = [0; 8192];
    loop {
        let count = from.read(&mut chunk).map_err(io_error)?;
        if count == 0 {
            break;
        }
        {
            let mut capture = capture.lock().map_err(|_| Error::Worker)?;
            if capture.len() + count > 262_144 {
                return Err(Error::Frame);
            }
            capture.extend_from_slice(&chunk[..count]);
        }
        to.write_all(&chunk[..count]).map_err(io_error)?;
    }
    // A peer may already have closed after its TLS close-notify.
    let _ = to.shutdown(Shutdown::Write);
    Ok(())
}
fn relay(
    listener: TcpListener,
    destination: SocketAddr,
    capture: Arc<Mutex<Vec<u8>>>,
) -> Result<()> {
    let incoming = accept(&listener)?;
    let outgoing = TcpStream::connect_timeout(&destination, DEADLINE).map_err(io_error)?;
    let upstream_from = incoming.try_clone().map_err(io_error)?;
    let upstream_to = outgoing.try_clone().map_err(io_error)?;
    let upstream_capture = capture.clone();
    let upstream = thread::Builder::new()
        .name("transport-forward".into())
        .spawn(move || pump(upstream_from, upstream_to, upstream_capture))
        .map_err(|_| Error::Worker)?;
    let downstream = pump(outgoing, incoming, capture);
    let upstream = upstream.join().map_err(|_| Error::Worker)?;
    downstream?;
    upstream
}

#[derive(Clone, Copy)]
enum Trust {
    Both,
    WrongClient,
    WrongServer,
}
fn exercise(via_relay: bool, trust: Trust) -> Result<(Result<()>, Result<()>)> {
    let server_id = Identity::new()?;
    let client_id = Identity::new()?;
    let other_id = Identity::new()?;
    let trusted_server = if matches!(trust, Trust::WrongServer) {
        &other_id
    } else {
        &server_id
    };
    let trusted_client = if matches!(trust, Trust::WrongClient) {
        &other_id
    } else {
        &client_id
    };
    let (server_config, client_config) =
        configs(&server_id, &client_id, trusted_server, trusted_client)?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(io_error)?;
    let server_address = listener.local_addr().map_err(io_error)?;
    let capture = Arc::new(Mutex::new(Vec::new()));
    let relay_listener = if via_relay {
        Some(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(io_error)?)
    } else {
        None
    };
    let address = relay_listener
        .as_ref()
        .map(|l| l.local_addr().map_err(io_error))
        .transpose()?
        .unwrap_or(server_address);
    let server_thread = thread::Builder::new()
        .name("transport-server".into())
        .spawn(move || server(listener, server_config))
        .map_err(|_| Error::Worker)?;
    let relay_capture = capture.clone();
    let relay_thread = relay_listener
        .map(|listener| {
            thread::Builder::new()
                .name("transport-relay".into())
                .spawn(move || relay(listener, server_address, relay_capture))
        })
        .transpose()
        .map_err(|_| Error::Worker)?;
    let client_result = client(address, client_config);
    let server_result = server_thread.join().map_err(|_| Error::Worker)?;
    if let Some(worker) = relay_thread {
        worker.join().map_err(|_| Error::Worker)??;
    }
    if via_relay
        && capture
            .lock()
            .map_err(|_| Error::Worker)?
            .windows(SAMPLE.len())
            .any(|w| w == SAMPLE.as_bytes())
    {
        return Err(Error::Frame);
    }
    Ok((client_result, server_result))
}

pub fn probe() -> std::result::Result<String, &'static str> {
    let run = || -> Result<()> {
        for relayed in [false, true] {
            let (client, server) = exercise(relayed, Trust::Both)?;
            client?;
            server?;
        }
        let (_, server) = exercise(false, Trust::WrongClient)?;
        if server != Err(Error::Authentication) {
            return Err(Error::Authentication);
        }
        let (client, _) = exercise(false, Trust::WrongServer)?;
        if client != Err(Error::Authentication) {
            return Err(Error::Authentication);
        }
        Ok(())
    };
    run().map_err(
        |_| "Synthetic TLS transport probe failed; payloads and key material are omitted.",
    )?;
    Ok("TLS 1.3 direct and blind-relay round trips passed. Untrusted peers rejected on both sides. Synthetic data only; clipboard synchronization is not implemented.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutual_tls_direct_and_relayed() {
        for via_relay in [false, true] {
            let (client, server) = exercise(via_relay, Trust::Both).expect("probe setup failed");
            assert_eq!(client, Ok(()), "client probe failed, relayed={via_relay}");
            assert_eq!(server, Ok(()), "server probe failed, relayed={via_relay}");
        }
        let (client, server) = exercise(false, Trust::WrongClient).expect("probe setup failed");
        assert_eq!(
            server,
            Err(Error::Authentication),
            "wrong-client result: {client:?}"
        );
        let (client, server) = exercise(false, Trust::WrongServer).expect("probe setup failed");
        assert_eq!(
            client,
            Err(Error::Authentication),
            "wrong-server result: {server:?}"
        );
    }
    #[test]
    fn total_connection_deadline_interrupts_a_stalled_socket() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_server, _) = listener.accept().unwrap();
        let mut client = DeadlineStream::with_timeout(client, Duration::from_millis(50)).unwrap();
        let mut byte = [0; 1];
        assert!(
            !matches!(client.read(&mut byte), Ok(1)),
            "stalled socket returned data"
        );
    }
    #[test]
    fn framing_rejects_oversize_and_empty_before_payload_read() {
        for length in [0, crate::text::MAX_TEXT_BYTES as u32 + 1, u32::MAX] {
            let mut bytes = MAGIC.to_vec();
            bytes.extend(length.to_be_bytes());
            let mut input = std::io::Cursor::new(bytes);
            assert!(matches!(receive_text(&mut input), Err(Error::Frame)));
            assert_eq!(input.position(), 8);
        }
    }
    #[test]
    fn framing_validates_content_without_logging_it() {
        let mut bytes = Vec::new();
        send_text(&mut bytes, SAMPLE).unwrap();
        let observed = receive_text(&mut bytes.as_slice()).unwrap();
        assert!(observed == SAMPLE);
        assert!(send_text(&mut Vec::new(), "bad\0text").is_err());
        let mut invalid = MAGIC.to_vec();
        invalid.extend(1u32.to_be_bytes());
        invalid.push(0xff);
        assert!(matches!(
            receive_text(&mut invalid.as_slice()),
            Err(Error::Frame)
        ));
    }
}

#[cfg(feature = "clipboard")]
mod clipboard_fixture;
#[cfg(feature = "clipboard")]
pub use clipboard_fixture::run as run_synthetic_clipboard_fixture;

#[cfg(feature = "clipboard")]
mod peer_protocol;
#[cfg(feature = "clipboard")]
mod pilot;
#[cfg(feature = "clipboard")]
pub use pilot::run as run_pilot;

pub mod outbound;

/// Read only macOS session metadata; never initializes the clipboard or transport.
#[cfg(all(feature = "clipboard", target_os = "macos"))]
pub fn macos_session_probe() -> std::result::Result<String, &'static str> {
    match crate::macos_session::probe() {
        crate::macos_session::State::Unknown => {
            Err("macOS session state is Unknown; clipboard access would remain Idle.")
        }
        state => Ok(format!(
            "macOS session state: {state:?}. No clipboard data accessed."
        )),
    }
}

//! Both endpoints connect outward to WSS; a second, pinned mutual TLS session protects content.
use super::*;
use rustls::pki_types::pem::PemObject;
use std::{
    collections::HashMap,
    io,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use subtle::ConstantTimeEq;
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig, stream::MaybeTlsStream};
use zeroize::Zeroizing;
const PATH: &str = "/our-clipboard/v1";
const CHUNK: usize = 16 * 1024;
const MAX_FRAME: usize = 64 * 1024;
const SETUP_TIMEOUT: Duration = Duration::from_secs(15);
const PAIR_TIMEOUT: Duration = Duration::from_secs(120);
const SESSION_LIMIT: Duration = Duration::from_secs(3600);
const MAX_CONNECTIONS: usize = 32;
const MAX_WAITING: usize = 16;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayError {
    Configuration,
    Authentication,
    PeerAuthentication,
    Transport,
    Protocol,
    Capacity,
    Timeout,
}
impl RelayError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Configuration => {
                "Invalid relay configuration, public offer, or missing OUR_CLIPBOARD_RELAY_TOKEN."
            }
            Self::Authentication => {
                "Relay TLS/access authentication failed; check the relay CA and access credential."
            }
            Self::PeerAuthentication => {
                "Peer end-to-end TLS setup failed: offer/role mismatch or rendezvous timeout."
            }
            Self::Transport => "Outbound relay transport failed.",
            Self::Protocol => "Relay protocol rejected invalid data.",
            Self::Capacity => "Relay capacity is exhausted.",
            Self::Timeout => "Relay or peer setup timed out.",
        }
    }
}
type R<T> = std::result::Result<T, RelayError>;
/// Transport access only, not group membership. No Debug/Display or secret-file fallback.
pub struct AccessToken(Zeroizing<Vec<u8>>);
impl AccessToken {
    pub fn from_env() -> R<Arc<Self>> {
        let value = Zeroizing::new(
            std::env::var("OUR_CLIPBOARD_RELAY_TOKEN").map_err(|_| RelayError::Configuration)?,
        );
        Self::from_bytes(value.as_bytes())
    }
    fn from_bytes(bytes: &[u8]) -> R<Arc<Self>> {
        if !(32..=256).contains(&bytes.len())
            || !bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(b))
        {
            return Err(RelayError::Configuration);
        }
        Ok(Arc::new(Self(Zeroizing::new(bytes.to_vec()))))
    }
    fn header(&self) -> Vec<u8> {
        let mut b = b"Bearer ".to_vec();
        b.extend(self.0.as_slice());
        b
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    A,
    B,
}
impl Side {
    fn byte(self) -> u8 {
        if self == Self::A { 0 } else { 1 }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Room([u8; 32]);
impl Room {
    pub fn parse(s: &str) -> R<Self> {
        if s.len() != 64 || !s.is_ascii() {
            return Err(RelayError::Configuration);
        }
        let mut b = [0; 32];
        for (i, chunk) in s.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let h = (chunk[0] as char)
                .to_digit(16)
                .ok_or(RelayError::Configuration)?;
            let l = (chunk[1] as char)
                .to_digit(16)
                .ok_or(RelayError::Configuration)?;
            b[i] = (h * 16 + l) as u8;
        }
        if b == [0; 32] {
            return Err(RelayError::Configuration);
        }
        Ok(Self(b))
    }
}
/// Public certificate only. Transfer/verify through a trusted independent channel before use.
pub struct PublicOffer(CertificateDer<'static>);
impl PublicOffer {
    pub fn to_wire(&self) -> Vec<u8> {
        let mut b = b"OCRO\x01".to_vec();
        b.extend((self.0.len() as u32).to_be_bytes());
        b.extend(self.0.as_ref());
        b
    }
    pub fn from_wire(bytes: &[u8]) -> R<Self> {
        if !(10..=32_777).contains(&bytes.len()) || &bytes[..5] != b"OCRO\x01" {
            return Err(RelayError::Configuration);
        }
        let n = u32::from_be_bytes(
            bytes[5..9]
                .try_into()
                .map_err(|_| RelayError::Configuration)?,
        ) as usize;
        if n == 0 || n + 9 != bytes.len() {
            return Err(RelayError::Configuration);
        }
        Ok(Self(bytes[9..].to_vec().into()))
    }
    pub fn fingerprint(&self) -> String {
        use std::fmt::Write;
        let mut text = String::from("sha256:");
        for byte in ring::digest::digest(&ring::digest::SHA256, &self.to_wire()).as_ref() {
            write!(&mut text, "{byte:02x}").expect("String write");
        }
        text
    }
}
pub struct RelayIdentity(Identity);
impl RelayIdentity {
    pub fn generate() -> R<Self> {
        Identity::new()
            .map(Self)
            .map_err(|_| RelayError::Configuration)
    }
    pub fn public_offer(&self) -> PublicOffer {
        PublicOffer(self.0.cert.clone())
    }
}
#[derive(Debug)]
struct Network {
    socket: TcpStream,
    deadline: Option<Instant>,
    poll: bool,
    cancel: Option<Arc<AtomicBool>>,
    setup_finished: Option<Arc<AtomicBool>>,
}
impl Network {
    fn new(socket: TcpStream) -> io::Result<Self> {
        socket.set_nodelay(true)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            deadline: Some(Instant::now() + SETUP_TIMEOUT),
            poll: false,
            cancel: None,
            setup_finished: None,
        })
    }
    fn paired(&mut self) {
        self.poll = true;
        self.deadline = None;
        self.cancel = None;
        self.setup_finished = None;
    }
    fn setup_done(&self) -> bool {
        self.setup_finished
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    }
    fn effective_deadline(&self) -> Option<Instant> {
        if self.setup_done() {
            None
        } else {
            self.deadline
        }
    }
    fn check_cancel(&self) -> io::Result<()> {
        if self.setup_done() {
            return Ok(());
        }
        if self
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
            || (self.setup_finished.is_some() && self.deadline.is_some_and(|d| Instant::now() >= d))
        {
            // A controlled setup deadline is terminal. TimedOut is reserved for
            // retryable socket polling in ByteStream/RelayReader.
            Err(io::ErrorKind::ConnectionAborted.into())
        } else {
            Ok(())
        }
    }
    fn check(until: Instant) -> io::Result<()> {
        if Instant::now() >= until {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(())
        }
    }
}
impl Read for Network {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check_cancel()?;
            if let Some(d) = self.effective_deadline() {
                Self::check(d)?;
            }
            match self.socket.read(b) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && !self.poll => {
                    thread::sleep(Duration::from_millis(1))
                }
                r => return r,
            }
        }
    }
}
impl Write for Network {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let until = self
            .effective_deadline()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(5));
        loop {
            self.check_cancel()?;
            Self::check(until)?;
            match self.socket.write(b) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1))
                }
                r => return r,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}
fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(CHUNK)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_FRAME * 2)
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME))
}
fn retryable(e: &tungstenite::Error) -> bool {
    matches!(e,tungstenite::Error::Io(e)if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::TimedOut))
}
fn ws_error(error: tungstenite::Error) -> io::Error {
    match error {
        tungstenite::Error::Io(error) => error,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            io::ErrorKind::BrokenPipe.into()
        }
        tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        ) => io::ErrorKind::ConnectionReset.into(),
        _ => io::Error::new(
            io::ErrorKind::InvalidData,
            "Encrypted relay protocol failed",
        ),
    }
}
fn peer_setup_error(error: io::Error) -> RelayError {
    if error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
        .is_some()
    {
        return RelayError::PeerAuthentication;
    }
    match error.kind() {
        io::ErrorKind::TimedOut => RelayError::Timeout,
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::NotConnected => RelayError::Transport,
        _ => RelayError::PeerAuthentication,
    }
}
/// Bounded, cancellable setup for a continuing pilot; not a retry/auth policy.
pub(crate) struct SetupControl {
    pub until: Instant,
    pub cancel: Arc<AtomicBool>,
    pub finished: Arc<AtomicBool>,
}

type ClientWs = WebSocket<MaybeTlsStream<Network>>;
struct ByteStream {
    ws: ClientWs,
    bytes: Vec<u8>,
    offset: usize,
}
impl Read for ByteStream {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if b.is_empty() {
            return Ok(0);
        }
        while self.offset == self.bytes.len() {
            match self.ws.read() {
                Ok(Message::Binary(bytes)) if !bytes.is_empty() => {
                    self.bytes = bytes.to_vec();
                    self.offset = 0;
                }
                Ok(Message::Close(_)) => return Ok(0),
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Err(e) if retryable(&e) => return Err(io::ErrorKind::WouldBlock.into()),
                Err(e) => return Err(ws_error(e)),
                _ => return Err(io::ErrorKind::InvalidData.into()),
            }
        }
        let n = b.len().min(self.bytes.len() - self.offset);
        b[..n].copy_from_slice(&self.bytes[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}
impl Write for ByteStream {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = b.len().min(CHUNK);
        if n == 0 {
            return Ok(0);
        }
        self.ws
            .send(Message::Binary(b[..n].to_vec().into()))
            .map_err(ws_error)?;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.ws.flush().map_err(ws_error)
    }
}
enum Secure {
    A(StreamOwned<ClientConnection, ByteStream>),
    B(StreamOwned<ServerConnection, ByteStream>),
}
impl Read for Secure {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::A(s) => s.read(b),
            Self::B(s) => s.read(b),
        }
    }
}
impl Write for Secure {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self {
            Self::A(s) => s.write(b),
            Self::B(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::A(s) => s.flush(),
            Self::B(s) => s.flush(),
        }
    }
}
struct Shared {
    stream: Mutex<Secure>,
    failed: AtomicBool,
}
pub struct RelayReader(Arc<Shared>);
pub struct RelayWriter(Arc<Shared>);
/// Retain for the whole application session; drop cancels even blocked I/O workers.
pub type RelayParts = (RelayReader, RelayWriter, RelayLifetime);
pub struct RelayLifetime(TcpStream);
impl Drop for RelayLifetime {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}
impl Read for RelayReader {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if b.is_empty() {
            return Ok(0);
        }
        loop {
            if self.0.failed.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let result = {
                self.0
                    .stream
                    .lock()
                    .map_err(|_| io::ErrorKind::BrokenPipe)?
                    .read(b)
            };
            match result {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    thread::sleep(Duration::from_millis(2))
                }
                r => {
                    if r.is_err() {
                        self.0.failed.store(true, Ordering::Relaxed)
                    }
                    return r;
                }
            }
        }
    }
}
impl Write for RelayWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.0.failed.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let r = self
            .0
            .stream
            .lock()
            .map_err(|_| io::ErrorKind::BrokenPipe)?
            .write(&b[..b.len().min(CHUNK)]);
        if r.is_err() {
            self.0.failed.store(true, Ordering::Relaxed)
        }
        thread::yield_now();
        r
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.0.failed.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let r = self
            .0
            .stream
            .lock()
            .map_err(|_| io::ErrorKind::BrokenPipe)?
            .flush();
        if r.is_err() {
            self.0.failed.store(true, Ordering::Relaxed)
        }
        r
    }
}
pub fn relay_roots(extra: Option<&Path>) -> R<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = extra {
        let cert = CertificateDer::from_pem_file(path).map_err(|_| RelayError::Configuration)?;
        roots.add(cert).map_err(|_| RelayError::Configuration)?;
    }
    outer_client_config(roots)
}
fn outer_client_config(roots: RootCertStore) -> R<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| RelayError::Configuration)?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}
// Shared path ensures preflight and real sessions enforce identical WSS trust rules.
fn open_wss(
    url: &str,
    token: &AccessToken,
    outer: Arc<ClientConfig>,
) -> R<(ByteStream, RelayLifetime)> {
    open_wss_controlled(url, token, outer, None)
}
fn open_wss_controlled(
    url: &str,
    token: &AccessToken,
    outer: Arc<ClientConfig>,
    control: Option<&SetupControl>,
) -> R<(ByteStream, RelayLifetime)> {
    use std::net::ToSocketAddrs;
    use tungstenite::client::IntoClientRequest;
    let mut request = url
        .into_client_request()
        .map_err(|_| RelayError::Configuration)?;
    let uri = request.uri();
    if uri.scheme_str() != Some("wss")
        || uri.path_and_query().map(|p| p.as_str()) != Some(PATH)
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
    {
        return Err(RelayError::Configuration);
    }
    let host = uri
        .host()
        .ok_or(RelayError::Configuration)?
        .trim_matches(['[', ']'])
        .to_owned();
    let port = uri.port_u16().unwrap_or(443);
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| RelayError::Transport)?;
    let until = control.map_or(Instant::now() + SETUP_TIMEOUT, |c| {
        c.until.min(Instant::now() + SETUP_TIMEOUT)
    });
    let mut socket = None;
    for addr in addresses.take(8) {
        if control.is_some_and(|c| c.cancel.load(Ordering::Relaxed)) {
            return Err(RelayError::Transport);
        }
        let remaining = until
            .checked_duration_since(Instant::now())
            .ok_or(RelayError::Timeout)?;
        let wait = if control.is_some() {
            remaining.min(Duration::from_millis(200))
        } else {
            remaining
        };
        if let Ok(s) = TcpStream::connect_timeout(&addr, wait) {
            socket = Some(s);
            break;
        }
    }
    let socket = socket.ok_or(RelayError::Transport)?;
    let guard = RelayLifetime(socket.try_clone().map_err(|_| RelayError::Transport)?);
    request.headers_mut().insert(
        "authorization",
        tungstenite::http::HeaderValue::from_bytes(&token.header())
            .map_err(|_| RelayError::Configuration)?,
    );
    let mut net = Network::new(socket).map_err(|_| RelayError::Transport)?;
    if let Some(c) = control {
        net.cancel = Some(c.cancel.clone());
        net.setup_finished = Some(c.finished.clone());
        net.deadline = Some(until);
    }
    let (ws, _) = tungstenite::client_tls_with_config(
        request,
        net,
        Some(ws_config()),
        Some(tungstenite::Connector::Rustls(outer)),
    )
    .map_err(|error| match error {
        tungstenite::HandshakeError::Failure(tungstenite::Error::Io(e))
            if e.get_ref()
                .and_then(|e| e.downcast_ref::<rustls::Error>())
                .is_none() =>
        {
            if e.kind() == io::ErrorKind::TimedOut
                || control.is_some_and(|_| Instant::now() >= until)
            {
                RelayError::Timeout
            } else {
                RelayError::Transport
            }
        }
        tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))
            if matches!(response.status().as_u16(), 429 | 503) =>
        {
            RelayError::Capacity
        }
        _ => RelayError::Authentication,
    })?;
    Ok((
        ByteStream {
            ws,
            bytes: Vec::new(),
            offset: 0,
        },
        guard,
    ))
}
/// Verify outer WSS/TLS and transport authentication only; no room or peer enrollment.
/// The connection is closed immediately and never carries clipboard/application data.
pub fn check_relay(url: &str, token: &AccessToken, outer: Arc<ClientConfig>) -> R<()> {
    let (mut stream, _lifetime) = open_wss(url, token, outer)?;
    // A successful upgrade has already verified TLS and the access credential.
    // Do not wait for a peer or a close reply; the lifetime guard cancels the socket.
    let _ = stream.ws.close(None);
    Ok(())
}
pub struct RelayOptions<'a> {
    pub url: &'a str,
    pub room: Room,
    pub side: Side,
    pub token: &'a AccessToken,
    pub peer: &'a PublicOffer,
    pub outer: Arc<ClientConfig>,
}
pub fn connect(identity: &RelayIdentity, options: RelayOptions<'_>) -> R<RelayParts> {
    connect_controlled(identity, options, None)
}
pub(crate) fn connect_controlled(
    identity: &RelayIdentity,
    options: RelayOptions<'_>,
    control: Option<&SetupControl>,
) -> R<RelayParts> {
    let RelayOptions {
        url,
        room,
        side,
        token,
        peer,
        outer,
    } = options;
    let (mut stream, guard) = open_wss_controlled(url, token, outer, control)?;
    // The outer setup cap can be earlier than the controller's total budget.
    let setup_until = match stream.ws.get_ref() {
        MaybeTlsStream::Rustls(s) => s.sock.effective_deadline(),
        _ => None,
    };
    let mut join = b"OCRJ\x01".to_vec();
    join.extend(room.0);
    join.push(side.byte());
    stream
        .ws
        .send(Message::Binary(join.into()))
        .map_err(|error| {
            if matches!(error, tungstenite::Error::Io(_))
                && setup_until.is_some_and(|until| Instant::now() >= until)
            {
                RelayError::Timeout
            } else {
                RelayError::Transport
            }
        })?;
    match stream.ws.get_mut() {
        MaybeTlsStream::Rustls(s) => {
            s.sock.deadline = Some(control.map_or(Instant::now() + PAIR_TIMEOUT, |c| {
                c.until.min(Instant::now() + PAIR_TIMEOUT)
            }))
        }
        _ => return Err(RelayError::Configuration),
    }
    let mut secure = match side {
        Side::A => {
            let mut conn = ClientConnection::new(
                client_config(&identity.0, &peer.0).map_err(|_| RelayError::Configuration)?,
                "localhost"
                    .try_into()
                    .map_err(|_| RelayError::Configuration)?,
            )
            .map_err(|_| RelayError::Configuration)?;
            conn.set_buffer_limit(Some(MAX_FRAME * 2));
            while conn.is_handshaking() {
                conn.complete_io(&mut stream).map_err(|e| {
                    if e.kind() == io::ErrorKind::InvalidData
                        || e.get_ref()
                            .and_then(|e| e.downcast_ref::<rustls::Error>())
                            .is_some()
                    {
                        RelayError::PeerAuthentication
                    } else if control.is_some_and(|c| {
                        Instant::now() >= c.until || c.cancel.load(Ordering::Relaxed)
                    }) {
                        RelayError::Timeout
                    } else {
                        peer_setup_error(e)
                    }
                })?;
            }
            Secure::A(StreamOwned::new(conn, stream))
        }
        Side::B => {
            let mut conn = ServerConnection::new(
                server_config(&identity.0, &peer.0).map_err(|_| RelayError::Configuration)?,
            )
            .map_err(|_| RelayError::Configuration)?;
            conn.set_buffer_limit(Some(MAX_FRAME * 2));
            while conn.is_handshaking() {
                conn.complete_io(&mut stream).map_err(|e| {
                    if e.kind() == io::ErrorKind::InvalidData
                        || e.get_ref()
                            .and_then(|e| e.downcast_ref::<rustls::Error>())
                            .is_some()
                    {
                        RelayError::PeerAuthentication
                    } else if control.is_some_and(|c| {
                        Instant::now() >= c.until || c.cancel.load(Ordering::Relaxed)
                    }) {
                        RelayError::Timeout
                    } else {
                        peer_setup_error(e)
                    }
                })?;
            }
            Secure::B(StreamOwned::new(conn, stream))
        }
    };
    let ws = match &mut secure {
        Secure::A(s) => &mut s.sock.ws,
        Secure::B(s) => &mut s.sock.ws,
    };
    match ws.get_mut() {
        MaybeTlsStream::Rustls(s) => {
            s.sock.paired();
            if let Some(c) = control {
                s.sock.cancel = Some(c.cancel.clone());
                s.sock.setup_finished = Some(c.finished.clone());
                s.sock.deadline = Some(c.until);
            }
        }
        _ => return Err(RelayError::Configuration),
    }
    let shared = Arc::new(Shared {
        stream: Mutex::new(secure),
        failed: AtomicBool::new(false),
    });
    Ok((RelayReader(shared.clone()), RelayWriter(shared), guard))
}
type ServerWs = WebSocket<StreamOwned<ServerConnection, Network>>;
struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
struct Waiting {
    ws: ServerWs,
    side: Side,
    at: Instant,
    _permit: Permit,
}
type Rooms = Arc<Mutex<HashMap<Room, Waiting>>>;
fn outer_server_config(
    certs: Vec<CertificateDer<'static>>,
    key: rustls::pki_types::PrivateKeyDer<'static>,
) -> R<Arc<ServerConfig>> {
    Ok(Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| RelayError::Configuration)?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|_| RelayError::Configuration)?,
    ))
}
pub fn serve(listen: SocketAddr, certificate: &Path, key: &Path, token: Arc<AccessToken>) -> R<()> {
    let certs = CertificateDer::pem_file_iter(certificate)
        .map_err(|_| RelayError::Configuration)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| RelayError::Configuration)?;
    let metadata = std::fs::metadata(key).map_err(|_| RelayError::Configuration)?;
    if !metadata.is_file() {
        return Err(RelayError::Configuration);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(RelayError::Configuration);
        }
    }
    let key = rustls::pki_types::PrivateKeyDer::from_pem_file(key)
        .map_err(|_| RelayError::Configuration)?;
    let tls = outer_server_config(certs, key)?;
    let listener = TcpListener::bind(listen).map_err(|_| RelayError::Transport)?;
    serve_listener(listener, tls, token, Arc::new(AtomicBool::new(false)), None)
}
struct Authorize(Arc<AccessToken>);
impl tungstenite::handshake::server::Callback for Authorize {
    // tungstenite requires the unboxed HTTP ErrorResponse in this exact trait signature.
    #[allow(clippy::result_large_err)]
    fn on_request(
        self,
        request: &tungstenite::handshake::server::Request,
        response: tungstenite::handshake::server::Response,
    ) -> std::result::Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        let supplied = request
            .headers()
            .get("authorization")
            .map(|v| v.as_bytes())
            .unwrap_or_default();
        if request.uri().path_and_query().map(|p| p.as_str()) != Some(PATH)
            || self.0.header().as_slice().ct_eq(supplied).unwrap_u8() != 1
        {
            let mut error =
                tungstenite::handshake::server::ErrorResponse::new(Some("Unauthorized".into()));
            *error.status_mut() = tungstenite::http::StatusCode::UNAUTHORIZED;
            return Err(error);
        }
        Ok(response)
    }
}
fn handle(
    socket: TcpStream,
    tls: Arc<ServerConfig>,
    token: Arc<AccessToken>,
    permit: Permit,
    rooms: Rooms,
    capture: Option<Arc<Mutex<Vec<u8>>>>,
) -> R<()> {
    let net = Network::new(socket).map_err(|_| RelayError::Transport)?;
    let mut conn = ServerConnection::new(tls).map_err(|_| RelayError::Configuration)?;
    let mut net = net;
    while conn.is_handshaking() {
        conn.complete_io(&mut net)
            .map_err(|_| RelayError::Authentication)?;
    }
    let mut ws = tungstenite::accept_hdr_with_config(
        StreamOwned::new(conn, net),
        Authorize(token),
        Some(ws_config()),
    )
    .map_err(|_| RelayError::Authentication)?;
    let msg = ws.read().map_err(|_| RelayError::Protocol)?;
    let Message::Binary(join) = msg else {
        return Err(RelayError::Protocol);
    };
    if join.len() != 38 || &join[..5] != b"OCRJ\x01" {
        return Err(RelayError::Protocol);
    }
    let room = Room(join[5..37].try_into().map_err(|_| RelayError::Protocol)?);
    if room.0 == [0; 32] {
        return Err(RelayError::Protocol);
    }
    let side = match join[37] {
        0 => Side::A,
        1 => Side::B,
        _ => return Err(RelayError::Protocol),
    };
    ws.get_mut().sock.paired();
    let pending = Waiting {
        ws,
        side,
        at: Instant::now(),
        _permit: permit,
    };
    let other = {
        let mut r = rooms.lock().map_err(|_| RelayError::Transport)?;
        r.retain(|_, w| w.at.elapsed() < PAIR_TIMEOUT);
        if let Some(old) = r.get(&room) {
            if old.side == side {
                return Err(RelayError::Capacity);
            }
        } else {
            if r.len() >= MAX_WAITING {
                return Err(RelayError::Capacity);
            }
            r.insert(room, pending);
            return Ok(());
        }
        r.remove(&room).ok_or(RelayError::Protocol)?
    };
    forward(pending, other, capture)
}
fn forward(mut a: Waiting, mut b: Waiting, capture: Option<Arc<Mutex<Vec<u8>>>>) -> R<()> {
    fn transfer(
        from: &mut ServerWs,
        to: &mut ServerWs,
        capture: &Option<Arc<Mutex<Vec<u8>>>>,
    ) -> R<Option<bool>> {
        match from.read() {
            Ok(Message::Binary(bytes)) => {
                if let Some(c) = capture {
                    let mut c = c.lock().map_err(|_| RelayError::Transport)?;
                    if c.len() + bytes.len() <= MAX_FRAME * 4 {
                        c.extend(bytes.as_ref());
                    }
                }
                to.send(Message::Binary(bytes))
                    .map_err(|_| RelayError::Transport)?;
                Ok(Some(true))
            }
            Ok(Message::Ping(_) | Message::Pong(_)) => Ok(Some(true)),
            Ok(Message::Close(_)) => Ok(None),
            Err(e) if retryable(&e) => Ok(Some(false)),
            _ => Err(RelayError::Protocol),
        }
    }
    let until = Instant::now() + SESSION_LIMIT;
    while Instant::now() < until {
        let mut progress = false;
        for direction in 0..2 {
            let result = if direction == 0 {
                transfer(&mut a.ws, &mut b.ws, &capture)?
            } else {
                transfer(&mut b.ws, &mut a.ws, &capture)?
            };
            match result {
                Some(p) => progress |= p,
                None => return Ok(()),
            }
        }
        if !progress {
            thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}
fn serve_listener(
    listener: TcpListener,
    tls: Arc<ServerConfig>,
    token: Arc<AccessToken>,
    stop: Arc<AtomicBool>,
    capture: Option<Arc<Mutex<Vec<u8>>>>,
) -> R<()> {
    listener
        .set_nonblocking(true)
        .map_err(|_| RelayError::Transport)?;
    let active = Arc::new(AtomicUsize::new(0));
    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
    while !stop.load(Ordering::Relaxed) {
        rooms
            .lock()
            .map_err(|_| RelayError::Transport)?
            .retain(|_, w| w.at.elapsed() < PAIR_TIMEOUT);
        match listener.accept() {
            Ok((socket, _)) => {
                let previous = active.fetch_add(1, Ordering::Relaxed);
                if previous >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::Relaxed);
                    drop(socket);
                    continue;
                }
                let permit = Permit(active.clone());
                let (tls, token, rooms, capture) =
                    (tls.clone(), token.clone(), rooms.clone(), capture.clone());
                thread::spawn(move || {
                    let _ = handle(socket, tls, token, permit, rooms, capture);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(_) => return Err(RelayError::Transport),
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;

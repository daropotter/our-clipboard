use super::*;
fn dial(
    url: &str,
    room: Room,
    side: Side,
    token: &AccessToken,
    identity: &RelayIdentity,
    peer: &PublicOffer,
    outer: Arc<ClientConfig>,
) -> R<RelayParts> {
    connect(
        identity,
        RelayOptions {
            url,
            room,
            side,
            token,
            peer,
            outer,
        },
    )
}

struct Harness {
    url: String,
    outer: Arc<ClientConfig>,
    token: Arc<AccessToken>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<R<()>>>,
    capture: Arc<Mutex<Vec<u8>>>,
}
fn token() -> Arc<AccessToken> {
    let mut random = [0; 32];
    getrandom::fill(&mut random).unwrap();
    let text = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    AccessToken::from_bytes(text.as_bytes()).unwrap()
}
impl Harness {
    fn new() -> Self {
        let identity = Identity::new().unwrap();
        let outer = outer_client_config(roots(&identity.cert).unwrap()).unwrap();
        let tls = outer_server_config(vec![identity.cert], identity.key.into()).unwrap();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let capture = Arc::new(Mutex::new(Vec::new()));
        let token = token();
        let (st, cap, t) = (stop.clone(), capture.clone(), token.clone());
        let worker = thread::spawn(move || serve_listener(listener, tls, t, st, Some(cap)));
        Self {
            url: format!("wss://localhost:{}{PATH}", address.port()),
            outer,
            token,
            stop,
            worker: Some(worker),
            capture,
        }
    }
    fn pair(&self, wrong_peer: bool) -> (R<RelayParts>, R<RelayParts>) {
        let a = RelayIdentity::generate().unwrap();
        let b = RelayIdentity::generate().unwrap();
        let ap = a.public_offer();
        let bp = if wrong_peer {
            RelayIdentity::generate().unwrap().public_offer()
        } else {
            b.public_offer()
        };
        let (url, outer, t) = (self.url.clone(), self.outer.clone(), self.token.clone());
        let left = thread::spawn(move || dial(&url, Room([7; 32]), Side::A, &t, &a, &bp, outer));
        let right = dial(
            &self.url,
            Room([7; 32]),
            Side::B,
            &self.token,
            &b,
            &ap,
            self.outer.clone(),
        );
        (left.join().unwrap(), right)
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            assert!(w.join().unwrap().is_ok());
        }
    }
}
fn deadline(a: &RelayLifetime, b: &RelayLifetime) {
    let a = a.0.try_clone().unwrap();
    let b = b.0.try_clone().unwrap();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(8));
        let _ = a.shutdown(Shutdown::Both);
        let _ = b.shutdown(Shutdown::Both);
    });
}
#[test]
fn both_roles_dial_out_and_relay_never_receives_plaintext() {
    let h = Harness::new();
    let (a, b) = h.pair(false);
    let (mut ar, mut aw, ag) = a.expect("A outbound handshake");
    let (mut br, mut bw, bg) = b.expect("B outbound handshake");
    deadline(&ag, &bg);
    aw.write_all(SAMPLE.as_bytes()).unwrap();
    aw.flush().unwrap();
    let mut received = vec![0; SAMPLE.len()];
    br.read_exact(&mut received).unwrap();
    assert_eq!(received, SAMPLE.as_bytes());
    bw.write_all(b"response").unwrap();
    bw.flush().unwrap();
    let mut response = [0; 8];
    ar.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"response");
    let captured = h.capture.lock().unwrap();
    assert!(!captured.is_empty());
    assert!(
        !captured
            .windows(SAMPLE.len())
            .any(|w| w == SAMPLE.as_bytes())
    );
}
#[test]
fn wrong_peer_certificate_never_establishes_an_application_stream() {
    let h = Harness::new();
    let (a, b) = h.pair(true);
    assert!(a.is_err());
    assert!(b.is_err());
}
#[test]
fn outer_certificate_and_access_token_are_verified_before_join() {
    let h = Harness::new();
    let a = RelayIdentity::generate().unwrap();
    let b = RelayIdentity::generate().unwrap();
    assert!(matches!(
        dial(
            &h.url,
            Room([1; 32]),
            Side::A,
            &token(),
            &a,
            &b.public_offer(),
            h.outer.clone()
        ),
        Err(RelayError::Authentication)
    ));
    let empty = outer_client_config(RootCertStore::empty()).unwrap();
    assert!(matches!(
        dial(
            &h.url,
            Room([1; 32]),
            Side::A,
            &h.token,
            &a,
            &b.public_offer(),
            empty
        ),
        Err(RelayError::Authentication)
    ));
    assert!(h.capture.lock().unwrap().is_empty());
}
#[test]
fn plaintext_urls_and_malformed_offers_are_rejected() {
    let h = Harness::new();
    let a = RelayIdentity::generate().unwrap();
    let b = RelayIdentity::generate().unwrap();
    for url in [
        h.url.replacen("wss:", "ws:", 1),
        h.url.clone() + "?credential=forbidden",
    ] {
        assert!(matches!(
            dial(
                &url,
                Room([1; 32]),
                Side::A,
                &h.token,
                &a,
                &b.public_offer(),
                h.outer.clone()
            ),
            Err(RelayError::Configuration)
        ));
    }
    let offer = a.public_offer();
    let bytes = offer.to_wire();
    let decoded = PublicOffer::from_wire(&bytes).unwrap();
    assert_eq!(decoded.fingerprint(), offer.fingerprint());
    for bytes in [&bytes[..bytes.len() - 1], b"invalid".as_slice()] {
        assert!(PublicOffer::from_wire(bytes).is_err());
    }
    assert!(Room::parse(&"0".repeat(64)).is_err());
    assert!(Room::parse("invalid").is_err());
}
#[test]
fn concurrent_full_duplex_streaming_stays_bounded() {
    let h = Harness::new();
    let (a, b) = h.pair(false);
    let (mut ar, mut aw, ag) = a.expect("A");
    let (mut br, mut bw, bg) = b.expect("B");
    deadline(&ag, &bg);
    let n = 512 * 1024;
    let x = thread::spawn(move || {
        aw.write_all(&vec![3; n]).unwrap();
        aw.flush().unwrap()
    });
    let y = thread::spawn(move || {
        bw.write_all(&vec![9; n]).unwrap();
        bw.flush().unwrap()
    });
    let r = thread::spawn(move || {
        let mut out = vec![0; n];
        ar.read_exact(&mut out).unwrap();
        assert!(out.iter().all(|v| *v == 9));
    });
    let mut out = vec![0; n];
    br.read_exact(&mut out).unwrap();
    assert!(out.iter().all(|v| *v == 3));
    x.join().unwrap();
    y.join().unwrap();
    r.join().unwrap();
}

#[test]
fn duplicate_slot_cannot_replace_the_waiting_client() {
    use tungstenite::client::IntoClientRequest;
    let h = Harness::new();
    let open = |side: Side| {
        let mut req = h.url.as_str().into_client_request().unwrap();
        req.headers_mut().insert(
            "authorization",
            tungstenite::http::HeaderValue::from_bytes(&h.token.header()).unwrap(),
        );
        let uri = req.uri();
        let port = uri.port_u16().unwrap();
        let socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
        let (mut ws, _) = tungstenite::client_tls_with_config(
            req,
            Network::new(socket).unwrap(),
            Some(ws_config()),
            Some(tungstenite::Connector::Rustls(h.outer.clone())),
        )
        .map_err(|_| "Test WSS handshake failed; headers omitted")
        .unwrap();
        let mut join = b"OCRJ\x01".to_vec();
        join.extend([4; 32]);
        join.push(side.byte());
        ws.send(Message::Binary(join.into())).unwrap();
        if let MaybeTlsStream::Rustls(s) = ws.get_mut() {
            s.sock.deadline = Some(Instant::now() + Duration::from_secs(3));
        }
        ws
    };
    let mut original = open(Side::A);
    thread::sleep(Duration::from_millis(20));
    let mut duplicate = open(Side::A);
    assert!(duplicate.read().is_err());
    let mut other = open(Side::B);
    original.send(Message::Binary(vec![7; 32].into())).unwrap();
    assert!(matches!(other.read(),Ok(Message::Binary(b))if b.as_ref()==[7;32]));
}

#[cfg(unix)]
#[test]
fn serve_rejects_exposed_key_before_binding() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    // An occupied address lets serve validate real PEM files without leaving
    // a detached server running. Transport proves private-key validation passed;
    // Configuration proves the exposed key was rejected before binding.
    let occupied = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = occupied.local_addr().unwrap();
    for (mode, expected) in [
        (0o600, RelayError::Transport),
        (0o640, RelayError::Configuration),
        (0o644, RelayError::Configuration),
        (0o606, RelayError::Configuration),
    ] {
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(
            serve(address, &cert_path, &key_path, token()),
            Err(expected)
        );
    }
}

#[test]
fn preflight_checks_outer_trust_without_reserving_a_room() {
    let relay = Harness::new();
    check_relay(&relay.url, &relay.token, relay.outer.clone()).unwrap();
    let wrong_token = token();
    assert_eq!(
        check_relay(&relay.url, &wrong_token, relay.outer.clone()),
        Err(RelayError::Authentication)
    );
    let wrong_ca = Identity::new().unwrap();
    assert_eq!(
        check_relay(
            &relay.url,
            &relay.token,
            outer_client_config(roots(&wrong_ca.cert).unwrap()).unwrap()
        ),
        Err(RelayError::Authentication)
    );
    assert_eq!(
        check_relay(
            "ws://localhost/our-clipboard/v1",
            &relay.token,
            relay.outer.clone()
        ),
        Err(RelayError::Configuration)
    );
    assert!(relay.capture.lock().unwrap().is_empty());
    let (left, right) = relay.pair(false);
    assert!(left.is_ok() && right.is_ok());
}

#[test]
fn continuing_setup_observes_cancellation_and_deadline_without_weakening_pins() {
    let h = Harness::new();
    let a = RelayIdentity::generate().unwrap();
    let b = RelayIdentity::generate().unwrap();
    let bp = b.public_offer();
    for cancelled in [true, false] {
        let c = SetupControl {
            until: if cancelled {
                Instant::now() + Duration::from_secs(1)
            } else {
                Instant::now() - Duration::from_secs(1)
            },
            cancel: Arc::new(AtomicBool::new(cancelled)),
            finished: Arc::new(AtomicBool::new(false)),
        };
        let result = connect_controlled(
            &a,
            RelayOptions {
                url: &h.url,
                room: Room([2; 32]),
                side: Side::A,
                token: &h.token,
                peer: &bp,
                outer: h.outer.clone(),
            },
            Some(&c),
        );
        assert!(matches!(
            result,
            Err(RelayError::Transport | RelayError::Timeout)
        ));
    }
    let (left, right) = h.pair(false);
    assert!(left.is_ok() && right.is_ok());
}
#[test]
fn transport_loss_is_retryable_but_malformed_records_remain_terminal() {
    assert_eq!(
        peer_setup_error(io::ErrorKind::UnexpectedEof.into()),
        RelayError::Transport
    );
    assert_eq!(
        peer_setup_error(io::ErrorKind::TimedOut.into()),
        RelayError::Timeout
    );
    assert_eq!(
        peer_setup_error(io::Error::new(
            io::ErrorKind::InvalidData,
            "Malformed peer data"
        )),
        RelayError::PeerAuthentication
    );
    assert_eq!(
        super::super::io_error(ws_error(tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
        ))),
        Error::Io
    );
    assert_eq!(
        super::super::io_error(ws_error(tungstenite::Error::Capacity(
            tungstenite::error::CapacityError::TooManyHeaders
        ))),
        Error::Frame
    );
}

#[test]
fn controlled_pairing_finishes_setup_without_expiring_the_live_stream() {
    let h = Harness::new();
    let a = RelayIdentity::generate().unwrap();
    let b = RelayIdentity::generate().unwrap();
    let ap = a.public_offer();
    let bp = b.public_offer();
    let control = Arc::new(SetupControl {
        until: Instant::now() + Duration::from_secs(1),
        cancel: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
    });
    let (url, token, outer, c) = (
        h.url.clone(),
        h.token.clone(),
        h.outer.clone(),
        control.clone(),
    );
    let left = thread::spawn(move || {
        connect_controlled(
            &a,
            RelayOptions {
                url: &url,
                room: Room([6; 32]),
                side: Side::A,
                token: &token,
                peer: &bp,
                outer,
            },
            Some(&c),
        )
    });
    let right = connect_controlled(
        &b,
        RelayOptions {
            url: &h.url,
            room: Room([6; 32]),
            side: Side::B,
            token: &h.token,
            peer: &ap,
            outer: h.outer.clone(),
        },
        Some(&control),
    );
    let (_, mut writer, _left_guard) = left.join().unwrap().unwrap();
    let (mut reader, _, _right_guard) = right.unwrap();
    control.finished.store(true, Ordering::Relaxed);
    thread::sleep(
        control.until.saturating_duration_since(Instant::now()) + Duration::from_millis(20),
    );
    writer.write_all(b"after-setup").unwrap();
    writer.flush().unwrap();
    let mut bytes = [0; 11];
    reader.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"after-setup");
}

#[test]
fn controlled_missing_peer_times_out_without_blocking_for_the_normal_join_limit() {
    let h = Harness::new();
    let a = RelayIdentity::generate().unwrap();
    let b = RelayIdentity::generate().unwrap();
    let control = SetupControl {
        until: Instant::now() + Duration::from_millis(100),
        cancel: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
    };
    let start = Instant::now();
    let result = connect_controlled(
        &a,
        RelayOptions {
            url: &h.url,
            room: Room([8; 32]),
            side: Side::A,
            token: &h.token,
            peer: &b.public_offer(),
            outer: h.outer.clone(),
        },
        Some(&control),
    );
    assert!(matches!(result, Err(RelayError::Timeout)));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn controlled_application_hello_read_expires_when_peer_withholds_hello() {
    let h = Harness::new();
    let a = RelayIdentity::generate().unwrap();
    let b = RelayIdentity::generate().unwrap();
    let ap = a.public_offer();
    let bp = b.public_offer();
    let control = Arc::new(SetupControl {
        until: Instant::now() + Duration::from_secs(1),
        cancel: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
    });
    let (url, token, outer, c) = (
        h.url.clone(),
        h.token.clone(),
        h.outer.clone(),
        control.clone(),
    );
    let left = thread::spawn(move || {
        connect_controlled(
            &a,
            RelayOptions {
                url: &url,
                room: Room([6; 32]),
                side: Side::A,
                token: &token,
                peer: &bp,
                outer,
            },
            Some(&c),
        )
    });
    let right = connect_controlled(
        &b,
        RelayOptions {
            url: &h.url,
            room: Room([6; 32]),
            side: Side::B,
            token: &h.token,
            peer: &ap,
            outer: h.outer.clone(),
        },
        Some(&control),
    );
    let (_, _, _left_guard) = left.join().unwrap().unwrap();
    let (mut reader, _, right_guard) = right.unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    thread::spawn(move || {
        let mut hello = [0; 1];
        tx.send(reader.read_exact(&mut hello).map_err(|e| e.kind()))
            .unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    drop(right_guard); // Also unblock a regressed implementation before failing.
    assert_eq!(result.unwrap(), Err(io::ErrorKind::ConnectionAborted));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!control.finished.load(Ordering::Relaxed));
}

#[test]
fn cancelling_blocked_wss_setup_io_returns_promptly() {
    let h = Harness::new();
    let control = SetupControl {
        until: Instant::now() + Duration::from_secs(10),
        cancel: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
    };
    let cancel = control.cancel.clone();
    // Wait for a TCP connection before cancelling, so preflight checks cannot
    // satisfy this test without exercising Network I/O cancellation.
    let (ws, guard) =
        open_wss_controlled(&h.url, &h.token, h.outer.clone(), Some(&control)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut ws = ws;
        tx.send(ws.read(&mut [0; 1]).map(|_| ()).map_err(|e| e.kind()))
            .unwrap();
    });
    thread::sleep(Duration::from_millis(100));
    assert!(matches!(
        rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    let start = Instant::now();
    cancel.store(true, Ordering::Relaxed);
    let result = rx.recv_timeout(Duration::from_secs(2));
    drop(guard);
    assert_eq!(result.unwrap(), Err(io::ErrorKind::ConnectionAborted));
    assert!(start.elapsed() < Duration::from_secs(2));
}

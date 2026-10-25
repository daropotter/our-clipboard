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

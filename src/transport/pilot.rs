//! Supervised two-machine pilot over pinned WSS or pretrusted SSH. No OS Idle hooks.
mod link;
use super::outbound::{self, AccessToken, PublicOffer, RelayIdentity, Room, Side};
use super::peer_protocol::{self as wire, Kind, Protocol, PublicPeer, Settings, SigningPeer};
use super::*;
use crate::{
    events::SignedTextEvent,
    freshness::{ClockEstimate, ClockPolicy, ClockProbe, DeliveryDecision},
    receipts::DeliveryTracker,
    session::ClipboardSession,
};
use link::{AttemptError, Link, Reconnector};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader},
    path::Path,
};
use std::{
    io::IsTerminal,
    process::{Child, Command, Stdio},
    sync::{atomic::AtomicBool, mpsc},
};
// Bound network work per iteration so native observation/retries remain serviceable.
const MAX_FRAMES_PER_TICK: usize = 8;
const PROBE_TIMEOUT_MS: u64 = 1000;
const PROBE_INTERVAL_MS: u64 = 5000;
const NATIVE_POLL_MS: u64 = 15;
// RTT uncertainty (at most 100 ms) plus scheduling slack, without renewing TTL.
const BASE_SEND_HOLDBACK_MS: u64 = 250;
// assess() includes elapsed sample age AND distance from the source sample.
// Two 20-second lifetimes give 40_000 MILLISECONDS, not a 40 ms RTT assumption.
const DRIFT_HORIZON_MS: u64 = 2 * wire::CLOCK_SAMPLE_LIFETIME_MS;
const GRANULARITY_MARGIN_MULTIPLIER: u64 = 8;
const APPLY_RETRY_INTERVAL_MS: u64 = 250;
const SEND_RETRY_INTERVAL_MS: u64 = 500;
struct Options {
    stdio: bool,
    host: Option<String>,
    program: Option<String>,
    relay: Option<String>,
    room: Option<Room>,
    side: Option<Side>,
    offer: Option<String>,
    peer_offer: Option<String>,
    relay_ca: Option<String>,
    idle_file: Option<String>,
    pause_file: Option<String>,
    logind_idle: bool,
    macos_session_idle: bool,
    reconnect: bool,
    seconds: u64,
    ttl: u32,
    drift: u32,
    granularity: u64,
}
fn options(args: &[String]) -> std::result::Result<Options, &'static str> {
    let mut o = Options {
        stdio: false,
        host: None,
        program: None,
        relay: None,
        room: None,
        side: None,
        offer: None,
        peer_offer: None,
        relay_ca: None,
        idle_file: None,
        pause_file: None,
        logind_idle: false,
        macos_session_idle: false,
        reconnect: false,
        seconds: 300,
        ttl: 30_000,
        drift: 0,
        granularity: 0,
    };
    let mut consent = false;
    let mut drift = false;
    let mut gran = false;
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while i < args.len() {
        let name = args[i].as_str();
        if !seen.insert(name) {
            return Err("Duplicate pilot option.");
        }
        i += 1;
        if name == "--replace-clipboard" {
            consent = true;
            continue;
        }
        if name == "--reconnect" {
            o.reconnect = true;
            continue;
        }
        if name == "--macos-session-idle" {
            o.macos_session_idle = true;
            continue;
        }
        if name == "--logind-idle" {
            o.logind_idle = true;
            continue;
        }
        if name == "--stdio" {
            o.stdio = true;
            continue;
        }
        let v = args.get(i).ok_or("Missing pilot option value.")?;
        i += 1;
        match name {
            "--relay" => {
                if !v.starts_with("wss://") {
                    return Err("Relay requires a wss:// URL.");
                }
                o.relay = Some(v.clone())
            }
            "--room" => {
                o.room =
                    Some(Room::parse(v).map_err(
                        |_| "Room must be a nonzero 64-character hexadecimal routing ID.",
                    )?)
            }
            "--side" => {
                o.side = Some(match v.as_str() {
                    "a" => Side::A,
                    "b" => Side::B,
                    _ => return Err("Relay side must be a or b."),
                })
            }
            "--offer" => o.offer = Some(v.clone()),
            "--peer-offer" => o.peer_offer = Some(v.clone()),
            "--relay-ca" => o.relay_ca = Some(v.clone()),
            "--pause-file" => {
                if v.is_empty() {
                    return Err("Pause marker path must not be empty.");
                }
                o.pause_file = Some(v.clone());
            }
            "--idle-file" => {
                if v.is_empty() {
                    return Err("Idle marker path must not be empty.");
                }
                o.idle_file = Some(v.clone());
            }
            "--ssh" => {
                if v.is_empty()
                    || v.starts_with('-')
                    || v.chars().any(|c| c.is_whitespace() || c.is_control())
                {
                    return Err("Invalid SSH destination.");
                }
                o.host = Some(v.clone())
            }
            "--remote-program" => {
                if !v.starts_with('/') || v.len() > 4096 || v.chars().any(char::is_control) {
                    return Err("Remote program must be an absolute path.");
                }
                o.program = Some(v.clone())
            }
            "--seconds" => {
                o.seconds = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=3600).contains(n))
                    .ok_or("Pilot duration must be 1-3600 seconds.")?
            }
            "--ttl-ms" => {
                o.ttl = v
                    .parse()
                    .ok()
                    .filter(|n| (1000..=30_000).contains(n))
                    .ok_or("Pilot TTL must be 1000-30000 ms.")?
            }
            "--max-drift-ppm" => {
                o.drift = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=10_000).contains(n))
                    .ok_or("Pilot relative drift bound must be 1-10000 ppm.")?;
                drift = true
            }
            "--clock-granularity-ms" => {
                o.granularity = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=10).contains(n))
                    .ok_or("Pilot granularity bound must be 1-10 ms.")?;
                gran = true
            }
            _ => return Err("Unknown pilot option."),
        }
    }
    if !consent {
        return Err("Pilot requires --replace-clipboard consent on both endpoints.");
    }
    if !drift || !gran {
        return Err(
            "Pilot requires explicit --max-drift-ppm and --clock-granularity-ms assumptions.",
        );
    }
    if o.relay.is_some() {
        if o.stdio || o.host.is_some() || o.program.is_some() {
            return Err("Do not mix relay and SSH/stdio options.");
        }
        if o.room.is_none() || o.side.is_none() || o.offer.is_none() || o.peer_offer.is_none() {
            return Err("Relay pilot requires --room, --side, --offer and --peer-offer.");
        }
        if o.offer == o.peer_offer {
            return Err("Own and peer public offer paths must differ.");
        }
    } else {
        if o.room.is_some()
            || o.side.is_some()
            || o.offer.is_some()
            || o.peer_offer.is_some()
            || o.relay_ca.is_some()
            || o.idle_file.is_some()
            || o.pause_file.is_some()
            || o.logind_idle
            || o.macos_session_idle
            || o.reconnect
        {
            return Err("Relay options require --relay.");
        }
        if o.stdio {
            if o.host.is_some() || o.program.is_some() {
                return Err("Use either internal --stdio or --ssh with --remote-program.");
            }
        } else if o.host.is_none() || o.program.is_none() {
            return Err("Pilot requires --relay or --ssh with --remote-program.");
        }
    }
    if o.seconds * 1000 <= u64::from(o.ttl) + 1000 {
        return Err("Pilot duration must exceed its quiet period by at least one second.");
    }
    Ok(o)
}
fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn ssh(o: &Options) -> std::result::Result<Child, &'static str> {
    let command = format!(
        "{} pilot --replace-clipboard --stdio --seconds {} --ttl-ms {} --max-drift-ppm {} --clock-granularity-ms {}",
        quoted(o.program.as_ref().unwrap()),
        o.seconds,
        o.ttl,
        o.drift,
        o.granularity
    );
    Command::new("ssh")
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            "--",
            o.host.as_ref().unwrap(),
            &command,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(
            |_| "Could not launch SSH. Configure key authentication and trusted known_hosts first.",
        )
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn queue(link: &Option<Link>, kind: Kind, bytes: Vec<u8>) -> Result<bool> {
    match link {
        Some(link) => link.queue(kind, bytes),
        None => Ok(false),
    }
}
struct Outgoing {
    event: SignedTextEvent,
    tracker: DeliveryTracker,
    send_after: u64,
    last_sent: Option<u64>,
}
fn settings(own: PublicPeer, peer: PublicPeer, o: &Options) -> Result<Settings> {
    if own.key == peer.key {
        return Err(Error::Authentication);
    }
    let mut keys = [own.key, peer.key];
    keys.sort();
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    digest.update(b"our-clipboard/ssh-pilot-pair/v1\0");
    for k in keys {
        digest.update(&k)
    }
    let group = digest
        .finish()
        .as_ref()
        .try_into()
        .map_err(|_| Error::Setup)?;
    let membership = ring::digest::digest(
        &ring::digest::SHA256,
        b"our-clipboard/ssh-pilot-two-active-peers/v1\0",
    )
    .as_ref()
    .try_into()
    .map_err(|_| Error::Setup)?;
    Ok(Settings {
        group,
        membership,
        ttl: o.ttl,
        policy: ClockPolicy::new(
            wire::CLOCK_MAX_RTT_MS,
            wire::CLOCK_SAMPLE_LIFETIME_MS,
            Some(o.drift),
            Some(o.granularity),
        )
        .map_err(|_| Error::Setup)?,
    })
}
fn idle_requested(path: Option<&str>) -> Result<bool> {
    match path {
        None => Ok(false),
        // Follow neither file contents nor symlinks: any existing entry pauses.
        Some(path) => {
            let parent = Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            if !std::fs::metadata(parent)
                .map_err(|_| Error::Worker)?
                .is_dir()
            {
                return Err(Error::Worker);
            }
            match std::fs::symlink_metadata(path) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(_) => Err(Error::Worker),
            }
        }
    }
}
fn session(
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
    o: &Options,
    guard: Option<outbound::RelayLifetime>,
    reconnect: Option<Arc<Reconnector>>,
) -> Result<()> {
    let role = if o.relay.is_some() {
        if o.side == Some(Side::A) { "a" } else { "b" }
    } else if o.stdio {
        "remote"
    } else {
        "local"
    };
    let signing = SigningPeer::new()?;
    let own = signing.public();
    let (initial, peer) = Link::open(input, output, own, None, guard, None)?;
    let mut link = Some(initial);
    let mut protocol = Protocol::with_settings(signing, peer, settings(own, peer, o)?)?;
    let mut attempt: Option<mpsc::Receiver<std::result::Result<Link, AttemptError>>> = None;
    let mut failures = 0u32;
    let mut next_attempt = 0u64;
    let mut calibrated = false;
    let mut resumed = false;
    let mut announced_resume = false;
    let started = wire::now()?;
    let end = started + o.seconds * 1000;
    let mut next_probe = started;
    let mut last_verified_clock = started;
    let mut pending_probe: Option<(ClockProbe, u64)> = None;
    let mut estimate: Option<ClockEstimate> = None;
    let mut peer_ready = false;
    let mut sent_ready = false;
    let mut clipboard: Option<ClipboardSession> = None;
    let mut outgoing: Option<Outgoing> = None;
    let mut next_retry = started;
    let mut idle = false;
    #[cfg(target_os = "linux")]
    let lock_monitor = o.logind_idle.then(crate::logind::Monitor::new);
    #[cfg(target_os = "macos")]
    let lock_monitor = o
        .macos_session_idle
        .then(crate::macos_session::Monitor::new);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let mut last_system_state = None;
    eprintln!(
        "[{role}] Authenticated pilot connected. Waiting through the new-session quiet period; startup contents are ignored."
    );
    while wire::now()? < end {
        let now = wire::now()?;
        if idle_requested(o.pause_file.as_deref())? {
            protocol.close();
            eprintln!(
                "[{role}] PAUSED: session ended and pending text discarded; restart with fresh offers."
            );
            return Ok(());
        }
        if o.reconnect
            && link::clock_stalled(now, last_verified_clock)
            && let Some(link) = &link
        {
            link.disconnected();
        }
        if let Some(error) = link.as_ref().and_then(Link::failure) {
            if error != Error::Io {
                return Err(error);
            }
            link = None;
            if reconnect.is_none() {
                eprintln!("[{role}] Pilot peer disconnected; session ended, no replay on restart.");
                return Ok(());
            }
            pending_probe = None;
            sent_ready = false;
            peer_ready = false;
            calibrated = false;
            next_probe = now;
            next_attempt = now + link::delay_ms(0, 0);
            failures = 0;
            eprintln!(
                "[{role}] DISCONNECTED: retaining the existing session; original TTL keeps running."
            );
        }
        if link.is_none()
            && let Some(config) = &reconnect
        {
            if let Some(receiver) = &attempt {
                match receiver.try_recv() {
                    Ok(Ok(next)) => {
                        link = Some(next);
                        attempt = None;
                        pending_probe = None;
                        next_probe = now;
                        sent_ready = false;
                        peer_ready = false;
                        calibrated = false;
                        last_verified_clock = now;
                        resumed = true;
                        announced_resume = false;
                        failures = 0;
                        eprintln!(
                            "[{role}] Transport restored; recalibrating the existing authenticated session."
                        );
                    }
                    Ok(Err(AttemptError::Terminal(error))) => return Err(error),
                    Ok(Err(AttemptError::Retry)) | Err(mpsc::TryRecvError::Disconnected) => {
                        attempt = None;
                        let mut jitter = [0];
                        getrandom::fill(&mut jitter).map_err(|_| Error::Setup)?;
                        next_attempt = now + link::delay_ms(failures, jitter[0]);
                        failures = failures.saturating_add(1);
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if link.is_none() && attempt.is_none() && now >= next_attempt {
                eprintln!("[{role}] RECONNECTING: checking the pinned peer on a new transport.");
                attempt =
                    Some(config.spawn(own, peer, Duration::from_millis(end.saturating_sub(now))));
            }
        }
        let marker_idle = idle_requested(o.idle_file.as_deref()).inspect_err(|_| {
            eprintln!(
                "[{role}] Idle marker unavailable; stopping before further clipboard access."
            );
        })?;
        #[cfg(target_os = "linux")]
        let system_idle = if let Some(monitor) = &lock_monitor {
            let state = monitor.state();
            if last_system_state != Some(state) {
                eprintln!("[{role}] System session: {state:?}; unknown state keeps Idle.");
                last_system_state = Some(state);
            }
            state != crate::logind::State::Active
        } else {
            false
        };
        #[cfg(target_os = "macos")]
        let system_idle = if let Some(monitor) = &lock_monitor {
            let state = monitor.state();
            if last_system_state != Some(state) {
                eprintln!("[{role}] System session: {state:?}; unknown state keeps Idle.");
                last_system_state = Some(state);
            }
            state != crate::macos_session::State::Active
        } else {
            false
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let system_idle = false;
        let requested = marker_idle || system_idle;
        if requested != idle {
            if !requested && let Some(session) = &mut clipboard {
                session.rebaseline_observer().map_err(|_| Error::Worker)?;
            }
            protocol.set_idle(requested)?;
            idle = requested;
            // Never publish a retained local copy after a pause; already queued frames
            // cannot be recalled. Fresh native changes are required after resuming.
            outgoing = None;
            if idle {
                eprintln!(
                    "[{role}] IDLE: local observation, new sends and clipboard writes paused."
                );
            } else {
                eprintln!(
                    "[{role}] ACTIVE: observer rebaselined; only new local copies are published."
                );
            }
        }
        // Reader/writer queues each hold one bounded frame; backpressure never blocks the event loop.
        for _ in 0..MAX_FRAMES_PER_TICK {
            let Some(current) = &link else {
                break;
            };
            match current.incoming.try_recv() {
                Ok(Ok((kind, bytes))) => match kind {
                    Kind::Request => {
                        let reply = protocol.clock_reply(&bytes)?;
                        queue(&link, Kind::Reply, reply)?;
                    }
                    Kind::Reply => {
                        if let Some((probe, _)) = pending_probe.take() {
                            match protocol.finish_probe(probe, &bytes) {
                                Ok(e) => {
                                    estimate = Some(e);
                                    calibrated = true;
                                    last_verified_clock = wire::now()?;
                                }
                                Err(_) => eprintln!(
                                    "[{role}] Pilot clock sample rejected; no unsafe age fallback."
                                ),
                            }
                        }
                    }
                    Kind::Ready => {
                        if bytes != [1] {
                            return Err(Error::Frame);
                        }
                        peer_ready = true;
                    }
                    Kind::Event => {
                        let event = SignedTextEvent::from_wire(&bytes).map_err(|_| Error::Frame)?;
                        if let Some(estimate) = &estimate
                            && sent_ready
                            && peer_ready
                        {
                            let result = if idle {
                                protocol.receive_idle(&event, estimate)
                            } else if let Some(session) = &mut clipboard {
                                protocol.receive_live(&event, estimate, session)
                            } else {
                                continue;
                            };
                            match result.inspect_err(|_e| {
                                eprintln!("[{role}] Receive admission failed.");
                            })? {
                                Some(
                                    d @ (DeliveryDecision::Applied { .. }
                                    | DeliveryDecision::AlreadyApplied { .. }),
                                ) => {
                                    queue(
                                        &link,
                                        Kind::Applied,
                                        protocol.receipt_bytes(&event, d)?,
                                    )?;
                                    if matches!(d, DeliveryDecision::Applied { .. }) {
                                        eprintln!("[{role}] Applied peer event.");
                                    }
                                }
                                None => eprintln!(
                                    "[{role}] Peer write failed; latest event retained for bounded retry."
                                ),
                                Some(DeliveryDecision::BufferedIdle { .. }) => eprintln!(
                                    "[{role}] Peer event buffered while Idle; no application receipt sent."
                                ),
                                Some(_) => eprintln!(
                                    "[{role}] Peer event skipped by age/order/session checks."
                                ),
                            }
                        }
                    }
                    Kind::Applied => {
                        if let Some(out) = &mut outgoing
                            && protocol.observe_receipt(&bytes, &mut out.tracker).is_ok()
                            && out.tracker.progress().complete()
                        {
                            eprintln!("[{role}] Peer confirmed application.");
                            outgoing = None;
                        }
                    }
                    Kind::Retry | Kind::Rejected => return Err(Error::Frame),
                },
                Ok(Err(Error::Frame)) => return Err(Error::Frame),
                Ok(Err(Error::Authentication)) => return Err(Error::Authentication),
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    current.disconnected();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if pending_probe
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_sub(*at) > PROBE_TIMEOUT_MS)
        {
            pending_probe = None;
            next_probe = now;
        }
        if link.is_some() && now >= next_probe && pending_probe.is_none() {
            let probe = protocol.start_probe()?;
            if queue(&link, Kind::Request, probe.request().to_wire())? {
                pending_probe = Some((probe, now));
                next_probe = now + PROBE_INTERVAL_MS;
            }
        }
        if !sent_ready && calibrated && protocol.ready()? {
            sent_ready = queue(&link, Kind::Ready, vec![1])?;
        }
        let transport_ready = link.is_some() && calibrated && sent_ready && peer_ready;
        if resumed && transport_ready && !announced_resume {
            eprintln!("[{role}] RECONNECTED: existing session ready; original TTL unchanged.");
            announced_resume = true;
        }
        if !idle && clipboard.is_none() && transport_ready {
            clipboard = Some(ClipboardSession::new().map_err(|_| Error::Worker)?);
            eprintln!(
                "[{role}] READY: copy new text now. Pilot changes the peer clipboard; contents are never logged."
            );
        }
        if !idle && let Some(session) = &mut clipboard {
            // Poll native local changes before retrying remote state, preserving local-copy order.
            if let Some(text) = session
                .next_local_text(Duration::from_millis(NATIVE_POLL_MS))
                .map_err(|_| {
                    eprintln!("[{role}] Native clipboard observation failed.");
                    Error::Worker
                })?
            {
                eprintln!("[{role}] New local copy selected.");
                let event = protocol.sign(&text)?;
                protocol.record_local(&event).inspect_err(|_e| {
                    eprintln!("[{role}] Local order commit failed.");
                })?;
                let tracker = protocol.tracker(&event)?;
                let send_after = event.context().created_boot_ms
                    + BASE_SEND_HOLDBACK_MS
                    + (DRIFT_HORIZON_MS * u64::from(o.drift))
                        .div_ceil(1_000_000 - u64::from(o.drift))
                    + GRANULARITY_MARGIN_MULTIPLIER * o.granularity;
                outgoing = Some(Outgoing {
                    event,
                    tracker,
                    send_after,
                    last_sent: None,
                });
            }
            if now >= next_retry {
                if transport_ready
                    && let Some(estimate) = &estimate
                    && let Some((event, decision)) =
                        protocol.retry(estimate, session).inspect_err(|_e| {
                            eprintln!("[{role}] Pending-event maintenance failed.");
                        })?
                    && matches!(
                        decision,
                        DeliveryDecision::Applied { .. } | DeliveryDecision::AlreadyApplied { .. }
                    )
                {
                    queue(
                        &link,
                        Kind::Applied,
                        protocol.receipt_bytes(&event, decision)?,
                    )?;
                    eprintln!("[{role}] Applied pending peer event.");
                }
                if !transport_ready && let Some(estimate) = &estimate {
                    protocol.maintain_disconnected(estimate)?;
                }
                next_retry = now + APPLY_RETRY_INTERVAL_MS;
            }
        } else {
            if idle && now >= next_retry {
                if let Some(estimate) = &estimate {
                    if o.reconnect && !transport_ready {
                        protocol.maintain_disconnected(estimate)?;
                    } else {
                        protocol.maintain_idle(estimate)?;
                    }
                }
                next_retry = now + APPLY_RETRY_INTERVAL_MS;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let now = wire::now()?;
        if now >= end {
            break;
        }
        if outgoing.as_ref().is_some_and(|out| {
            now.saturating_sub(out.event.context().created_boot_ms) >= u64::from(o.ttl)
        }) {
            outgoing = None;
            eprintln!("[{role}] Latest local event expired without a confirmed peer application.");
        }
        if transport_ready
            && let Some(out) = &mut outgoing
            && now >= out.send_after
            && out
                .last_sent
                .is_none_or(|last| now.saturating_sub(last) >= SEND_RETRY_INTERVAL_MS)
            && queue(&link, Kind::Event, out.event.to_wire())?
        {
            out.last_sent = Some(now);
        }
    }
    eprintln!("[{role}] Pilot duration ended; temporary session buffers discarded.");
    Ok(())
}
fn save_offer(path: &Path, offer: &PublicOffer) -> std::result::Result<(), &'static str> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(
            |_| "Public offer path must be new and writable; existing files are never replaced.",
        )?;
    file.write_all(&offer.to_wire())
        .and_then(|_| file.sync_all())
        .map_err(|_| "Could not save public offer.")
}
fn wait_offer(path: &Path) -> std::result::Result<PublicOffer, &'static str> {
    let until = Instant::now() + Duration::from_secs(300);
    loop {
        match File::open(path) {
            Ok(file) => {
                if !file
                    .metadata()
                    .map_err(|_| "Could not inspect peer offer.")?
                    .is_file()
                {
                    return Err("Peer offer must be a regular file.");
                }
                let mut bytes = Vec::new();
                file.take(32_778)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "Could not read peer offer.")?;
                return PublicOffer::from_wire(&bytes)
                    .map_err(|_| "Peer public offer is invalid or oversized.");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && Instant::now() < until => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(_) => return Err("Peer public offer unavailable or wait expired."),
        }
    }
}
// sha256: plus 32 digest bytes encoded as hex, CRLF, and one overflow byte.
const MAX_FINGERPRINT_INPUT: u64 = ("sha256:".len() + 32 * 2 + 2 + 1) as u64;
fn confirm_peer(input: &mut impl BufRead, expected: &str) -> std::result::Result<(), &'static str> {
    let mut text = String::new();
    input
        .take(MAX_FINGERPRINT_INPUT)
        .read_line(&mut text)
        .map_err(|_| "Could not read fingerprint confirmation.")?;
    if text.len() as u64 >= MAX_FINGERPRINT_INPUT || text.trim() != expected {
        return Err(
            "Peer fingerprint confirmation failed; no network pairing or clipboard access occurred.",
        );
    }
    Ok(())
}
// Error carries only enum variants, never backend messages, text or keys.
fn relay_session_error(error: Error) -> &'static str {
    match error {
        Error::Setup => {
            "Relay pilot stopped (Setup): check clipboard backend, local state and clock assumptions."
        }
        Error::Io => {
            "Relay pilot stopped (Io): transport or clipboard operation failed; check connectivity and desktop access."
        }
        Error::Authentication => {
            "Relay pilot stopped (Authentication): peer identity or signed session verification failed."
        }
        Error::Frame => {
            "Relay pilot stopped (Frame): invalid protocol frame, receipt or clock calibration."
        }
        Error::Worker => "Relay pilot stopped (Worker): a protocol worker failed.",
    }
}
fn relay_pilot(o: &Options) -> std::result::Result<(), &'static str> {
    let token = AccessToken::from_env().map_err(|e| e.message())?;
    let outer =
        outbound::relay_roots(o.relay_ca.as_deref().map(Path::new)).map_err(|e| e.message())?;
    let identity = RelayIdentity::generate().map_err(|e| e.message())?;
    let offer = identity.public_offer();
    save_offer(Path::new(o.offer.as_ref().unwrap()), &offer)?;
    eprintln!(
        "Own PUBLIC offer saved. Fingerprint: {}",
        offer.fingerprint()
    );
    eprintln!(
        "Transfer only this public offer by your chosen channel; waiting up to five minutes for the peer offer."
    );
    let peer = wait_offer(Path::new(o.peer_offer.as_ref().unwrap()))?;
    eprintln!("Received PUBLIC peer offer: {}", peer.fingerprint());
    eprintln!(
        "Compare with the other computer's OWN fingerprint through an independent trusted channel, then type that full fingerprint and press Enter:"
    );
    confirm_peer(
        &mut BufReader::new(std::io::stdin().lock()),
        &peer.fingerprint(),
    )?;
    let config = Arc::new(Reconnector {
        identity,
        peer,
        url: o.relay.as_ref().unwrap().clone(),
        room: o.room.unwrap(),
        side: o.side.unwrap(),
        token,
        outer,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let _cancel = config.cancel_on_drop();
    let (reader, writer, guard) = config.initial().map_err(|e| e.message())?;
    session(
        reader,
        writer,
        o,
        Some(guard),
        o.reconnect.then_some(config),
    )
    .map_err(relay_session_error)
}
// clipboard-rs emits unstructured stdout diagnostics on X11 ownership changes.
// A pilot exclusively owns this process: preserve its binary output pipe, then
// suppress dependency stdout so diagnostics can never corrupt protocol frames.
#[cfg(unix)]
fn protocol_stdout() -> std::result::Result<std::fs::File, &'static str> {
    // The descriptor is dynamically allocated at or above 3, not fixed to fd 3.
    // The clipboard backend is an in-process library, so it has no Child stdout to pipe.
    let output = rustix::io::fcntl_dupfd_cloexec(std::io::stdout(), 3)
        .map_err(|_| "Could not isolate pilot output.")?;
    let sink = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .map_err(|_| "Could not isolate pilot output.")?;
    rustix::stdio::dup2_stdout(&sink).map_err(|_| "Could not isolate pilot output.")?;
    Ok(output.into())
}
#[cfg(not(unix))]
fn protocol_stdout() -> std::result::Result<std::fs::File, &'static str> {
    Err("This SSH pilot currently supports Linux and macOS endpoints.")
}
pub fn run(args: &[String]) -> std::result::Result<String, &'static str> {
    if matches!(args,[arg] if arg=="--help"||arg=="-h") {
        return Ok("Usage: our-clipboard pilot --replace-clipboard --relay wss://HOST/our-clipboard/v1 --room HEX --side a|b --offer FILE --peer-offer FILE --max-drift-ppm N --clock-granularity-ms N\nOr: our-clipboard pilot --replace-clipboard --ssh USER@HOST --remote-program /absolute/binary --max-drift-ppm N --clock-granularity-ms N [--seconds 300] [--ttl-ms 30000]\n\nOptional --reconnect retries transport loss within the same live authenticated session, without renewing TTL. Relay mode requires OUR_CLIPBOARD_RELAY_TOKEN and independent public-offer fingerprint confirmation, with no SSH or listener on either client. Optional --macos-session-idle observes the macOS caller/console state (experimental registry schema; unknown/stale means Idle). Optional --logind-idle observes the Linux/X11 logind session (unknown/stale means Idle). Optional --idle-file FILE pauses while the marker exists; metadata errors stop the pilot. This injects Idle state, not automatic screen-lock detection. Optional --pause-file FILE ends the session and discards pending text when present; resuming requires fresh pilots/offers. SSH mode requires an existing trusted SSH connection. Wait for both READY statuses. Linux/macOS supervised testing; see docs/OUTBOUND-PILOT.md (relay) or docs/PILOT.md (SSH).".into());
    }
    let o = options(args)?;
    #[cfg(not(target_os = "macos"))]
    if o.macos_session_idle {
        return Err("--macos-session-idle requires macOS; no fallback is enabled.");
    }
    #[cfg(not(target_os = "linux"))]
    if o.logind_idle {
        return Err(
            "--logind-idle requires Linux/X11 with systemd-logind; no fallback is enabled.",
        );
    }
    if idle_requested(o.pause_file.as_deref())
        .map_err(|_| "Pause marker unavailable; pilot not started.")?
    {
        return Ok(
            "PAUSED: pilot not started; remove the marker before creating a fresh session.".into(),
        );
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland")
    {
        return Err(
            "The supervised pilot currently supports X11 on Linux and macOS; native Wayland pilot integration is pending.",
        );
    }

    if o.stdio && (std::io::stdin().is_terminal() || std::io::stdout().is_terminal()) {
        return Err("Internal pilot --stdio requires a pipe, never a terminal.");
    }
    let output = protocol_stdout()?;
    if o.relay.is_some() {
        drop(output);
        relay_pilot(&o)?;
        return Ok(String::new());
    }
    let result = if o.stdio {
        session(std::io::stdin(), output, &o, None, None)
    } else {
        drop(output);
        let mut child = ChildGuard(ssh(&o)?);
        let input = child.0.stdout.take().ok_or("SSH output unavailable.")?;
        let output = child.0.stdin.take().ok_or("SSH input unavailable.")?;
        session(input, output, &o, None, None)
    };
    result.map_err(|_|"Pilot stopped: transport, clock or clipboard operation failed; content and keys are omitted.")?;
    Ok(String::new())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consent_explicit_bounds_and_role_are_required() {
        for args in [
            vec![],
            vec!["--replace-clipboard", "--stdio"],
            vec![
                "--replace-clipboard",
                "--ssh",
                "-ProxyCommand=evil",
                "--remote-program",
                "/bin/true",
                "--max-drift-ppm",
                "1000",
                "--clock-granularity-ms",
                "1",
            ],
        ] {
            assert!(options(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err());
        }
    }
    #[test]
    fn remote_paths_are_shell_quoted() {
        assert_eq!(quoted("/a b/it's"), "'/a b/it'\\''s'");
    }
}

#[cfg(test)]
mod relay_tests {
    use super::*;
    #[test]
    fn idle_marker_is_content_free_and_errors_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paused");
        let name = path.to_str().unwrap();
        assert_eq!(idle_requested(None), Ok(false));
        assert_eq!(
            idle_requested(dir.path().join("missing/marker").to_str()),
            Err(Error::Worker)
        );
        assert_eq!(idle_requested(Some(name)), Ok(false));
        std::fs::write(&path, []).unwrap();
        assert_eq!(idle_requested(Some(name)), Ok(true));
        let invalid = path.join("child");
        assert_eq!(idle_requested(invalid.to_str()), Err(Error::Worker));
        #[cfg(unix)]
        {
            let link = dir.path().join("dangling");
            std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
            assert_eq!(idle_requested(link.to_str()), Ok(true));
        }
    }
    #[test]
    fn relay_requires_complete_role_and_never_mixes_with_ssh() {
        let base = [
            "--replace-clipboard",
            "--relay",
            "wss://relay.example/our-clipboard/v1",
            "--room",
            &"1".repeat(64),
            "--side",
            "a",
            "--offer",
            "own.offer",
            "--peer-offer",
            "peer.offer",
            "--max-drift-ppm",
            "1000",
            "--clock-granularity-ms",
            "1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
        assert!(options(&base).is_ok());
        let mut mixed = base.clone();
        mixed.extend(["--stdio".into()]);
        assert!(options(&mixed).is_err());
        let mut same = base.clone();
        same[10] = "own.offer".into();
        assert!(options(&same).is_err());
        let mut plaintext = base;
        plaintext[2] = "ws://relay.example/our-clipboard/v1".into();
        assert!(options(&plaintext).is_err());
    }
    #[test]
    fn independent_fingerprint_confirmation_is_exact_and_bounded() {
        let expected = RelayIdentity::generate()
            .unwrap()
            .public_offer()
            .fingerprint();
        assert!(
            confirm_peer(
                &mut std::io::Cursor::new(format!("{expected}\n")),
                &expected
            )
            .is_ok()
        );
        assert!(
            confirm_peer(
                &mut std::io::Cursor::new(format!("{expected}\r\n")),
                &expected
            )
            .is_ok()
        );
        for wrong in [
            "yes".to_string(),
            "0".repeat(1000),
            String::new(),
            format!("{expected}{}junk", " ".repeat(100)),
        ] {
            assert!(confirm_peer(&mut std::io::Cursor::new(wrong), &expected).is_err());
        }
    }
    #[test]
    fn relay_errors_preserve_the_safe_protocol_variant() {
        for (error, name) in [
            (Error::Setup, "Setup"),
            (Error::Io, "Io"),
            (Error::Authentication, "Authentication"),
            (Error::Frame, "Frame"),
            (Error::Worker, "Worker"),
        ] {
            assert!(relay_session_error(error).contains(&format!("({name})")));
        }
    }
    #[test]
    fn public_offer_output_never_replaces_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let offer = RelayIdentity::generate().unwrap().public_offer();
        let path = directory.path().join("public.offer");
        save_offer(&path, &offer).unwrap();
        assert!(save_offer(&path, &offer).is_err());
        assert_eq!(
            wait_offer(&path).unwrap().fingerprint(),
            offer.fingerprint()
        );
    }
}

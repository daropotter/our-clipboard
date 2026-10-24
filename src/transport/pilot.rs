//! Explicit two-machine pilot over pretrusted SSH. No discovery, persistent pairing or OS Idle hooks.
use super::peer_protocol::{self as wire, Kind, Protocol, PublicPeer, Settings, SigningPeer};
use super::*;
use crate::{
    events::SignedTextEvent,
    freshness::{ClockEstimate, ClockPolicy, ClockProbe, DeliveryDecision},
    receipts::DeliveryTracker,
    session::ClipboardSession,
};
use std::{
    io::IsTerminal,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, SyncSender, TrySendError},
};
struct Options {
    stdio: bool,
    host: Option<String>,
    program: Option<String>,
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
        if name == "--stdio" {
            o.stdio = true;
            continue;
        }
        let v = args.get(i).ok_or("Missing pilot option value.")?;
        i += 1;
        match name {
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
    if o.stdio {
        if o.host.is_some() || o.program.is_some() {
            return Err("Use either internal --stdio or --ssh with --remote-program.");
        }
    } else if o.host.is_none() || o.program.is_none() {
        return Err("Pilot requires --ssh and --remote-program.");
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
fn queue(sender: &SyncSender<(Kind, Vec<u8>)>, kind: Kind, bytes: Vec<u8>) -> Result<bool> {
    match sender.try_send((kind, bytes)) {
        Ok(()) => Ok(true),
        Err(TrySendError::Full(_)) => Ok(false),
        Err(TrySendError::Disconnected(_)) => Err(Error::Io),
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
        policy: ClockPolicy::new(100, 20_000, Some(o.drift), Some(o.granularity))
            .map_err(|_| Error::Setup)?,
    })
}
fn session(
    input: impl Read + Send + 'static,
    mut output: impl Write + Send + 'static,
    o: &Options,
) -> Result<()> {
    let role = if o.stdio { "remote" } else { "local" };
    let signing = SigningPeer::new()?;
    let own = signing.public();
    own.send(&mut output)?;
    let (hello_tx, hello_rx) = mpsc::sync_channel(1);
    let (incoming_tx, incoming) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut input = input;
        let peer = PublicPeer::receive(&mut input);
        if hello_tx.send(peer).is_err() {
            return;
        }
        loop {
            let frame = wire::read_any(&mut input);
            let stop = frame.is_err();
            if incoming_tx.send(frame).is_err() || stop {
                break;
            }
        }
    });
    let peer = hello_rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| Error::Setup)??;
    let mut protocol = Protocol::with_settings(signing, peer, settings(own, peer, o)?)?;
    let (writer, write_rx) = mpsc::sync_channel::<(Kind, Vec<u8>)>(1);
    thread::spawn(move || {
        while let Ok((kind, bytes)) = write_rx.recv() {
            if wire::send(&mut output, kind, &bytes).is_err() {
                break;
            }
        }
    });
    let started = wire::now()?;
    let end = started + o.seconds * 1000;
    let mut next_probe = started;
    let mut pending_probe: Option<(ClockProbe, u64)> = None;
    let mut estimate: Option<ClockEstimate> = None;
    let mut peer_ready = false;
    let mut sent_ready = false;
    let mut clipboard: Option<ClipboardSession> = None;
    let mut outgoing: Option<Outgoing> = None;
    let mut next_retry = started;
    eprintln!(
        "[{role}] Pilot connected over pretrusted SSH. Waiting through the new-session quiet period; startup contents are ignored."
    );
    while wire::now()? < end {
        let now = wire::now()?;
        // Reader/writer queues each hold one bounded frame; backpressure never blocks the event loop.
        for _ in 0..8 {
            match incoming.try_recv() {
                Ok(Ok((kind, bytes))) => match kind {
                    Kind::Request => {
                        let reply = protocol.clock_reply(&bytes)?;
                        queue(&writer, Kind::Reply, reply)?;
                    }
                    Kind::Reply => {
                        if let Some((probe, _)) = pending_probe.take() {
                            match protocol.finish_probe(probe, &bytes) {
                                Ok(e) => estimate = Some(e),
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
                        if let (Some(estimate), Some(session)) = (&estimate, &mut clipboard) {
                            match protocol
                                .receive_live(&event, estimate, session)
                                .inspect_err(|_e| {
                                    eprintln!("[{role}] Receive admission failed.");
                                })? {
                                Some(
                                    d @ (DeliveryDecision::Applied { .. }
                                    | DeliveryDecision::AlreadyApplied { .. }),
                                ) => {
                                    queue(
                                        &writer,
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
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!(
                        "[{role}] Pilot peer disconnected; session ended, no replay on restart."
                    );
                    return Ok(());
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if pending_probe
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_sub(*at) > 1000)
        {
            pending_probe = None;
            next_probe = now;
        }
        if now >= next_probe && pending_probe.is_none() {
            let probe = protocol.start_probe()?;
            if queue(&writer, Kind::Request, probe.request().to_wire())? {
                pending_probe = Some((probe, now));
                next_probe = now + 5000;
            }
        }
        if !sent_ready && estimate.is_some() && protocol.ready()? {
            sent_ready = queue(&writer, Kind::Ready, vec![1])?;
        }
        if clipboard.is_none() && sent_ready && peer_ready {
            clipboard = Some(ClipboardSession::new().map_err(|_| Error::Worker)?);
            eprintln!(
                "[{role}] READY: copy new text now. Pilot changes the peer clipboard; contents are never logged."
            );
        }
        if let Some(session) = &mut clipboard {
            // Poll native local changes before retrying remote state, preserving local-copy order.
            if let Some(text) =
                session
                    .next_local_text(Duration::from_millis(15))
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
                outgoing = Some(Outgoing {
                    event,
                    tracker,
                    send_after: now
                        + 250
                        + (40_000 * u64::from(o.drift)).div_ceil(1_000_000 - u64::from(o.drift))
                        + 8 * o.granularity,
                    last_sent: None,
                });
            }
            if now >= next_retry {
                if let Some(estimate) = &estimate
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
                        &writer,
                        Kind::Applied,
                        protocol.receipt_bytes(&event, decision)?,
                    )?;
                    eprintln!("[{role}] Applied pending peer event.");
                }
                next_retry = now + 250;
            }
        } else {
            thread::sleep(Duration::from_millis(5));
        }
        if outgoing.as_ref().is_some_and(|out| {
            now.saturating_sub(out.event.context().created_boot_ms) >= u64::from(o.ttl)
        }) {
            outgoing = None;
            eprintln!("[{role}] Latest local event expired without a confirmed peer application.");
        }
        if let Some(out) = &mut outgoing
            && now >= out.send_after
            && out
                .last_sent
                .is_none_or(|last| now.saturating_sub(last) >= 500)
            && queue(&writer, Kind::Event, out.event.to_wire())?
        {
            out.last_sent = Some(now);
        }
    }
    eprintln!("[{role}] Pilot duration ended; temporary session buffers discarded.");
    Ok(())
}
// clipboard-rs emits unstructured stdout diagnostics on X11 ownership changes.
// A pilot exclusively owns this process: preserve its binary output pipe, then
// suppress dependency stdout so diagnostics can never corrupt protocol frames.
#[cfg(unix)]
fn protocol_stdout() -> std::result::Result<std::fs::File, &'static str> {
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
        return Ok("Usage: our-clipboard pilot --replace-clipboard --ssh USER@HOST --remote-program /absolute/binary --max-drift-ppm N --clock-granularity-ms N [--seconds 300] [--ttl-ms 30000]\n\nRequires existing SSH key authentication and a pinned known_hosts entry. Wait for both READY statuses before copying new text. Linux/macOS only; supervised testing, no automatic lock/Idle integration. See docs/PILOT.md.".into());
    }
    let o = options(args)?;
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
    let result = if o.stdio {
        session(std::io::stdin(), output, &o)
    } else {
        drop(output);
        let mut child = ChildGuard(ssh(&o)?);
        let input = child.0.stdout.take().ok_or("SSH output unavailable.")?;
        let output = child.0.stdin.take().ok_or("SSH input unavailable.")?;
        session(input, output, &o)
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

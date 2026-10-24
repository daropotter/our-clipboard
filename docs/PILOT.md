# Interactive X11 ↔ macOS pilot

The `pilot` command synchronizes **new** local text copies in both directions
between a Linux X11 desktop and a macOS desktop (or two X11 desktops). It is an opt-in, supervised test
mode, not the production client or persistent pairing flow. It uses your existing
trusted SSH connection for encryption and peer authentication, then signs events,
calibrates boot clocks and verifies receiver-signed application receipts.

## Prepare both machines

1. Build the same revision on Linux and macOS with stable Rust:

   ```sh
   cargo build --locked
   ./target/debug/our-clipboard doctor --clipboard
   ```

2. Use an X11 session on Linux and a logged-in, unlocked desktop on macOS. On the
   Mac, enable Remote Login for the intended account and grant any clipboard
   access requested by macOS. Run the Mac endpoint as that desktop user.
3. Establish SSH key authentication and verify the Mac's host key through your
   usual trusted process. The pilot requires an existing `known_hosts` entry:

   ```sh
   ssh -T -o BatchMode=yes -o StrictHostKeyChecking=yes USER@MAC true
   ssh -T -o BatchMode=yes -o StrictHostKeyChecking=yes USER@MAC \
     /Users/USER/our-clipboard/target/debug/our-clipboard doctor --clipboard
   ```

   Replace the account, hostname/SSH alias and absolute binary path. A missing
   host-key pin or password-only login fails; the pilot never auto-accepts a key.
   No server program needs to listen on an additional network port.

## Start from the X11 machine

```sh
./target/debug/our-clipboard pilot --replace-clipboard \
  --ssh USER@MAC \
  --remote-program /Users/USER/our-clipboard/target/debug/our-clipboard \
  --max-drift-ppm 1000 --clock-granularity-ms 1 \
  --seconds 300
```

The initiator explicitly launches the remote endpoint with clipboard replacement
consent too. Both machines must use the same revision. Paths are shell-quoted and
SSH runs without a PTY or password prompts. The local command owns its process
output while running; protocol bytes use a dedicated pipe and statuses go to
stderr, without clipboard text or private key material.

The clock bounds above are **explicit test assumptions**, not validated universal
Linux/macOS guarantees. The pilot requires RTT ≤100 ms, refreshes signed samples
every five seconds and stops rather than falling back to UTC if safe age
assessment fails. A small bounded delay before sending keeps messages away from
clock uncertainty boundaries; it does not renew TTL.

The example's 1000 ppm means a 0.1% bound on relative clock-rate difference;
1 ms is an assumed clock-read granularity bound. These are supervised-test
parameters, not certified values for typical hardware. Validating deployment
bounds requires comparing both counters to an independent elapsed-time reference
across normal operation and suspend/resume, accounting for both devices' rate
errors and quantization with margin. Native counter units and successful RTT
samples alone do not establish those worst-case bounds.

The send holdback uses a **40-second drift horizon**: two 20-second calibration
lifetimes for elapsed sample age and source-timestamp distance. It is independent
of the 100 ms maximum measured RTT; the base delay and granularity margin are
additional. Clock-policy errors should be reported with content-free statuses;
changing the assumptions is a new test policy, not proof that the clocks improved.

Wait for **both `[local] READY` and `[remote] READY`**. With the default 30-second
TTL, readiness includes approximately 30 seconds of new-session quiet time. The
native observers start afterward; preexisting clipboard contents are not sent.

## Check the behavior

- Copy new multiline Unicode text on X11 and paste it into a Mac application.
- Copy different text on the Mac and paste it on X11.
- Copy the same text again normally; it is a new event, not a content-hash duplicate.
- Watch `Applied peer event.` and `Peer confirmed application.` statuses. An ACK
  means the expected receiver reported a completed write, not continued ownership.
- Confirm that one copy does not start repeated back-and-forth publications.

Only the latest outbound event is retained for bounded retries, for its original
TTL. The latest failed inbound write may retry too. New local copies are recorded
before publication so lower-order delayed remote events do not replace them.
The command stops after five minutes by default; Ctrl+C ends it sooner. Duration
is configurable up to one hour. Stop before locking either desktop: automatic OS
lock/Idle detection is not connected to this pilot yet.

Every run uses fresh in-memory session/signing keys and temporary metadata.
Disconnect/restart discards text buffers and starts a new quiet period. There is
no reconnect/resume, discovery, persistent group enrollment, multi-hop routing,
production clock policy or background service. A permission/backend/clock/transport
failure ends the session with a content-free error. An absent receipt remains
unconfirmed; it is not interpreted as proof the peer is offline.

## What has been tested

CI builds/tests the code on macOS, Windows and Linux and separately exercises the
native macOS clipboard adapter. The pilot's full interactive CLI path is tested
with **real SSH**, fresh pinned test host keys and two independent Xvfb clipboards
in a private network-disabled container. It checks startup isolation, a new copy
of identical text, bidirectional writes, signed ACKs and absence of echo.

A real X11-to-Mac desktop session is a manual test with your machines; the above
coverage does not claim that cross-machine combination has already been certified.
The pilot currently supports Linux X11/macOS endpoints; Wayland and Windows SSH
endpoints are outside this pilot. Native backend operations can still block on
an unresponsive owner; the duration limit is checked between operations.

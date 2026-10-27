# X11 ↔ macOS with outbound connections only

Neither computer needs SSH, port forwarding or a listening socket. Both run
locally and connect to a manually chosen **WSS relay on HTTPS port 443**. Inside
WSS, an independent pinned mutual TLS 1.3 session encrypts the application data
end to end. The relay cannot read clipboard text or issue a valid peer identity.

This is a supervised two-computer pilot. A separate reachable relay is required;
this repository does not provision hosting or silently choose a public provider.

## Relay operator

The authorized [stage deployment](../deploy/stage/README.md) is available at
`wss://clipboard.stage.daropotter.com/our-clipboard/v1`; obtain its transport
credential through an authorized secret channel.

For an unprivileged container on a separate Linux server, use
[the relay hosting recipe](RELAY-HOSTING.md). No hosting is deployed automatically.

Run the relay on a separate server that is allowed to listen, never on either
restricted workstation. Build the same revision without graphical dependencies:

```sh
cargo build --locked --no-default-features
./target/debug/our-clipboard relay --listen 0.0.0.0:443 \
  --tls-cert /secure/fullchain.pem --tls-key /secure/privkey.pem
```

Use a TLS certificate valid for the relay hostname and a private key with private
Unix permissions. The service needs permission to bind 443, or an explicitly
configured 443 ingress to its unprivileged TLS port. WSS must reach the path
`/our-clipboard/v1`. TLS/HTTP forwarding must preserve WebSocket upgrades and the
Authorization header. No deployment or firewall change is performed automatically.

Inject `OUR_CLIPBOARD_RELAY_TOKEN` into the server and both clients using your
secret-management mechanism. It must be 32–256 ASCII letters/digits/underscores/
hyphens. It grants relay transport access only, not clipboard group membership.
Do not place it in URLs, command arguments, chat, source control or logs.

The experimental server limits active sockets (32), waiting rooms (16), messages
(64 KiB), buffers, join waiting (two minutes) and paired lifetime (one hour).
Credential rotation, fair per-user quotas and hardened public hosting are not yet
implemented. Direct WSS is supported; HTTP CONNECT proxy configuration is not.

## Prepare the computers

Build the same revision with clipboard support on X11 and macOS:

```sh
cargo build --locked
./target/debug/our-clipboard doctor --clipboard
```

First check each computer can reach the chosen relay, without clipboard access:

```sh
./target/debug/our-clipboard relay-check --relay wss://RELAY_HOST/our-clipboard/v1
```

Set `OUR_CLIPBOARD_RELAY_TOKEN` before the check; add `--relay-ca` for a private
CA. Success verifies only outer WSS/TLS/access, not peer trust or text delivery.

Use logged-in, unlocked desktop sessions and grant native macOS clipboard access
if requested. Set the relay access credential through the environment on each.
Use a new working directory or fresh offer filenames for every run.

Choose one shared public routing ID, a nonzero 64-character hexadecimal value.
For example, generate one with `openssl rand -hex 32` and copy the result to both
commands below as `ROUTE_HEX`. This ID is routing metadata, **not a password or
proof of peer trust**. Replace `RELAY_HOST` with the operator's selected hostname.

On X11:

```sh
./target/debug/our-clipboard pilot --replace-clipboard \
  --relay wss://RELAY_HOST/our-clipboard/v1 --room ROUTE_HEX --side a \
  --offer ./x11.offer --peer-offer ./mac-for-x11.offer \
  --max-drift-ppm 1000 --clock-granularity-ms 1 --seconds 300
```

On macOS, run locally in Terminal:

```sh
./target/debug/our-clipboard pilot --replace-clipboard \
  --relay wss://RELAY_HOST/our-clipboard/v1 --room ROUTE_HEX --side b \
  --offer ./mac.offer --peer-offer ./x11-for-mac.offer \
  --max-drift-ppm 1000 --clock-granularity-ms 1 --seconds 300
```

Sides `a` and `b` select TLS/routing roles; **both initiate outbound TCP**. Neither
runs a TLS listening service. For a private relay CA, explicitly add
`--relay-ca /path/to/public-ca.pem` on both computers; hostname verification stays
active. There is no insecure certificate-bypass flag or SSH fallback.

## Confirm identities independently

Each process creates a public `.offer` file and displays its **OWN** fingerprint.
Private session keys stay in memory and are never exported.

1. Transfer `x11.offer` to the Mac as `x11-for-mac.offer`, and `mac.offer` to X11 as
   `mac-for-x11.offer`, through your chosen file-transfer channel. No inbound SSH
   is needed. Publish the destination file only after copying completes, for
   example by copying to a temporary name then renaming it to the expected name.
2. Compare each received fingerprint with the other computer's **OWN** fingerprint
   through an independent trusted channel. A file/key supplied by the relay alone
   is not sufficient authorization.
3. Enter the full `sha256:...` fingerprint from the other computer at the local
   prompt. Do this on both machines. `yes`, an empty response and mismatches fail
   before network pairing or clipboard access.

Offers may wait five minutes for arrival. After confirmations, both clients
should join the same relay room within two minutes. Old offers cannot be reused
with a restarted process: start fresh processes and exchange new offers.
This is manual test enrollment, not the final pairing-code protocol.

## Optional Linux/X11 session observation

On Linux/X11 relay pilots, append `--logind-idle` to observe the session containing
this process through the system D-Bus `org.freedesktop.login1` service. It is
opt-in and unsupported on macOS/Windows; there is no silent fallback there.
No listener, root access, lock command, inhibitor or change to the desktop is
installed. Without the flag, the existing supervised behavior stays unchanged.

A worker first resolves `GetSessionByPID` with the actual pilot PID. Some desktop
terminals run under the user manager and have no direct logind session. In that
case the worker opens the selected local X11 connection, obtains its server PID
from kernel Unix-socket peer credentials, and resolves that PID instead. TCP X11
cannot use this fallback. The returned session's `User` UID must match the pilot's
effective UID; no environment-supplied session ID or guessed user Display is used.
The display connection only establishes/verifies the binding and checks liveness;
it never requests clipboard data. The unique login1 owner/session path are pinned
and properties are read without caching. Only a local `Type=x11`, `Class=user`, `Active=true`,
`LockedHint=false`, `Remote=false` session with manager `PreparingForSleep=false`
can enable clipboard work. Missing/wrong-typed properties, unavailable login1,
failed queries and an unsupported session remain Idle. No graphical clipboard backend is
initialized until an eligible state and the protocol's existing quiet period
and peer readiness permit it.

The event loop never waits on D-Bus. Worker method calls time out at 200 ms and
poll about every 75 ms; samples older than 300 ms become Unknown/Idle. Freshness
uses the native suspend-aware boot clock, timestamped **before** each query, so
an old pre-suspend observation cannot acquire a new timestamp on return. These
are scheduling bounds, not a real-time OS guarantee. A blocked worker cannot
silently leave an indefinitely Active cache.

Status lines distinguish `System session: Active`, `Idle` and `Unknown`. Unknown
always gates the clipboard. A manual `--idle-file` marker combines with this gate
using OR: removing the marker cannot override a locked/unknown system session.
Manual Pause still ends the session immediately when the event loop observes it.
The existing observer rebaseline, original TTL and signed application ACK rules
apply unchanged to system-driven Idle transitions.

The monitor deliberately does not rebind to another service owner/session during
a pilot. A login1 restart, bus loss or failed initial setup requires a fresh
pilot; it does not grant automatic clipboard access. Ordinary property-query
failures on the same live owner can recover after a new valid sample.

**Deployment limitation:** `LockedHint` is cooperative desktop state, not an
OS-enforced security barrier. Not every X11 locker updates it correctly. Querying
state also has polling latency and cannot cancel an already running native call
or queued network frame. Verify your actual desktop/locker updates the hint and
active/sleep state before relying on this experimental gate. CI exercises a
private login1 service and real native clipboards, not a physical desktop lock.
Continue supervised testing; macOS lock observation is still pending, and the
manual Idle marker is a test/control mechanism rather than an automatic hook.

## Explicit local Idle control

Relay pilots optionally accept `--idle-file /absolute/path/to/paused`. This explicitly injects
Idle state for supervised testing; it is not the manual Pause action and does not
detect OS locking automatically. Its parent directory must
already exist and stay accessible; use a private directory owned by your user and
a different marker on each machine. The pilot checks existence without reading
file contents. Any existing entry (including a dangling symlink) pauses; metadata
errors or a missing parent stop the pilot before further clipboard work.

For example, create a private directory before launching:

```sh
mkdir -p "$HOME/.config/our-clipboard"
chmod 700 "$HOME/.config/our-clipboard"
```

Append `--idle-file "$HOME/.config/our-clipboard/paused"` to the local pilot
command. From another local terminal, pause it:

```sh
touch "$HOME/.config/our-clipboard/paused"
```

Wait for the local `[a] IDLE` or `[b] IDLE` status before treating the pause as
acknowledged. While paused, the event loop does not read new local clipboard
text, schedule local event publications or invoke clipboard writes. Already
queued/in-flight network frames and native operations cannot be recalled. The
existing native handles remain alive to preserve clipboard ownership; the gate
is not an OS-level barrier or a cancellation mechanism for blocked native calls.

Clock probes and authenticated inbound admission continue. Only the latest
eligible remote envelope is kept in bounded memory, with its original signed
TTL. Buffered Idle is not an application ACK. Expired/uncertain candidates are
dropped even while paused. Entering Idle discards local pending publication; it
is never replayed automatically after resuming.

To resume, remove only the marker you created:

```sh
unlink "$HOME/.config/our-clipboard/paused"
```

Wait for `ACTIVE` (and `READY` if the pilot started paused). Native notifications
are rebaselined without reading text or releasing ownership, so local copies
made during Idle do not become new publications. A fresh remote candidate can
then apply once and receive its signed application ACK; expired candidates stay
discarded. New local copies after resume are observable normally. Neither
resuming nor pausing restarts the authenticated session or renews TTL.

Manual **Pause** has different semantics: add `--pause-file /absolute/path/to/pause`.
Creating that marker ends the authenticated session and discards its pending text;
there is no in-place resume or buffer replay. Remove the marker and start fresh
pilot processes with new offers/fingerprints and a new quiet period. Ctrl+C also
ends the session. Pause takes priority if both markers exist. These local marker
paths are available only in relay mode and are never sent to the peer.

Native macOS lock integration and physical X11 locker certification are still
pending. The optional Linux/X11 logind observer is described above. Continue
to stop the pilot before locking a real desktop; this gate provides explicit
control and the integration point for future platform signals.

## Copy after both READY messages

Wait for `[a] READY` and `[b] READY`. The default 30-second TTL requires about
30 seconds of new-session quiet time after the authenticated connection is ready.
Only then do observers start; startup contents, including copied fingerprints,
are not published.

Copy new multiline Unicode text on X11 and paste on the Mac, then copy different
text on the Mac and paste on X11. A normal new copy of identical text is another
event. `Applied peer event.` and `Peer confirmed application.` distinguish an
actual write from an unconfirmed send. Retries retain original signed bytes and
TTL, with bounded latest-event buffers. No text is logged or saved as history.

Clock bounds in the examples remain explicit test assumptions, not certification
of typical hardware: 1000 ppm relative drift, 1 ms granularity, RTT ≤100 ms,
signed samples every five seconds with a 20-second lifetime. See
[the clock-assumption explanation](PILOT.md) for validation limits. Unsafe age
assessment fails closed rather than using UTC or renewing TTL.

Stop with Ctrl+C or wait for the five-minute duration. Stop before locking either
desktop: automatic OS-lock/Idle hooks are not wired yet. Native backend operations
can still block between duration checks. Reconnect, discovery, production
membership and persistent device enrollment are outside this pilot.

CI tests paused startup, Idle buffering/expiry/resume and observer rebaseline,
as well as real WSS on port 443, inner pinned TLS, independent confirmations, two
Xvfb clipboards, signed ACKs, no echo and startup isolation in a private
network-disabled container. `strace` asserts no endpoint `listen()` call and no
connection before a rejected fingerprint confirmation. macOS builds and native
adapter tests run separately; a physical X11↔Mac pair remains a manual validation.

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

## Optional reconnect within the same running session

Add `--reconnect` to **both** relay pilot commands to recover from transport loss.
First pairing still requires fresh offers and independent fingerprint confirmation;
this flag does not retry failed initial enrollment or authorize a different peer.
SSH/stdio pilots reject the option. Default relay behavior still ends on disconnect.

While the process stays alive, reconnect retains its original TLS identity,
signing key/session, durable counters/order state, native observer and latest
bounded pending envelopes. New local copies observed while Active but offline
remain eligible only within their **original signed TTL**. Expired text is dropped;
resuming never re-signs or publishes the current clipboard merely because a
connection returned. Idle still suppresses local observation and application.
Manual Pause ends the session and releases pending text, including during setup.

Every replacement transport validates the configured WSS hostname/CA/access
credential and the same independently pinned inner peer certificate. Its public
signing key **and** application session must exactly match the established peer.
Then fresh signed clock calibration and readiness are exchanged. This reconnect
uses the existing application session/quiet-period boundary; it does not create
a new session, reset Lamport/sequence/replay state or renew the TTL. A restarted
peer, changed certificate/key/session, malformed protocol or observed authentication
rejection stops the pilot rather than silently enrolling/retrying it.

Clock estimates are not assumed fresh after reconnect. Native writes/pending
retries wait for refreshed readiness; maintenance during disconnection retains
original expiry and may conservatively discard candidates when clock bounds
become uncertain. The receiver rechecks full signed freshness/order before any
application and produces an application ACK only for a successful write/duplicate.

The event loop stays responsive to Idle/Pause during an attempt. At most one
attempt runs, with a nominal 10-second setup budget capped by the remaining pilot
lifetime. Retryable transport/capacity failures use jittered delays from about
250 ms up to four seconds. All offline time counts toward `--seconds`. OS DNS
resolution and already executing native calls cannot be cancelled by these
budgets; cancelled resolution cannot initiate a new connection if it later returns.
Already queued/in-flight frames cannot be recalled, and partial frames from an
old transport are discarded, never spliced into a new one.

A silent connection also becomes unusable when no signed clock sample has been
validated for 21 seconds (the 20-second calibration lifetime plus probe timeout).
The reconnect owner closes that transport; there is no unsafe age fallback.
`DISCONNECTED`, `RECONNECTING` and `RECONNECTED` report transport/session progress.
`RECONNECTED` confirms refreshed session readiness, not clipboard delivery; look
for the receiver-signed application confirmation separately.

This still uses one explicitly configured relay. It does not discover services,
switch providers, restore a crashed process or persist text. Relay waiting-room,
capacity and session-lifetime limits remain in effect; recovery is not an
availability guarantee. A new process or a manual Pause still requires fresh
offers and the usual new-session quiet period.

CI restarts an isolated real WSS relay between native copies, verifies fresh
offline delivery, expired-text rejection, new copies of identical text after
recovery, no echo, unchanged application session and responsive Pause while the
peer is absent. A separate case changes the relay credential and requires a
terminal authentication error. A long-outage case buffers an Idle event with a
30-second TTL, disconnects for 22 seconds, verifies stale calibration discards the
candidate without ending the session, then delivers a new copy after reconnect.
These fault tests never target the stage service.

## Optional macOS caller/console observation

On macOS relay pilots, append `--macos-session-idle`. First inspect the current
state without pairing, network access or clipboard data access:

```sh
./target/debug/our-clipboard doctor --session
```

It reports only Active/Idle, or exits with Unknown when the native schema cannot
be verified. It never logs a session dictionary, username, UID or clipboard text.
The diagnostic is macOS-only; other systems reject the pilot flag before offers
or transport setup. Existing behavior is unchanged unless you opt in.

A worker reads the **calling process's** Core Graphics session dictionary before
and after an IOKit root-property lookup. The UID must match the effective UID,
the two session snapshots must agree, login must be complete and the caller must
be on console. UID/console-set binding is retained for that pilot; a different
binding stays Unknown rather than silently rebinding. Console-set identifies
hardware, not a cryptographic login identity; stop/restart pilots across logout
or WindowServer replacement.

The IOKit `IOConsoleLocked` property must exist with exact CFBoolean type. Only
an explicit false can permit Active. A present positive session lock hint also
forces Idle; its absence is **not** an unlock signal. Missing/wrong-typed required
fields, null dictionaries, native failures and inconsistent snapshots become
Unknown/Idle. This covers locked startup without trusting an unlock notification
that occurred before the process started. No event-only unlock fallback exists.

Like the Linux gate, observations run separately from clipboard work, about every
75 ms; samples older than 300 ms become Unknown. The suspend-aware boot timestamp
is taken before native I/O, so a blocked or suspended read cannot renew an old
Active result. Framework calls have no cancellation deadline; a stalled worker
leaves the event loop responsive and its cache expires. Manual Idle combines by
OR, Pause takes priority, and resuming retains original TTL/observer rebaseline.

**Experimental schema:** the framework functions are public, but the registry
lock key and supplemental session lock hint are not a supported public lock-state
contract. They are described in Apple's open-source kernel. A macOS change may
leave the gate Unknown, and polling/native operations have latency; this is not
an OS-enforced privacy barrier. No settings, lock commands, inhibitors,
permissions or listeners are installed. Continue supervised operation and stop
pilots before locking real desktops until your OS/lock/suspend behavior has been
verified with synthetic text. Physical Mac lock/unlock/sleep validation remains
manual; CI validates framework linking, strict native types, initial-state policy
and conservative clock/binding transitions without locking the runner.

For the first physical check, keep all clipboard pilots stopped and run only
this local diagnostic loop in Terminal:

```sh
while true; do
  ./target/debug/our-clipboard doctor --session
  sleep 1
done
```

Manually lock/unlock or switch users, then inspect the state lines after returning.
A locked/off-console session must never report Active. Stop with Ctrl+C. Unknown
means this adapter cannot verify that environment; it is not a successful unlock.
The loop needs no clipboard permissions or relay credential. Only after that
check should a separately supervised synthetic-text pilot test exercise the gate.

Sources for the implemented distinction:

- [CGSessionCopyCurrentDictionary](https://developer.apple.com/documentation/coregraphics/cgsessioncopycurrentdictionary())
  and [standard session properties](https://developer.apple.com/documentation/coregraphics/window-server-session-properties).
- Apple's [IOKitKeysPrivate.h](https://github.com/apple-oss-distributions/xnu/blob/main/iokit/IOKit/IOKitKeysPrivate.h)
  identifies the registry schema; [IOService.cpp](https://github.com/apple-oss-distributions/xnu/blob/main/iokit/Kernel/IOService.cpp)
  publishes the console lock state, including system-sleep transitions. These
  implementation details are not treated as a compatibility guarantee.

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
Continue supervised testing; macOS observation has its own experimental adapter above, and the
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

Physical Mac lock/suspend and X11 locker certification are still pending. Both
optional platform observers and their limits are described above. Continue
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
can still block between duration checks. Automatic discovery, production membership and persistent device enrollment are
outside this pilot; relay-only reconnect is opt-in as described above.

CI tests paused startup, Idle buffering/expiry/resume and observer rebaseline,
as well as real WSS on port 443, inner pinned TLS, independent confirmations, two
Xvfb clipboards, signed ACKs, no echo and startup isolation in a private
network-disabled container. `strace` asserts no endpoint `listen()` call and no
connection before a rejected fingerprint confirmation. macOS builds and native
adapter tests run separately; a physical X11↔Mac pair remains a manual validation.

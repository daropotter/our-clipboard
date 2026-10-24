# Our Clipboard

Clipboard sharing between trusted computers, without a permanent host.

## Status

Early development. An opt-in [interactive SSH pilot](docs/PILOT.md) can synchronize
new text copies between two Unix desktops. Production pairing, group routing and
background operation remain under development. This repository grows through
small, tested pull requests. Planned targets are Windows, macOS,
and Linux (X11 and native Wayland); these are goals, not verified support claims.

## Try X11 ↔ macOS

Build this revision on both machines and follow [the pilot guide](docs/PILOT.md).
It uses an already trusted SSH connection, explicit clipboard consent, signed
events, original TTL and receiver ACKs. Wait for both READY statuses before copying.
This is supervised testing; automatic lock/Idle integration is not connected yet.

## Direction

- Plain text first, including Unicode and multiple lines.
- Explicit pairing, cryptographic device identities, and encrypted connections.
- Direct and multi-hop delivery without requiring a full mesh.
- No clipboard history and no clipboard contents in diagnostic logs.
- A headless transport relay separate from trusted clipboard clients.

## Development

Install stable Rust, then run:

```sh
cargo run -- --help
cargo run -- doctor
cargo run -- doctor --clipboard
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The CLI provides help, version information, and runtime platform diagnostics.
The `doctor` command does not read the clipboard. Client and relay commands are not available yet.
CI runs formatting, linting, and CLI tests on Linux, Windows, and macOS.
This does not verify graphical clipboard access on those systems.
All source code, documentation, and pull requests use English.

After the initial repository commit, changes go through pull requests. Each pull
request must provide a working increment and pass its checks. Pending increments
may be stacked, with each pull request targeting the preceding branch. Review and
merge the stack from the bottom up, retargeting dependent pull requests as needed.

## Clipboard diagnostics

`cargo run -- doctor --clipboard` initializes the selected native clipboard
backend and reports its name without reading or writing clipboard contents.
It exits with an error when initialization fails, including missing native
Wayland data-control access. Successful initialization is not proof of read/write
permission, notifications, or synchronization. Use the explicit synthetic probe
below in a disposable session for a read/write check.

## Clipboard access prototype

```sh
cargo run -- clipboard-probe --replace-clipboard
```

This explicit, destructive probe replaces the current clipboard with synthetic
Unicode text and checks a readback. Use a disposable session. It does not save or
restore previous contents, log text, establish pairing, or synchronize devices.
On X11/Wayland, clipboard ownership may end when the process exits.

The prototype uses `arboard` for Windows/macOS/X11 and `wl-clipboard-rs`
for native Wayland data-control. Wayland sessions never silently fall back to
XWayland. The probe reports the backend used after successful verification. It requires
a graphical session and its permissions. GNOME Wayland without compatible data-control can use the explicitly installed
[GNOME Shell 46 adapter prototype](gnome/README.md).

Opt-in integration tests write synthetic multiline Unicode in one process and
verify it in a second process while the clipboard owner remains alive:

```sh
cargo test --locked --test clipboard -- --ignored --test-threads=1
```

CI runs these tests in disposable Windows/macOS runners, Xvfb, headless Sway,
and KWin 6 with a virtual output. The KWin environment uses Debian 13 and
software rendering, with no XWayland, physical GPU, or host clipboard access.
This tests native clipboard data-control, not a full Plasma desktop session.
A separate headless GNOME Shell 46 job tests the extension adapter without XWayland.
Normal `cargo test` skips these destructive graphical tests explicitly.
permission denial, notifications, lock/unlock, and suspend/resume still
need dedicated coverage before platform support can be claimed. Runner tests do
not certify desktop OS releases.

## Automatic stack retargeting

After a PR is merged, the `Retarget stacked PRs` workflow moves open PRs targeting
its source branch to its actual merge target (including targets other than
`main`). Only direct children move; deeper stack layers keep their dependencies.
Already retargeted or closed PRs are skipped, and failed updates fail the workflow
while allowing other children to be processed. Failed runs can be rerun.

The workflow uses `pull_request_target: closed`, repository code from the trusted
workflow revision, and `GITHUB_TOKEN` with pull-request write permission. It never
executes the triggering PR's code. Merged fork heads are ignored because their
branch names do not identify base branches in this repository.

This automation becomes active after the workflow is merged; it does not replay
past merges. Retargeting with `GITHUB_TOKEN` does not itself trigger another Actions
workflow, so rerun CI or push a new commit if fresh checks against the new base
are needed. Retargeting does not rebase commits or merge PRs.

Run its API-mocked regression tests with Node.js 22 or newer:

```sh
node --test .github/scripts/*.test.cjs
```

## Text validation

The shared clipboard adapter accepts at most **10,000,000 UTF-8 bytes** per text
value. Empty strings, embedded NUL, and invalid UTF-8 are rejected. Whitespace,
line endings, and Unicode composition are preserved. Invalid writes are rejected
before touching the native clipboard; rejecting a write never clears it.

Wayland pipe reads stop after the limit plus one byte and reject oversize content
without truncation. The current Windows/macOS/X11 library allocates native reads
before validation, so its read allocation is not yet bounded by this limit.
Wayland payload pipes have a fixed two-second read deadline; origin-marker pipes
have a one-second deadline and a 32-byte limit. A stalled or incomplete transfer
fails without returning partial text, and progress does not renew the deadline.
Compositor negotiation before obtaining a pipe and other native blocking calls
are outside these deadlines. Uniform operation cancellation and native allocation
limits remain future work. General client synchronization is under development;
the supervised SSH pilot provides a limited two-machine path.


## Native notification probe

```sh
cargo run -- watch-probe --observe-clipboard 10
```

The bounded probe subscribes to native X11 XFixes or Wayland data-control notifications and reports
only their count. It never reads clipboard contents, ignores PRIMARY, and does
not emit the selection already present at startup. Repeated copies of identical
text count as separate native changes. Selection clears and owner exits may also
produce notifications; a notification is not yet a publishable text event.

On Wayland, ext-data-control is preferred, with wlr-data-control as fallback.
The initial compositor selection is consumed as a baseline, PRIMARY is ignored,
and offered MIME metadata is discarded without requesting payloads. The observer
uses the first advertised seat; hotplug/reselection needs further coverage.
Windows uses a message-only window subscribed to WM_CLIPBOARDUPDATE. macOS
observes the native NSPasteboard changeCount (metadata polling; no text polling).
Watchers stay on their creating thread and stop at the requested deadline.
Missing Wayland data-control returns an explicit error without XWayland fallback.
Own-write suppression and network synchronization follow separately.

## Local clipboard session prototype

`ClipboardSession` joins native notifications with text validation. It emits new
local text observations and provides `apply_received_text` for future network
messages. Applying text adds a random per-session origin marker in a separate
clipboard format, allowing the session to suppress its own writes. Copying the
same text normally (without that marker) remains a new local observation.
Startup contents are neither read nor emitted, and markers/text are never logged.

This marker is local loop prevention, not a network identity or authorization
proof. Apps that preserve custom clipboard formats can preserve it too; interaction
with clipboard managers still needs dedicated tests. Native reads and marker
queries do not yet have a uniform end-to-end timeout or allocation bound. The X11
tagged writer uses clipboard-rs; ordinary reads/writes continue to use the existing
adapter. There is no network, pairing, Idle handling, or persistent clipboard data
in this prototype.

## Synthetic TLS transport prototype

```sh
cargo run -- transport-probe
cargo run --no-default-features -- transport-probe
cargo test --locked --no-default-features
```

The probe creates ephemeral Ed25519 test identities and exchanges synthetic text
between mutually authenticated TLS 1.3 endpoints over loopback, directly and
through a blind TCP forwarder. Both endpoints reject untrusted peer certificates.
The forwarder never terminates TLS or receives endpoint keys; its bounded byte
capture is test instrumentation, not a clipboard log. Keys and payloads are not
printed or written to files.

Connections (including handshake/partial I/O) and accepts have fixed three-second
deadlines. Experimental framing validates text length before allocating payloads.
`--no-default-features` builds the transport without clipboard, display or GNOME
adapter dependencies; no graphical session is required.

This is not a deployed relay or the final wire protocol. It does not implement
pairing, membership, discovery, NAT traversal, QUIC, UDP-blocked fallback,
reconnection or cross-device clipboard synchronization. The loopback probe checks
TLS behavior and a forwarding path; it does not establish Internet reachability
or select the final transport architecture.

## Two-clipboard integration fixture

CI also runs `synthetic-clipboard-sync` in a private container with two independent
Xvfb servers. Separate sender/receiver processes exchange newly observed synthetic
text in both directions over mutual TLS. Third processes read each destination
clipboard independently before the corresponding applied acknowledgement is sent
or accepted. Own-write notifications must not cause an echo, and preexisting
synthetic baseline contents must not be emitted.

The fixture exchanges public TLS certificates, event-signing keys and session IDs
through trusted anonymous parent/child pipes; private keys stay in their generating
processes. Those setup pipes provide the test membership/key mapping. It requires explicit
`--replace-clipboard` consent and two disposable X11 displays. It is an example/test
helper, not an installed client command, a pairing protocol, or a persistent daemon.

To reproduce in the isolated environment:

```sh
cargo build --locked --example synthetic-clipboard-sync
docker build -t our-clipboard-tests -f tests/linux/Dockerfile .
docker run --rm --network none \
  -v "$PWD/target/debug/examples:/examples:ro" \
  -v "$PWD/tests/linux/run-sync.sh:/run-sync.sh:ro" \
  our-clipboard-tests bash /run-sync.sh /examples/synthetic-clipboard-sync
```

Signed canonical events now pass through `DeliveryReceiver` and temporary durable
metadata before `ClipboardSession` applies them. Both directions obtain signed
boot-clock samples over TLS and wait through a two-second receiver quiet period.
The fixture explicitly assumes 1000 ppm relative drift, one-millisecond granularity,
a 100 ms maximum RTT and 20-second sample lifetime on its controlled test hosts.
These bounds are test assumptions, not production guarantees. An extra 100 ms
send delay keeps sample events away from uncertainty boundaries; TTL is not reset.

Typed bounded frames distinguish clock requests/replies, signed events and
receiver-signed application receipts bound to the event and receiver sessions. Tampered signatures and pre-session/expired
creation times preserve the receiver's baseline clipboard. An injected pre-write
failure requests retransmission of the identical envelope, then a successful
write produces its signed receipt; a further duplicate is acknowledged without
another write. Per-target ledgers verify the pinned receiver key/session, count
that target once, and reject an injected tampered ACK before accepting the valid
original. Independent readers and own-write markers continue checking application
and absence of echo in both directions.

This proves the signed admission/clipboard/observer/TLS/application path for
controlled X11 sessions. Production pairing/membership, deployment clock bounds,
persistent runtime setup, reconnect behavior, multi-hop delivery and cross-platform
end-to-end certification remain pending. The separate headless `transport-probe`
continues using its earlier experimental text framing.

## Persistent client signing identity

```sh
cargo run -- identity init
cargo run -- identity show
cargo run -- identity init --profile workstation
```

An explicit `init` creates a client Ed25519 signing key in the platform credential
store: Windows Credential Manager, macOS Keychain, or Linux Secret Service.
Repeated initialization and later processes keep the same public fingerprint.
`show` never generates a key. Locked/unavailable stores, corrupt records, and a
key that differs from its saved public pin fail without replacement or plaintext
file fallback. The CLI prints the fingerprint, never seed/key bytes.

The configuration directory holds only a versioned public-key pin and an empty
profile lock; writes are serialized and public pins are created atomically without
overwriting existing metadata. If credential saving succeeded before a crash but
the public pin was not created, explicit initialization recovers the pin from that
same credential. A missing credential with an existing pin requires recovery,
not automatic key rotation. Replacement/reset/export commands are not provided.

Profiles accept 1–64 ASCII letters/digits/hyphens/underscores. The default metadata
root is the platform configuration location; `OUR_CLIPBOARD_CONFIG_DIR` explicitly
overrides it. Client credentials use a client-specific namespace, leaving relay
identities separate. System-store protections depend on the user's OS configuration.

For a headless build with a configured credential service:

```sh
cargo run --no-default-features --features os-keyring -- identity show
```

No service means an explicit error. `--no-default-features` alone omits the system
keyring integration. Tests use unique ephemeral credentials on Windows/macOS and
an encrypted, unlocked test collection on a private Linux Secret Service bus;
the host keyring is not touched by the container test. The synthetic transport
fixtures still use ephemeral test identities; persistent signing identity is not
connected to a production pairing/client protocol yet.

## Signed text event prototype

```sh
cargo run --no-default-features -- envelope-probe
```

Canonical versioned envelopes bind group, membership revision, source identity,
source session, sequence, Lamport clock, creation timestamps, original TTL and
UTF-8 text under a domain-separated Ed25519 signature. The full envelope hash
includes its signature; forwarding preserves canonical bytes and never renews
TTL. Decoding checks type/length/context and rejects trailing bytes before copying
text. Content and private keys are never logged or exposed through Debug.

Verification requires a source key from already trusted membership; keys are not
self-authorized by appearing in a message. Only a verified envelope can reach the
replay guard, and only an authenticated handshake layer may activate a source
session. The guard rejects old sessions/membership contexts, detects recent ID
conflicts, retains sequence/Lamport watermarks after digest-cache eviction, and
fails on capacity exhaustion without evicting protection. Re-activating the same
session does not reset its watermark. Older sequences outside the digest cache
are retired, not falsely classified as known identical duplicates.

This is a protocol foundation: signature/replay checks do not establish freshness,
membership rights, handshake freshness or application acknowledgements. The base replay guard is in memory; the durable admission wrapper below
restores watermarks and requires fresh receiver sessions after restart. Signed Unix time
is metadata, not sufficient evidence of age. The signed boot-time timestamp is
used for the suspend-aware freshness checks below and the signed two-clipboard
fixture above. The headless TLS probe retains its experimental text framing.

## Freshness and logical admission prototype

```sh
cargo run --no-default-features -- freshness-probe
```

A one-use random challenge obtains a signed source boot-clock sample bound to
both devices, both sessions, group and membership revision. Delayed/replayed
replies cannot refresh an estimate. Fixed versioned request/reply encodings carry
no clipboard text. Production transport must authenticate membership before
answering clock requests; this exchange alone grants no permissions.

Linux CLOCK_BOOTTIME, Windows QueryInterruptTimePrecise and macOS
mach_continuous_time count suspend time. UTC is display metadata only; there is
no wall-clock fallback. ClockPolicy requires explicit relative drift, granularity,
maximum RTT and calibration lifetime bounds. These are deployment assumptions to
validate before production, not universal guarantees. The probe uses ideal fake
clock bounds only for synthetic data. Native suspend/resume still needs dedicated
VM/desktop validation; ordinary tests simulate elapsed suspend time without
changing the host clock.

Clock estimates conservatively bound age and grow uncertainty with elapsed time
and drift. Expired, future, uncertain, wrong-context or stale-calibration events
cannot enter logical admission. TTL is capped by the receiver and forwarding
never resets it. A new authenticated receiver session mutes for its maximum TTL
and then rejects events created before that boundary. Preserve the same running
session across accidental reconnects; restart/manual rejoin requires a fresh one.

Admission checks freshness before touching replay watermarks or logical clocks.
Concurrent candidates converge by (Lamport, device ID); duplicate deliveries do
not advance clocks. Source session changes retain source Lamport watermarks.
Overflow fails without partial mutation. Only winner metadata is retained, with
no clipboard history. Logical selection is not a clipboard-write acknowledgement:
the actuator must separately track successful application and retries.

The base admission object is in memory; the durable wrapper below persists its
watermarks. The two-clipboard fixture exercises clock negotiation and signed
admission through actual TLS and native writes. Production pairing/membership,
clock policy and persistent client runtime remain to be integrated.

## Durable admission metadata

```sh
cargo run --no-default-features -- state-probe
```

The probe uses temporary metadata and synthetic identities. It never opens the
user's keyring or clipboard. Library callers can use `DurableAdmission` with a
dedicated private directory and a fresh authenticated `ReceiverSession`.
`initialize` explicitly creates a new store; `open` requires an existing valid
store and never silently recreates missing/corrupt/unsupported state. Use one
store per local device and group, with a fixed source capacity of 1–1024. Capacity
changes and recovery/reset policy are not implemented.

Bundled SQLite stores the local publication sequence/Lamport watermark, source
session/membership and sequence/Lamport watermarks, and the selected winner's
order/hash. No clipboard text, event body or private signing key is stored.
Transactions use rollback journaling and `synchronous=EXTRA`; an OS file lock
allows one owning process. Unsigned counters use fixed eight-byte blobs, covering
the complete u64 range. Reservations commit before signing/sending, so a crash
can create a gap but cannot reuse a returned number. Received decisions commit
before being returned; failed commits leave the in-memory state unchanged and
block further mutations until reopening.

Reopening requires a different receiver session and new clock estimates/quiet
period. Persisted source sessions remain inactive until explicitly reauthenticated.
Watermarks survive both restart and authenticated source session changes. Recent
duplicate hashes are intentionally memory-only: after restart, an already-seen
sequence is retired rather than acknowledged as an identical applied event.
Winner selection does not certify clipboard application or provide payload replay;
an actuator must track writes and acknowledgements separately.

The caller must provide a trusted, persistent local directory (0700 on Unix;
Windows uses inherited account ACLs) and retain the lock/database/journal files
together. This protects against process interruption and write failures subject
to the filesystem's sync/locking guarantees, not malicious rollback, deleted
metadata, hardware power-loss certification, network filesystems or an attacker
with access to the account. Startup must not catch `open` errors and automatically
fall back to `initialize`. Tests exercise actual child-process exit with an open
transaction, reopen, replay retention, write failure, scope/version validation,
capacity and counter exhaustion. Production transport integration remains pending.

## Applying admitted deliveries

`DeliveryReceiver` wraps `DurableAdmission` and a synchronous text-write callback.
Only `Applied` or `AlreadyApplied` can become an application receipt, bound to the
full signed-envelope hash and sent over an authenticated channel. Admission or
queue acceptance is never enough. A failed write can retry the exact envelope
while it remains fresh and selected; a newer winner supersedes that retry.
Successful duplicates do not write again or advance Lamport. Freshness is checked
again after durable commit, immediately before invoking the actuator.

The receiver retains one application hash in memory, no payload history. After
restart, retired sequences cannot manufacture application receipts. A receipt
means the callback reported a successful write, not that the clipboard is still
unchanged. Callback errors may follow partial native effects, so exactly-once
external writes are not claimed. The callback must return actual completion,
serialize access and avoid reentry; native operation duration is not bounded here.

## End-to-end application receipts

`SignedApplicationReceipt` binds the full envelope hash, event source/session/
sequence, group/membership, receiver identity/session and an explicit applied
status under the receiver's domain-separated Ed25519 signature. It contains no
clipboard text. Signing accepts only matching `Applied`/`AlreadyApplied`
decisions; verification requires a receiver key from trusted membership. Signers
remain responsible for truthful reports: signatures cannot prove an honest OS.

`DeliveryTracker` keeps one event's fixed set of expected receiver sessions (up to
1024), supplied by authenticated membership/readiness. Only a verified receipt
from that exact receiver/session advances its progress. A neighbor's ACK never
confirms other members; duplicates are idempotent and a missing ACK remains
unconfirmed. Forwarders preserve original receipt bytes and cannot sign for the
destination. Late original application receipts can be recorded after text TTL
expires; the ledger does not reinterpret them as new clipboard writes.

This API currently represents successful application only. Idle/buffering, skip
and error results need separate explicit states; they are not inferred as applied.
The ledger retains metadata only and is not durable. Its owner must bound the
number and lifetime of concurrent trackers. Group membership, routing and UI
integration remain future increments. The two-clipboard TLS fixture exercises
signed receipts in both directions, including tampered ACK rejection and
idempotent per-target progress. Retry/rejection control hashes remain separate
frame types and never count as application receipts.

## Latest-event retry and Idle buffer

`BufferedDelivery` retains at most one canonical signed envelope in a bounded,
zeroizing memory buffer. Idle accepts/adopts newer valid winners without calling
the clipboard actuator or allowing local publication reservations. Leaving Idle
retries only the latest pending winner with its original TTL and session, using
a current authenticated clock estimate. Failed writes can retry the same bytes;
success, expiry/uncertainty, source-session replacement or explicit close clears
pending text. Close seals the session; reopening requires the usual fresh session.
`BufferedIdle` is not an application receipt.

The owner must call maintenance while Idle/disconnected to release expired data,
stop native clipboard observation while Idle and rebaseline it on resume. Native
lock detection and a network Idle announcement are not yet wired to this API.
Buffer limits cover the one retained wire value; decoding/verification uses bounded
scratch copies. Nothing from the buffer is persisted or exposed as history.

Newly observed local copies must reserve numbers, sign the event and call
`record_local_event` before sending. This transaction selects the local winner,
so an older incoming candidate cannot overwrite it. Unreserved/foreign contexts
and conflicting reuse are rejected; selection clears pending remote retries.
A reservation by itself does not claim a clipboard copy. Native observation still
must exclude startup and Idle-era contents.

## License

[MIT](LICENSE), copyright 2026 Daro Potter.

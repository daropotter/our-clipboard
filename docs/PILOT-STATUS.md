# Structured relay pilot status

Add `--status-json` to a relay pilot invocation, or set `"status_json": true`
in a [launch file](PILOT-CONFIG.md). The default is false. This enables a
version 1 JSON Lines stream on stdout; human diagnostics and fingerprint prompts
remain on stderr. It adds no listener, status file, enrollment or clipboard consent.
SSH/stdio mode rejects this option because its stdout is a binary protocol pipe.
The pilot remains experimental on Linux/X11 and macOS, with a separate explicit
[native Wayland data-control opt-in](WAYLAND-PILOT.md).

For example, enable `status_json` in a launch file, then run:

```sh
our-clipboard pilot --config /path/to/x11.json --replace-clipboard | consumer
```

Keep stdin available for fingerprint confirmation unless explicitly selecting a
previously approved peer. Do not merge stderr into this stream. `config-check`
and `preflight` retain their existing human-readable output even when the launch
file enables runtime status. No runtime stream is started by those read-only commands.

Each line is exactly one of these records:

```json
{"version":1,"state":"ready"}
{"version":1,"event":"peer_application_confirmed"}
```

State changes are deduplicated. The states are:

- `starting`: options/platform checks passed and runtime setup is beginning.
- `waiting_for_peer_offer`: a fresh own public offer was saved; the peer offer
  and any required independent confirmation are still pending.
- `connecting`: peer-offer verification/confirmation completed; relay/TLS setup
  is pending. This does not certify a live peer session.
- `synchronizing`: the authenticated link exists, but new-session quiet time,
  clock calibration or the peer's ready frame is still pending.
- `disconnected`: transport loss was detected. With reconnect enabled the same
  live session may retry; original TTL keeps running. Local copies may still be
  observed. No application confirmation is inferred from this state.
- `idle`: the authenticated link exists and local clipboard access is paused by
  the configured marker/system monitor. This does not certify calibration or peer
  readiness. Disconnection takes precedence over Idle in this stream.
- `ready`: the authenticated transport, clock calibration and both ready frames
  have passed, the local native clipboard session exists, and local Idle is off.
  It is a local snapshot, not a delivery receipt or guarantee of continued reachability.
- `paused`: a Pause marker ended/prevented the session. This is terminal; pending
  text is discarded and a new run needs fresh offers.
- `ended`: the run ended normally (duration or disconnect without reconnect).
- `failed`: runtime setup/session failed. Read the content-free diagnostic on stderr.

An initially present Pause marker produces only `paused`, before credentials,
trust, network or clipboard access. Invalid arguments/platform rejection and
failures before stdout isolation may exit without JSON records. Successful normal
runs end in `ended` or `paused`; runtime errors attempt to emit `failed` and exit 2.
Abrupt termination or a broken output stream may leave no terminal record.

Events are independent of readiness transitions:

- `local_copy_selected`: a new local copy was observed, not proof of sending.
- `local_applied`: a peer event was successfully applied to the local clipboard,
  including a still-valid pending event after Idle. Already-applied duplicates
  do not emit this event again.
- `peer_buffered_idle`: the peer's event was admitted to the local Idle buffer;
  it has not been applied and no application ACK is implied.
- `peer_application_confirmed`: the selected peer's authenticated application
  receipt completed the tracker for the latest outstanding local event.
- `local_event_expired`: the outstanding local event reached its original TTL
  without a confirmed peer application. This does not prove it was never applied.

The schema carries no text, text length/hash, event/device/session IDs, names,
paths, URLs, fingerprints, credentials or backend error strings. It is operational
metadata: observation/application timing remains visible. It is not a history,
per-event audit log, persistent snapshot, heartbeat or multi-peer delivery API.
Multiple local copies can replace an outstanding event, and ordinary admission
rejections/maintenance drops need not emit a record. Consumers must tolerate future
versions/event names and use process exit/liveness independently; the last `ready`
record cannot prove a crashed process is still ready.

Drain stdout promptly. Writes are synchronous like human diagnostics; a stalled
consumer can stall the pilot. Output errors stop the run rather than silently
continuing with a misleading monitoring stream. Clipboard libraries' incidental
stdout is isolated from the stream using the existing preserved-output mechanism.

CI exercises opt-in streams with real private WSS, native X11 clipboard writes,
signed application ACKs, Idle, Pause and reconnect; native builds validate the
serializer/options on macOS and Windows. Physical X11↔macOS validation remains
separate; this change does not add Windows pilot support.

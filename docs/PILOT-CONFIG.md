# Local relay launch settings

A client build can run the signed, explicitly approved-peer relay pilot from a
versioned JSON file. This reduces command-line repetition; it does not implement
a daemon, saved clipboard consent, discovery or a new pairing protocol.

Copy the [X11 example](examples/pilot-x11.json) and
[macOS example](examples/pilot-mac.json) into separate per-run working directories
on their respective computers. Edit each copy before use:

- Set the manually chosen WSS relay and the same nonzero 64-hex routing room on both.
  The sample hostname is reserved; it is not a hosted service. The sample room is
  public routing metadata, not a password or proof of trust.
- Select the already initialized local identity profile and explicitly approved
  peer name. Follow [peer trust](PEER-TRUST.md) first; this file cannot approve,
  repair or replace trust.
- Choose fresh own/received offer filenames for this run. Both still exchange
  fresh signed public offers through the chosen file-transfer channel. Existing
  own offer files are never overwritten or removed automatically.

On Unix protect the launch file with `chmod 600 FILE`. It must belong to the
current user and must not be group/world-writable. Use an account-private local
directory on Windows and all other platforms. Treat launch settings and their
parent directories as trusted local configuration: they select the destination
peer/relay and local controls. They are not authenticated membership manifests or
protected against a malicious same-account/administrator change or rollback.
Symlink/non-regular configuration files are rejected. No permissions are repaired.

Check a file without starting a session:

```sh
our-clipboard pilot config-check --file /path/to/run/x11.json
```

This only reads/validates configuration schema and argument consistency. It does
not access the credential service, resolve/contact a relay, initialize clipboard
backends, publish/consume offers or change controls. It does not certify saved
trust, fresh output filenames, CA certificates, native/platform readiness or
end-to-end delivery. It works without a graphical session in a client build;
`--no-default-features` server builds omit this client command/JSON dependency.

Start explicitly:

```sh
our-clipboard pilot --config /path/to/run/x11.json --replace-clipboard
```

The reverse order of these two options also works. `--replace-clipboard` must be
in the actual invocation; it is never saved in JSON. Config mode accepts only
those two options, without overrides, extra flags or recursive includes. Edit the
file to change settings, or use the existing direct command-line mode instead.
Settings are read once at startup; edits affect a new invocation, not an active
session. Existing marker-file controls still act dynamically. No TLS key,
application session or buffered clipboard state is saved in the launch file.
The ordinary pilot startup/quiet period, Idle/Pause, peer verification and signed
application ACK rules remain unchanged. Wait for both READY statuses before copying.

## Schema version 1

The JSON object requires `version: 1`, `relay`, `room`, `side`, `identity_profile`,
`peer_name`, `offer`, `peer_offer`, `max_drift_ppm` and `clock_granularity_ms`.
Only signed mode with an explicitly selected peer approval is supported by launch
files. The existing ephemeral/interactive/SSH command-line modes remain available
separately; configuration files cannot select them.

Optional fields are `seconds` (default 300), `ttl_ms` (default 30000), `relay_ca`,
`idle_file`, `pause_file`, `reconnect`, `logind_idle` and `macos_session_idle`.
Optional `experimental_wayland` explicitly selects the Linux native data-control
[experimental relay pilot](WAYLAND-PILOT.md); its default is false.
Optional `status_json` enables [structured runtime status](PILOT-STATUS.md).
Optional booleans default to false. Numeric bounds and duration-versus-quiet-period
checks are shared with the existing CLI. Native observation flags retain their
existing OS/schema/physical-validation restrictions; a static check does not test
those capabilities.

Paths resolve relative to the launch file's directory, never the invocation's
working directory. Parent directories must already exist; none are created by
validation. Absolute paths also work; Windows drive-relative paths are rejected.
Offer paths resolving to the same parent/filename are rejected. Values are local
platform paths; use the appropriate path syntax on each computer. The identity
configuration root still comes from the existing OS location or explicit
`OUR_CLIPBOARD_CONFIG_DIR` environment override, not from this file.

The parser rejects files above 64 KiB, malformed JSON, duplicate/unknown fields,
unsupported versions, wrong types and invalid path/profile/peer values. Errors
are static and never echo a rejected field/value or JSON body. There is no token,
private-key, clipboard-text, `replace_clipboard`, shell, environment expansion or
command field. URLs cannot contain user credentials, query strings or fragments.
Keep `OUR_CLIPBOARD_RELAY_TOKEN` in its authorized environment/secret channel;
never put credentials anywhere in a launch file, URL, command argument or repo.

Both clients remain outbound-only; the existing relay/stage needs no update.
The private CI fixture checks launch files from a different working directory,
real WSS/native clipboard delivery, saved peer verification with closed stdin,
relay outages, original-TTL expiry and Pause. Rejected fields/no CLI consent and
read-only checking are covered separately. Physical X11↔macOS launch-file testing
remains manual in addition to existing native adapter/store CI.

## Local identity/trust preflight

After static checking, explicitly check local launch prerequisites:

```sh
our-clipboard pilot preflight --file /path/to/run/x11.json
```

Unlike `config-check`, this may access/unlock the OS credential service. It checks
platform-specific option compatibility and session-environment selection, local Pause/Idle marker metadata, that the
own offer path is unused, the environment credential's format, configured CA
parsing, the existing local identity/public pin and the selected owner-signed peer
approval. A configured Pause marker returns PAUSED before credential/trust access.
The command never initializes identities, repairs metadata, creates offers,
changes controls, opens clipboard backends, resolves/connects to a relay or starts
a listener. Unix credential-service IPC may occur; no relay authentication request
is sent. Filesystem reads assume trusted local storage and can still block.

A successful `LOCAL PREFLIGHT PASSED` reports public local/peer fingerprints and
the local marker state. It is a snapshot, **not READY** or a guarantee that a later
launch will succeed: credential validity at the server, relay/peer reachability,
TLS handshake/current peer session, received offers, native clipboard permissions,
actual native lock/suspend state and signed delivery are not tested. In particular,
the peer offer is neither read nor consumed. Use `relay-check` separately for an
explicit network-only check, and wait for both READY statuses after actual launch.
Native session-observation polling/schema limits still apply.

Only existing profile/approval metadata is opened under the existing advisory
profile lock. Missing profile directories or lock files are not created by
preflight, `identity show` or peer reads. If identity metadata needs recovery,
run `identity init` explicitly under its existing fail-closed rules; no replacement
identity/key is generated merely to make a check pass. The command has no
`--replace-clipboard` option and stores no clipboard consent. A later launch still
requires the explicit flag in its own invocation.

The native identity CI matrix preserves headless store tests and separately runs
`native_preflight_reads_persistent_identity_and_peer_trust` with the client enabled
on macOS and Windows. This subprocess test verifies successful reads, rejection
of a corrupted approval, and rejection of missing profile/lock metadata without
repair or offer creation. Linux uses a disposable Secret Service; these checks
must not be run against a personal credential store. They complement the isolated
WSS/clipboard fixture and do not replace physical X11↔macOS validation.

## Local Pause requests and marker inspection

With `pause_file` configured before launch, request session-ending Pause using:

```sh
our-clipboard pilot pause --file /path/to/run/x11.json
our-clipboard pilot controls --file /path/to/run/x11.json
```

`pause` creates an empty marker exclusively at the configured path, relative to
that launch file's directory. Parent directories must already exist. New Unix
markers have permissions no broader than 0600. An existing entry of any kind
already requests Pause and is preserved without opening its contents, following
its symlink, changing permissions, truncating or deleting it. New markers are
synced (including their directory on Unix); a sync error leaves the marker in
place and reports failure. The command does not create a missing configuration
or invent a default `pause_file`.

This only requests Pause: it is **not acknowledgement from a running process**.
A pilot stops/discards pending text when its loop next observes the marker; a
process blocked in startup, filesystem access or output may not observe it
immediately. Verify process exit or its terminal `paused` record in the
[structured status stream](PILOT-STATUS.md). This is separate from the command's
success output. The command also prevents subsequent starts with that marker.
It never resumes/restarts a session or removes a marker. To start again, explicitly
remove the intended marker, choose fresh offer files and run a new pilot; the
ordinary quiet period and clipboard consent still apply. It does not revive old
buffered text. Idle remains a separate, non-terminal gate.

`controls` reads marker metadata and reports `present`, `absent` or
`not configured` separately for Pause and Idle. Metadata errors fail rather than
claiming absence. This is a local snapshot, not a process query or a claim of
READY, native screen-lock state, peer reachability or delivery. Both commands
parse the launch file afresh, without platform/backend initialization, credentials,
relay access or clipboard access; they work even without an initialized identity.
The `status_json` setting affects a runtime pilot, not these commands' output.
They reject `--replace-clipboard` and other extra options.

Use the same trusted configuration and local filesystem as the running pilot.
An active pilot uses its startup settings; editing the launch file before a
control command can select a different marker. There is no process/session
binding, remote control, new listener or protection against malicious same-account
configuration/path changes. The command cannot override native/system Idle.

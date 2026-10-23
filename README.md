# Our Clipboard

Clipboard sharing between trusted computers, without a permanent host.

## Status

Early development. Clipboard synchronization is not implemented yet. This repository
will grow through small, tested pull requests. Planned targets are Windows, macOS,
and Linux (X11 and native Wayland); these are goals, not verified support claims.

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
a graphical session and its permissions. GNOME Wayland without a compatible
data-control protocol needs a separate adapter; it is not supported by this probe.

Opt-in integration tests write synthetic multiline Unicode in one process and
verify it in a second process while the clipboard owner remains alive:

```sh
cargo test --locked --test clipboard -- --ignored --test-threads=1
```

CI runs these tests in disposable Windows/macOS runners, Xvfb, headless Sway,
and KWin 6 with a virtual output. The KWin environment uses Debian 13 and
software rendering, with no XWayland, physical GPU, or host clipboard access.
This tests native clipboard data-control, not a full Plasma desktop session.
Normal `cargo test` skips these destructive graphical tests explicitly. GNOME,
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
Blocking-owner timeouts and notification handling remain future work. These rules
prepare clipboard access; network synchronization is not implemented yet.

## License

[MIT](LICENSE), copyright 2026 Daro Potter.

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
Missing data-control and other platforms return explicit errors without an
XWayland fallback. Own-write suppression, other platform watchers, and network
synchronization follow separately.

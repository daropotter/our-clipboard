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
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The CLI provides help, version information, and runtime platform diagnostics.
It does not read the clipboard. Client and relay commands are not available yet.
CI runs formatting, linting, and CLI tests on Linux, Windows, and macOS.
This does not verify graphical clipboard access on those systems.
All source code, documentation, and pull requests use English.

After the initial repository commit, changes go through pull requests. Each pull
request must provide a working increment and pass its checks. Pending increments
may be stacked, with each pull request targeting the preceding branch. Review and
merge the stack from the bottom up, retargeting dependent pull requests as needed.

## License

[MIT](LICENSE), copyright 2026 Dariusz Wojciechowski.

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

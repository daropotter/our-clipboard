# Explicit local peer approvals

`peer trust` saves one independently approved device key under a local name.
`peer show` authenticates and displays that approval in a later process. These
commands need the `os-keyring` feature (enabled by default), but no graphical
session, relay credential, network connection or clipboard access.

Initialize the local identity explicitly if it does not exist:

```sh
our-clipboard identity init --profile workstation
```

Obtain a **signed** public offer from the other computer using the relay pilot's
`--identity-profile` mode. Use the exact room and the offer issuer's side from
that exchange. On X11, for example, approving the Mac's side-b offer:

```sh
our-clipboard peer trust --profile workstation --name mac \
  --offer mac-for-x11.offer --room YOUR_64_HEX_ROOM --side b
our-clipboard peer show --profile workstation --name mac
```

The command verifies the offer's signature, room and role, displays the local
and remote public device fingerprints, and asks for the remote full
`ed25519-sha256-v1:...` fingerprint. Compare it with the other computer's
`identity show` output over an independent trusted channel before entering it.
`yes`, EOF, truncated/mismatched fingerprints and unsigned/invalid offers do not
save trust. Approve the opposite direction separately on the other computer.
A signed file alone is not authorization, liveness or proof of current reachability.
An older signed offer can identify the stable key for this offline command;
it cannot be used to restore its old application or TLS session.

The selected profile must already exist in the OS credential store. Missing,
locked or corrupt identity storage fails without creating a replacement key or
recovering public metadata. Profile names and peer names use 1–64 ASCII letters,
digits, hyphens or underscores. Names are exact local labels, not remote claims.

## Storage and failure behavior

The same configuration root as `identity init/show` is used, including an explicit
`OUR_CLIPBOARD_CONFIG_DIR` override. Each approval is public metadata at
`PROFILE/peers/NAME.pin`: a versioned canonical owner key, peer key and exact name,
signed by the local device identity under a separate domain. Private keys stay
in the OS credential service. Peer relationships remain local metadata; the
command never sends them to a relay or another device.

The existing per-profile lock serializes identity and peer metadata operations.
The current local public identity pin must match the loaded owner key. Reads are
bounded and reject non-regular records, wrong owner/name, altered signatures,
truncation and trailing bytes. On Unix the profile/peer directories must be
private (0700); symlink peer directories/files are rejected. Windows inherits the
account's configuration-directory ACLs; use trusted account-private local storage.
New files are created atomically without clobbering any existing name, with file
sync and Unix directory sync before success. These are local filesystem checks,
not hardware power-loss certification or network-filesystem support.

A missing name is an error. A corrupt record, an existing name or a changed key
is never repaired, replaced or automatically trusted. Repeating `peer trust` for
the same name fails even for the same key; inspect it with `peer show` instead.
An approval is immutable in this prototype. There is no replace/remove/rotate or
distributed revocation protocol. Protect local metadata/backups against deletion,
rollback and same-account/administrator compromise; signatures do not supply
anti-rollback state. Restoring an old valid approval does not prove it is current.

These approvals authorize only an explicitly selected local experimental peer
relationship. They are not group membership, invitation/removal rights, a group
manifest, relay access credentials or the final six-digit-code enrollment flow.
This increment provides storage and offline CLI management; pilots still require
manual fingerprint confirmation until explicit pin selection is integrated.

CI tests authenticated reload, byte tampering, wrong owner/name, immutable names,
missing/corrupt records, locks, path validation, symlinks and exposed directories.
The native identity-store test also runs trust/show in separate processes with
real disposable Secret Service/Keychain/Credential Manager entries. Host stores
are not used by the private Linux container fixture.

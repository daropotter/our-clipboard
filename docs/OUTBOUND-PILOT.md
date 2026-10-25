# X11 ↔ macOS with outbound connections only

Neither computer needs SSH, port forwarding or a listening socket. Both run
locally and connect to a manually chosen **WSS relay on HTTPS port 443**. Inside
WSS, an independent pinned mutual TLS 1.3 session encrypts the application data
end to end. The relay cannot read clipboard text or issue a valid peer identity.

This is a supervised two-computer pilot. A separate reachable relay is required;
this repository does not provision hosting or silently choose a public provider.

## Relay operator

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

CI tests real WSS on port 443, inner pinned TLS, independent confirmations, two
Xvfb clipboards, signed ACKs, no echo and startup isolation in a private
network-disabled container. `strace` asserts no endpoint `listen()` call and no
connection before a rejected fingerprint confirmation. macOS builds and native
adapter tests run separately; a physical X11↔Mac pair remains a manual validation.

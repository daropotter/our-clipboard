# Run the WSS relay on a separate server

This recipe packages the existing experimental relay. It does not provision a
server, domain, certificate or firewall, and must not run on either outbound-only
workstation. Choose a separate Linux Docker host allowed to accept HTTPS/WSS on
TCP 443. Both clipboard clients continue to initiate outgoing connections only.

The relay handles encrypted records and cannot decrypt clipboard text. Its access
credential authorizes transport only; clients must still independently confirm
peer fingerprints as described in [the outbound pilot guide](OUTBOUND-PILOT.md).

## Build the image

From the repository root at the same revision used by both clients:

```sh
docker build -t our-clipboard-relay -f deploy/relay.Dockerfile .
```

The multi-stage build uses `cargo build --locked --release --no-default-features`.
The runtime contains no graphical backend or Rust toolchain, runs as numeric
UID/GID `65532:65532`, and binds unprivileged port 8443. The build context allows
only source/manifests and selected Dockerfiles; assumptions, `.env` files, TLS
keys and local build artifacts are excluded. No image is published automatically.
Base image tags follow their maintained releases; record the built image ID for
rollback rather than assuming a rebuild produces identical bytes.

## Prepare credentials on the server

Obtain a certificate chain valid for your chosen DNS hostname through your own
certificate-management process. The full chain is public; the private key and
transport access credential are secrets. Keep them outside the repository.

The following commands assume a rootful Linux Docker host without user-namespace
remapping. For rootless/remapped Docker, provision ownership through that
runtime's UID mapping; do not loosen key permissions to make mounts readable.

```sh
sudo install -d -m 0700 /srv/our-clipboard
sudo install -d -o 65532 -g 65532 -m 0700 /srv/our-clipboard/tls
sudo install -o 65532 -g 65532 -m 0444 /path/to/fullchain.pem \
  /srv/our-clipboard/tls/fullchain.pem
sudo install -o 65532 -g 65532 -m 0600 /path/to/privkey.pem \
  /srv/our-clipboard/tls/privkey.pem
```

Through your secret manager, create `/srv/our-clipboard/relay.env`, readable only
by the server operator (`0600`), containing one environment assignment named
`OUR_CLIPBOARD_RELAY_TOKEN`. Its value must be 32–256 ASCII letters, digits,
underscores or hyphens. Supply the same credential to both clients through their
environment. Never put the value in a command argument, URL, chat or diagnostic
output. The container receives it via `--env-file`; Docker administrators can
inspect container environment, so restrict access to the Docker daemon.

## Start the service

Run this **on the separate server**, after checking port 443 is available:

```sh
sudo docker run -d --name our-clipboard-relay --restart unless-stopped \
  --read-only --cap-drop ALL --security-opt no-new-privileges:true \
  --pids-limit 64 --memory 128m --cpus 1 \
  --log-driver local --log-opt max-size=10m --log-opt max-file=3 \
  --env-file /srv/our-clipboard/relay.env \
  --mount type=bind,src=/srv/our-clipboard/tls,dst=/tls,readonly \
  --publish 443:8443 \
  our-clipboard-relay
```

Docker forwards server port 443 to container port 8443; the application needs no
root privileges or network capabilities. The read-only directory mount lets a
certificate manager replace files atomically without mounting its full state.
No writable data volume is needed. Do not mount the Docker socket in the service.

Configure your server/provider ingress for TCP 443 and the hostname's DNS through
your normal operations process. The endpoint is
`wss://YOUR_RELAY_HOST/our-clipboard/v1`. This recipe serves TLS directly; if an
existing ingress occupies 443, configure a WebSocket-capable route to the relay
instead of starting a competing listener. Preserve the path and Authorization
header. Clients always verify the relay hostname and certificate; private CAs
require an explicit `--relay-ca` file, never a TLS bypass.

Check process state and content-free service diagnostics:

```sh
sudo docker ps --filter name=our-clipboard-relay
sudo docker logs --tail 20 our-clipboard-relay
```

A running process alone does not prove DNS, ingress, WSS or peer connectivity.
From either workstation (with the transport credential in its environment), run:

```sh
./target/debug/our-clipboard relay-check --relay wss://YOUR_RELAY_HOST/our-clipboard/v1
```

This command needs no graphical session and works in a headless build. It checks
outgoing TCP, validated TLS/hostname, and the credential-authenticated WSS upgrade,
then closes without joining a room, generating offers or accessing a clipboard.
For a private CA add `--relay-ca /path/to/public-ca.pem`. It cannot prove peer
identity, available room capacity or clipboard delivery. No token/URL is echoed.
DNS resolution uses the OS resolver; socket setup has bounded deadlines, but a
stalled OS resolver is outside those deadlines.

Complete the two-client exchange in [OUTBOUND-PILOT.md](OUTBOUND-PILOT.md) before
calling deployment verified. No HTTP health endpoint or management listener is
provided. Do not use `docker inspect` output as a diagnostic attachment: it includes
the injected environment.

## Renew, restart and roll back

The relay reads TLS files once at startup. After renewing the certificate/key,
preserve ownership and private permissions, then restart the container to load
them. Restart ends active sessions. Start fresh pilots and exchange fresh offers;
there is no automatic reconnect or clipboard replay.

Updating the image or access credential requires recreating the container with
the same run options. Coordinate a credential change with both clients. Keep the
previous image ID for rollback; neither image should contain deployment secrets.
Do not delete your certificate-manager state during container replacement.

This remains a supervised pilot, not a hardened public hosting service. The relay
bounds sockets (32), waiting rooms (16), records, setup waits and paired lifetime
(one hour). Per-user fair quotas, live credential rotation, reconnect and automatic
OS-lock integration are still pending. Never lock either desktop while its pilot
is active.

## Container integration check

CI builds this exact image and starts it with UID 65532, a read-only root,
capabilities dropped, and the limits above. An isolated test container shares only
its network namespace to drive two native Xvfb clients through real WSS and pinned
inner TLS. It checks both directions, signed ACKs, startup isolation and no echo;
`strace` checks neither client calls `listen()`. No test publishes a host port.

To repeat after building the test images and native helper:

```sh
cargo build --locked --bin our-clipboard --example synthetic-clipboard-sync
docker build -t our-clipboard-tests -f tests/linux/Dockerfile .
docker build -t our-clipboard-relay-tests -f tests/linux/Dockerfile.relay .
python3 tests/linux/run-relay-container.py
```

The script generates test-only credentials in a temporary Docker volume and
removes its containers and volume on completion. It does not use workstation
clipboard sessions, production certificates or an external relay.

# Stage relay deployment

Endpoint: `wss://clipboard.stage.daropotter.com/our-clipboard/v1`.

This deployment is on the existing SSH host alias `stage`, not on a workstation
or production host. The relay image was built from PR #31 revision `fe7d0e7` and
passed its 16 CI checks before deployment. Its immutable local image ID is:

```text
sha256:bf079942676abe85db0c41e206b5fa209809629f29762538c93173a9b09d9298
```

There is no image registry publication or automatic deployment. The tested image
was transferred to stage with `docker save` over authenticated SSH and loaded
locally. `/etc/our-clipboard/image.env` pins `RELAY_IMAGE` to this image ID.
Clients with `relay-check` currently need PR #32 or a later merged revision.

## Topology and installed files

The existing `eyrie-stage-proxy` Caddy owns public 443 and obtains/renews the
public hostname certificate through its existing ACME configuration. Its existing
`/etc/caddy/previews/*.caddy` import includes the new route. No shared Caddyfile,
production service, DNS record or firewall rule was changed.

Caddy enforces TLS 1.3 for this hostname, matches only `/our-clipboard/v1`, and
forwards WebSocket upgrades with their Authorization header over verified TLS to
`our-clipboard-stage-relay:8443`. The self-signed upstream certificate is pinned
in Caddy's trust pool with the exact upstream DNS name; no TLS verification is
disabled. The independent inner peer mutual TLS remains opaque to both proxies.

The relay joins `stage-preview-proxy_default` and publishes **no host ports**.
Compose runs it as UID/GID 65532, with a read-only root/TLS mount, no capabilities,
no-new-privileges, 64 PIDs, 128 MiB RAM and one CPU. Bounded local Docker logs contain
no clipboard data or access credential. Docker administrators can inspect the
environment and must be trusted.

Installed locations:

- `/opt/our-clipboard-stage/compose.yaml`: this Compose definition.
- `/etc/systemd/system/our-clipboard-stage.service`: starts after the existing
  stage ingress service; enabled at boot. Docker restart policy handles crashes.
- `/etc/our-clipboard/image.env`: nonsecret immutable image selection.
- `/etc/our-clipboard/relay.env`: **secret**, root-owned 0600, initial stage-only
  transport credential. This bootstrap credential has not been imported into
  OpenBao; it is not managed by the existing read-only stage AppRole.
- `/etc/our-clipboard/tls/privkey.pem`: **secret**, UID/GID 65532, 0600.
- `/etc/our-clipboard/tls/fullchain.pem`: public upstream certificate, valid until
  2027-10-25. Renew it before expiry; see below.
- `/var/lib/flock-stage-previews/caddy/our-clipboard.caddy`: the new route.
- `/var/lib/flock-stage-previews/caddy/our-clipboard-ca.pem`: only the public
  upstream certificate, readable by Caddy. No private key is mounted in Caddy.

Never include environment files, Docker inspect output, private keys or tokens in
PRs, diagnostic attachments, chat or documentation. No secrets are in this folder.

## Client access

Provision the transport credential from the stage root-only environment file
through an authorized secret channel. It does not confer peer/group membership.
An operator with outgoing SSH access to stage can download the environment file
without printing its contents; keep the destination directory 0700 and file 0600.
No workstation SSH listener is needed. Do not paste its value into commands.

After loading that environment into the local shell:

```sh
./target/debug/our-clipboard relay-check \
  --relay wss://clipboard.stage.daropotter.com/our-clipboard/v1
```

Clients use the public CA roots; **no `--relay-ca` override is needed**. A passing
check certifies only outer WSS/TLS/access. Continue with fresh public offers and
independent fingerprint confirmation in [OUTBOUND-PILOT.md](../../docs/OUTBOUND-PILOT.md).
Use this endpoint for both sides. Stop pilots before locking either desktop.

## Operator checks

Run these on stage without dumping the environment:

```sh
sudo systemctl status our-clipboard-stage.service
sudo docker ps --filter name=our-clipboard-stage-relay
sudo docker logs --tail 20 our-clipboard-stage-relay
sudo docker exec eyrie-stage-proxy caddy validate \
  --config /etc/caddy/Caddyfile --adapter caddyfile
```

The service is `Type=oneshot`; `active (exited)` records successful Compose startup,
not ongoing readiness. Use container state plus external `relay-check` to verify
operation. Never use a full `docker inspect` dump for support.

For a replacement image, transfer the tested image first, update only
`/etc/our-clipboard/image.env`, then run `sudo systemctl restart
our-clipboard-stage.service`. Compose recreates the relay. Keep the previous image
ID for rollback. Any restart ends active pilots; start fresh processes/offers.

The public certificate renews through Caddy automatically. The separate upstream
certificate is static. Before its expiry, generate a new Ed25519 server key and
self-signed certificate with SAN `our-clipboard-stage-relay`, serverAuth EKU and
CA:FALSE, in an operator-only staging directory. Stop relay pilots, replace the
upstream pair preserving private permissions/ownership, replace the public Caddy
trust certificate, recreate/restart the relay, validate and reload Caddy, and run
external preflight again. Do not copy private key material into the ingress tree.

To withdraw this deployment, move only `our-clipboard.caddy` out of the wildcard
import directory to an operator backup location, validate and reload Caddy, then
stop/disable `our-clipboard-stage.service`. Preserve secrets and old image for
rollback; do not restart or stop the shared stage ingress.

## Deployment validation (2026-10-25)

- Public DNS already resolved to stage; no DNS write was required.
- Public HTTPS chain verified; root returned 404, unauthenticated WSS returned401.
- Real `relay-check` with the separately provisioned token passed; an invalid
  transport token was rejected.
- Two disposable Xvfb clients connected outbound through the public hostname and
  existing Caddy, independently confirmed peer fingerprints, and exchanged native
  clipboard text in both directions with receiver-signed ACKs, startup isolation
  and no echo. strace found no client listen syscall. No host clipboard was used.
- Existing todoapp, usage and legacy Paperclip routes still returned their
  expected 200,401 and307 statuses after the reload.
- Image identity, non-root user, read-only root, empty host port bindings,
  capability drop, memory limit and enabled boot service were checked live.

This validates the deployed path with two controlled Linux clients. A physical
X11↔macOS pair is still a manual test; automatic lock handling, reconnect,
production enrollment and fair per-user relay quotas remain outside this pilot.

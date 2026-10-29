# Experimental native Wayland relay pilot

Linux clients may explicitly opt into a supervised native **data-control** WSS
pilot. The default X11/macOS path remains unchanged. This is not general Wayland
or GNOME pilot support; physical Wayland↔macOS testing remains separate.

Use the same revision on both computers and follow the
[outbound pilot](OUTBOUND-PILOT.md) for a manually chosen relay, credential
injection, fresh public-offer exchange and independent fingerprint confirmation.
Alternatively use an initialized identity and [explicit saved peer approval](PEER-TRUST.md).
Neither workstation needs SSH or a listening socket; the existing relay needs no change.

On the native Wayland endpoint, append `--experimental-wayland` to the direct
relay invocation. In a launch file, set `"experimental_wayland": true`; the
[X11/macOS launch-file guide](PILOT-CONFIG.md) and
[Wayland example](examples/pilot-wayland.json) otherwise use the same format.
The flag is local: an X11/macOS peer uses its usual invocation without it.

```sh
our-clipboard pilot config-check --file /path/to/run/wayland.json
our-clipboard pilot preflight --file /path/to/run/wayland.json
our-clipboard pilot --config /path/to/run/wayland.json --replace-clipboard
```

Run from the actual logged-in Wayland user's environment, with its legitimate
`XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` (or Wayland session declaration). Never
point at another user's runtime directory or enable XWayland to bypass missing
access. The client requires clipboard read/write data-control **and** native
selection-change observation. At session initialization it uses data-control-only
constructors, including when rebaselining after Idle. Missing socket/protocol/seat
access fails rather than selecting an X11 or GNOME adapter, even if `DISPLAY`
points to a reachable X11 server. No clipboard contents are logged.

On Linux, Wayland session selection without the explicit option is rejected before
credential access or offer publication. The option is relay-only, requires a
native Wayland context, and is rejected on other operating systems. Static
`config-check` validates the schema/options only. Preflight additionally checks
session-environment selection, but still does not initialize a native backend or
certify protocol/permission availability. An existing Pause marker retains its
priority over environment/backend/credential access.

Do not combine this mode with `--logind-idle` / `logind_idle`: that experimental
observer's caller/display binding is currently X11-only. Use the explicit local
Idle/Pause markers for supervised testing. Neither the Wayland flag nor the
markers certify native screen-lock/suspend state. The GNOME Shell extension and
GNOME clipboard adapter tests elsewhere in the repository do not imply GNOME
pilot support; this mode never falls back to that adapter.

Wait for both READY statuses (or the local `ready` records on both
[status streams](PILOT-STATUS.md)) before copying new text. Existing startup
isolation/quiet period, local origin markers, signed events/ACKs, original TTL,
Idle buffering, Pause and authenticated reconnect rules apply unchanged. The
[Pause command](PILOT-CONFIG.md#local-pause-requests-and-marker-inspection) works
without connecting to the compositor or the relay. Restart requires fresh offers
and a new quiet period; a local control request is not process acknowledgement.

## Validation scope

CI runs Sway 1.9 / wlroots 0.17 from Ubuntu 24.04 in a disposable headless
compositor, against an independent Xvfb clipboard through a private WSS relay.
It exercises both directions, multiline Unicode, same-text new copies, startup
isolation, origin-marker echo suppression, Idle/Pause, signed application ACKs,
original-TTL expiry and reconnect. A second pair of processes reloads OS-store
identity and saved peer approval from a private Secret Service using launch files.
A reachable X11 server remains in the Wayland client's environment; syscall traces
assert that client never connects to X11 or listens. Missing Wayland access is
also checked with reachable X11 to prove there is no fallback.

Native adapter tests on other compositors remain separate. KWin and GNOME pilot
integration, physical Wayland↔macOS runs, system lock/suspend certification and
background operation are not verified by this fixture. Other data-control
compositors may work but are not certified here. A compositor can deny or revoke
access; do not treat a successful static preflight as clipboard readiness.

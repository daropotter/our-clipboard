# GNOME Wayland adapter prototype

GNOME Shell 46 exposes clipboard operations to the client through a versioned
local interface on the logged-in user's session D-Bus. It opens no network port.
The ordinary native Wayland data-control backend remains the first choice; the
extension is used only when that backend is unavailable and the extension's
Wayland socket matches the requested session.

## Manual installation for development

Copy `our-clipboard@daropotter` into
`~/.local/share/gnome-shell/extensions/our-clipboard@daropotter`, log out and back
in, then enable **Our Clipboard Adapter** in Extensions or run:

```sh
gnome-extensions enable our-clipboard@daropotter
cargo run -- doctor --clipboard
```

Installation and enabling are explicit. The application does not install or enable
extensions automatically. Disable it with:

```sh
gnome-extensions disable our-clipboard@daropotter
```

Enabling the adapter allows applications with access to that user's session bus
to read/write the clipboard through this interface. Local origin markers prevent
loops; they are not authentication or group-membership proofs. Sandboxing,
permissions UI, packaging and a final local-access policy need review before a
production release.

## Behavior and coverage

- Text/size validation applies on both sides. Text and markers are not logged.
- GNOME's selection owner-change signal maintains a revision counter; clients
  sample only this metadata, not clipboard text.
- Applied text is associated with its native selection-source object. A normal
  copy with a new owner clears the marker even if its text is identical.
- A new extension generation invalidates existing client adapters. Recreate the
  client session after disabling/re-enabling or restarting the extension.
- D-Bus method calls have a two-second client timeout. Native St clipboard reads
  can still allocate before size validation; uniform backend limits remain work.

CI runs the actual extension in headless GNOME Shell 46/Mutter 46 on Ubuntu 24.04,
with a virtual monitor, software rendering, no XWayland, private buses and no host
clipboard/network access. The private "system" bus is a test stand-in; this is not
a full login session and does not verify lock/suspend, desktop integration or
other GNOME versions. Those require additional environments before release.

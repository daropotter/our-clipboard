#!/bin/bash
set -euo pipefail
ulimit -c 0
if [ "${OUR_CLIPBOARD_PRIVATE_BUS:-}" != 1 ]; then
    export OUR_CLIPBOARD_PRIVATE_BUS=1
    exec dbus-run-session -- bash "$0" "$@"
fi
export DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"
export XDG_RUNTIME_DIR="$(mktemp -d)"
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=GNOME
export LIBGL_ALWAYS_SOFTWARE=1
export MUTTER_DEBUG_DUMMY_MODE_SPECS=1280x720
unset DISPLAY WAYLAND_DISPLAY
mkdir -p "$HOME/.local/share/gnome-shell/extensions"
cp -r /extension "$HOME/.local/share/gnome-shell/extensions/our-clipboard@daropotter"
gsettings set org.gnome.shell enabled-extensions "['our-clipboard@daropotter']"
gnome-shell --wayland --no-x11 --headless --virtual-monitor 1280x720 \
    >"$XDG_RUNTIME_DIR/gnome.log" 2>&1 &
compositor_pid=$!
trap 'kill "$compositor_pid" 2>/dev/null || true' EXIT
for attempt in {1..150}; do
    if timeout 1s gdbus call --session --dest org.gnome.Shell \
        --object-path /org/gnome/Shell/Extensions/OurClipboard \
        --method org.gnome.Shell.Extensions.OurClipboard1.Version >/dev/null 2>&1; then break; fi
    if ! kill -0 "$compositor_pid" 2>/dev/null; then break; fi
    sleep 0.1
done
if ! gdbus call --session --dest org.gnome.Shell \
    --object-path /org/gnome/Shell/Extensions/OurClipboard \
    --method org.gnome.Shell.Extensions.OurClipboard1.Version; then
    gsettings get org.gnome.shell enabled-extensions
    gsettings get org.gnome.shell disable-user-extensions
    gnome-extensions info our-clipboard@daropotter || true
    cat "$XDG_RUNTIME_DIR/gnome.log"
    exit 1
fi
socket=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'wayland-*' -type s -print -quit)
test -n "$socket"
export WAYLAND_DISPLAY="$(basename "$socket")"
test "$#" -eq 1
"$1" --ignored --test-threads=1

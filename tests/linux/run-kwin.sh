#!/bin/bash
set -euo pipefail
binary=$1
export XDG_RUNTIME_DIR="$(mktemp -d)"
export XDG_SESSION_TYPE=wayland
export LANG=C.UTF-8
export QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software
export LIBGL_ALWAYS_SOFTWARE=1 KWIN_COMPOSE=Q
unset DISPLAY WAYLAND_DISPLAY
kwin_wayland --version
# A separate bus and virtual output avoid the host desktop and physical GPU.
dbus-run-session -- kwin_wayland --virtual --no-lockscreen \
    --no-global-shortcuts --socket clipboard-test \
    >"$XDG_RUNTIME_DIR/kwin.log" 2>&1 &
compositor_pid=$!
trap 'kill "$compositor_pid" 2>/dev/null || true' EXIT
for attempt in {1..100}; do
    if [ -S "$XDG_RUNTIME_DIR/clipboard-test" ]; then break; fi
    if ! kill -0 "$compositor_pid" 2>/dev/null; then break; fi
    sleep 0.1
done
if [ ! -S "$XDG_RUNTIME_DIR/clipboard-test" ]; then
    cat "$XDG_RUNTIME_DIR/kwin.log"
    exit 1
fi
export WAYLAND_DISPLAY=clipboard-test
"$binary" --ignored --test-threads=1

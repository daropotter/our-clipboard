#!/bin/bash
set -euo pipefail
binary=$1
env -u WAYLAND_DISPLAY -u XDG_SESSION_TYPE xvfb-run -a "$binary" --ignored --test-threads=1
export XDG_RUNTIME_DIR="$(mktemp -d)"
export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
unset DISPLAY
export XDG_SESSION_TYPE=wayland
sway --unsupported-gpu --config /dev/null >"$XDG_RUNTIME_DIR/sway.log" 2>&1 &
compositor_pid=$!
trap 'kill "$compositor_pid" 2>/dev/null || true' EXIT
socket=''
for attempt in {1..50}; do
    socket=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'wayland-*' -type s -print -quit)
    if [ -n "$socket" ]; then break; fi
    sleep 0.1
done
if [ -z "$socket" ]; then cat "$XDG_RUNTIME_DIR/sway.log"; exit 1; fi
export WAYLAND_DISPLAY="$(basename "$socket")"
"$binary" --ignored --test-threads=1

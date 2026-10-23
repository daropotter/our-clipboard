#!/bin/bash
set -euo pipefail
binary=$1
unset WAYLAND_DISPLAY WAYLAND_SOCKET
export XDG_SESSION_TYPE=x11
Xvfb :71 -screen 0 1280x720x24 -nolisten tcp >/tmp/source-x11.log 2>&1 &
source_pid=$!
Xvfb :72 -screen 0 1280x720x24 -nolisten tcp >/tmp/target-x11.log 2>&1 &
target_pid=$!
trap 'kill "$source_pid" "$target_pid" 2>/dev/null || true' EXIT
for display in :71 :72; do
    ready=0
    for attempt in {1..50}; do
        if DISPLAY="$display" "$binary" --replace-clipboard check-display >/dev/null 2>&1; then ready=1; break; fi
        sleep 0.1
    done
    if [ "$ready" -ne 1 ]; then cat /tmp/source-x11.log /tmp/target-x11.log; exit 1; fi
done
export DISPLAY=:71 OUR_CLIPBOARD_TEST_TARGET_DISPLAY=:72
"$binary" --replace-clipboard controller

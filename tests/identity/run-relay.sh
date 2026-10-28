#!/bin/bash
set -euo pipefail
if [ "${OUR_CLIPBOARD_PRIVATE_KEYRING_BUS:-}" != 1 ]; then
    export OUR_CLIPBOARD_PRIVATE_KEYRING_BUS=1
    export XDG_RUNTIME_DIR="$(mktemp -d)"
    export OUR_CLIPBOARD_CONFIG_DIR="$(mktemp -d)"
    exec dbus-run-session -- bash "$0" "$@"
fi
# Only the test container's private Secret Service collection is unlocked.
python3 - <<'PY'
import secrets
import subprocess
result = subprocess.run(
    ['gnome-keyring-daemon', '--unlock', '--components=secrets'],
    input=secrets.token_urlsafe(32).encode() + b'\n',
    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10,
)
if result.returncode:
    raise SystemExit('Could not initialize the disposable credential service')
PY
export OUR_CLIPBOARD_TEST_SIGNED_OFFERS=1
OUR_CLIPBOARD_TEST_ENROLL_PEERS=1 python3 /run-relay-pilot.py "$1" "$2"
# A second pair of processes reloads the same profiles, uses fresh public offers,
# then survives actual relay outages in the same live session.
OUR_CLIPBOARD_TEST_NAMED_PEERS=1 OUR_CLIPBOARD_TEST_RECONNECT=1 python3 /run-relay-pilot.py "$1" "$2"

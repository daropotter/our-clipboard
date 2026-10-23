#!/bin/bash
set -euo pipefail
if [ "${OUR_CLIPBOARD_PRIVATE_KEYRING_BUS:-}" != 1 ]; then
    export OUR_CLIPBOARD_PRIVATE_KEYRING_BUS=1
    export XDG_RUNTIME_DIR="$(mktemp -d)"
    exec dbus-run-session -- bash "$0" "$@"
fi
# Only a disposable encrypted test collection is unlocked. No password goes to logs.
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
"$1" --ignored --test-threads=1

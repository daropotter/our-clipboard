"""Read-only metadata probe for the disposable credential fixture, never secrets."""
import json
import os
import re
import subprocess

BUS = "org.freedesktop.DBus"
BUS_PATH = "/org/freedesktop/DBus"
SERVICE = "org.freedesktop.secrets"
ROOT = "/org/freedesktop/secrets"


def _call(destination, path, method, *args):
    try:
        result = subprocess.run(
            ["gdbus", "call", "--session", "--dest", destination, "--object-path", path,
             "--method", method, "--timeout", "2", *args],
            capture_output=True, text=True, timeout=3,
        )
        return result.stdout if result.returncode == 0 and len(result.stdout) <= 4096 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def _match(pattern, text):
    return re.fullmatch(pattern, text.strip()) if isinstance(text, str) else None


def snapshot(call=_call):
    address = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    private = (os.environ.get("OUR_CLIPBOARD_PRIVATE_KEYRING_BUS") == "1" and bool(address)
               and all(part.startswith("unix:") for part in address.split(";")))
    result = {"private_fixture": private, "service": "unknown" if private else "not_checked",
              "collection": "not_checked", "owner_changed": None}
    if not private:
        return result
    try:
        present = _match(r"\((true|false),\)", call(BUS, BUS_PATH, BUS + ".NameHasOwner", SERVICE))
        if present is None:
            return result
        if present[1] == "false":
            result["service"] = "absent"
            return result
        result["service"] = "present"
        owner = _match(r"\('(:[0-9]+\.[0-9]+)',\)", call(BUS, BUS_PATH, BUS + ".GetNameOwner", SERVICE))
        if owner is None:
            result["service"] = "unknown"
            return result
        # Unique destinations cannot activate a replacement well-known service.
        owner = owner[1]
        result["collection"] = "unknown"
        alias = _match(r"\(objectpath '(/[A-Za-z0-9_/]*)',\)",
                       call(owner, ROOT, "org.freedesktop.Secret.Service.ReadAlias", "default"))
        if alias is not None:
            if alias[1] == "/":
                result["collection"] = "missing"
            elif alias[1].startswith(ROOT + "/collection/"):
                locked = _match(r"\(<(true|false)>,\)",
                                call(owner, alias[1], "org.freedesktop.DBus.Properties.Get",
                                     "org.freedesktop.Secret.Collection", "Locked"))
                if locked is not None:
                    result["collection"] = "locked" if locked[1] == "true" else "unlocked"
        current = _match(r"\('(:[0-9]+\.[0-9]+)',\)", call(BUS, BUS_PATH, BUS + ".GetNameOwner", SERVICE))
        if current is not None:
            result["owner_changed"] = current[1] != owner
    except Exception:
        # Diagnostics must not mask the original test failure or print backend data.
        pass
    return result


if __name__ == "__main__":
    print(json.dumps(snapshot(), sort_keys=True))

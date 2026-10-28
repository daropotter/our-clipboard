"""Local preflight assertions on the fixture's private credential bus only."""
import json
from pathlib import Path
import subprocess

def check(root, binary, env, settings, side, fingerprints):
    config = Path(env["OUR_CLIPBOARD_CONFIG_DIR"])
    def snapshot():
        return {str(p.relative_to(config)): p.read_bytes() for p in config.rglob("*") if p.is_file()}
    before = snapshot()
    number = 0
    def call(data, expected, *, override_env=None, success=False):
        nonlocal number
        number += 1
        path = root / f"preflight-{side}-{number}.json"
        path.write_text(json.dumps(data))
        path.chmod(0o600)
        trace = root / f"preflight-{side}-{number}.trace"
        result = subprocess.run(["strace", "-f", "-qq", "-e", "trace=listen,connect", "-o", str(trace),
                                 binary, "pilot", "preflight", "--file", str(path)],
                                env=override_env or env, capture_output=True, timeout=10)
        assert result.returncode == (0 if success else 2), "Unexpected local preflight result"
        assert expected.encode() in result.stdout + result.stderr, "Missing preflight diagnostic"
        lines = trace.read_text().splitlines()
        assert all("AF_UNIX" in line and "X11-unix" not in line for line in lines if "connect(" in line), "Preflight contacted transport/clipboard"
        assert not any("listen(" in line for line in lines), "Preflight opened listener"
        assert not (root / settings["offer"]).exists(), "Preflight published an offer"
        assert not (root / settings["peer_offer"]).exists(), "Preflight created a received offer"
        return result
    ok = call(settings, "LOCAL PREFLIGHT PASSED", success=True)
    for fingerprint in fingerprints.values():
        assert fingerprint.encode() in ok.stdout, "Wrong local/approved identity reported"
    missing = dict(settings, peer_name="absent")
    call(missing, "Peer trust is missing")
    missing = dict(settings, identity_profile="absent-preflight-profile")
    call(missing, "Identity is missing")
    assert not (config / "absent-preflight-profile").exists(), "Preflight created missing identity metadata"
    bad_token = "SECRET-DO-NOT-ECHO!"
    bad = call(settings, "Relay credential missing or invalid", override_env=dict(env, OUR_CLIPBOARD_RELAY_TOKEN=bad_token))
    assert bad_token.encode() not in bad.stdout + bad.stderr
    bad_ca = root / f"bad-{side}.pem"
    bad_ca.write_text("not a certificate")
    call(dict(settings, relay_ca=bad_ca.name), "Relay CA configuration invalid")
    pin = config / f"pilot-{side}/peers/remote.pin"
    original = pin.read_bytes()
    damaged = bytearray(original)
    damaged[-1] ^= 1
    pin.write_bytes(damaged)
    try:
        call(settings, "Peer trust record is invalid")
        assert pin.read_bytes() == damaged, "Preflight repaired peer approval"
    finally:
        pin.write_bytes(original)
    assert snapshot() == before, "Preflight changed local identity/trust metadata"

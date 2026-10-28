"""Explicit enroll/reject cases for the private keyring/WSS fixture only."""
import os
from pathlib import Path
import subprocess

def enroll(root, binary, env, room, fingerprints):
    assert os.environ.get("OUR_CLIPBOARD_PRIVATE_KEYRING_BUS") == "1"
    for side, peer in (("a", "b"), ("b", "a")):
        result = subprocess.run([binary, "peer", "trust", "--profile", "pilot-" + side,
                                 "--name", "remote", "--offer", str(root / f"{peer}.offer"),
                                 "--room", room, "--side", peer], env=env,
                                input=(fingerprints[peer] + "\n").encode(), capture_output=True, timeout=10)
        assert result.returncode == 0, "Explicit offline peer approval failed"
        shown = subprocess.run([binary, "peer", "show", "--profile", "pilot-" + side,
                                "--name", "remote"], env=env, capture_output=True, timeout=10)
        assert shown.returncode == 0 and fingerprints[peer].encode() in shown.stdout, "Peer approval reload failed"

def reject_changed_or_unusable_trust(root, binary, env, room, bad_args, spawn, status, wait_for, offer_ready):
    assert os.environ.get("OUR_CLIPBOARD_PRIVATE_KEYRING_BUS") == "1"
    config = Path(env["OUR_CLIPBOARD_CONFIG_DIR"])
    pin = config / "pilot-a/peers/remote.pin"
    original = pin.read_bytes()
    def reject(case, diagnostic, *, name="remote", peer_offer=None, profile="pilot-a", before_offer=True):
        args = bad_args.copy()
        own = root / f"{case}.offer"
        trace = root / f"{case}.trace"
        log = root / f"{case}.log"
        for option, value in (("--offer", own), ("-o", trace), ("--identity-profile", profile)):
            args[args.index(option) + 1] = str(value)
        if peer_offer:
            args[args.index("--peer-offer") + 1] = str(peer_offer)
        args.extend(["--peer-name", name])
        process = spawn(args, env=dict(env, DISPLAY=":91"), stdin=subprocess.DEVNULL, log=log)
        assert process.wait(timeout=5) == 2, "Unusable named peer was accepted"
        assert diagnostic in status(log), "Expected precise peer trust rejection"
        if before_offer:
            assert not own.exists(), "Unusable trust created an offer"
        lines = trace.read_text().splitlines()
        assert all("AF_UNIX" in line and "X11-unix" not in line for line in lines if "connect(" in line), "Trust failure reached transport or clipboard"
        assert not any("listen(" in line for line in lines), "Trust failure created listener"
    reject("missing-pin", "Peer trust is missing", name="absent")
    damaged = bytearray(original)
    damaged[-1] ^= 1
    pin.write_bytes(damaged)
    try:
        reject("corrupt-pin", "Peer trust record is invalid")
        assert pin.read_bytes() == damaged, "Corrupt trust was repaired"
    finally:
        pin.write_bytes(original)
    initialized = subprocess.run([binary, "identity", "init", "--profile", "pilot-other"],
                                 env=env, capture_output=True, timeout=10)
    assert initialized.returncode == 0, "Disposable alternate identity failed"
    other_args = bad_args.copy()
    other_offer = root / "other-device.offer"
    for option, value in (("--offer", other_offer), ("--peer-offer", root / "never-arrives.offer"),
                          ("--identity-profile", "pilot-other"), ("--side", "b"), ("-o", root / "other.trace")):
        other_args[other_args.index(option) + 1] = str(value)
    other = spawn(other_args, env=dict(env, DISPLAY=":91"), stdin=subprocess.DEVNULL, log=root / "other.log")
    try:
        wait_for(lambda: offer_ready(other_offer))
        # The signature/room/side are valid; only the approved stable key differs.
        reject("changed-key", "Saved peer key differs", peer_offer=other_offer, before_offer=False)
        assert pin.read_bytes() == original, "Changed peer replaced approval"
        foreign = config / "pilot-other/peers"
        foreign.mkdir(mode=0o700)
        (foreign / "remote.pin").write_bytes(original)
        reject("foreign-owner", "does not belong to the current local device", profile="pilot-other")
    finally:
        # Complete the waiting file input with a deliberately invalid offer.
        # Signalling strace alone need not terminate its traced child.
        (root / "never-arrives.offer").write_bytes(b"invalid")
        assert other.wait(timeout=3) == 2, "Alternate-offer helper did not stop before transport"
    assert pin.read_bytes() == original

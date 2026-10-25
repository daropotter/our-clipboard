#!/usr/bin/env python3
"""Real WSS + inner mutual TLS + native clipboards; no host ports/credentials/SSH."""
import hashlib
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time

binary, helper = sys.argv[1:3]
external = os.environ.get("OUR_CLIPBOARD_EXTERNAL_RELAY") == "1"
relay_port = 8443 if external else 443
children = []
handles = []

def spawn(args, *, env=None, log=None, stdin=None):
    out = subprocess.DEVNULL
    if log:
        out = open(log, "wb")
        handles.append(out)
    child = subprocess.Popen(args, env=env, stdin=stdin, stdout=subprocess.DEVNULL, stderr=out)
    children.append(child)
    return child

def wait_for(check, seconds=10):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError("Timed out; clipboard content and credentials omitted")

def inspect(display, mode):
    env = dict(os.environ, DISPLAY=display, XDG_SESSION_TYPE="x11")
    return subprocess.run([helper, "--replace-clipboard", mode], env=env,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0

def seed(display, mode):
    return spawn([helper, "--replace-clipboard", mode],
                 env=dict(os.environ, DISPLAY=display, XDG_SESSION_TYPE="x11"))

def status(path):
    return path.read_text() if path.exists() else ""

def offer_ready(path):
    if not path.exists():
        return False
    b = path.read_bytes()
    return len(b) >= 10 and len(b) == 9 + int.from_bytes(b[5:9], "big")

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    try:
        for display in (":91", ":92"):
            spawn(["Xvfb", display, "-screen", "0", "1280x720x24", "-nolisten", "tcp"])
            wait_for(lambda: inspect(display, "check-display"))
        cert, key = root / "relay.pem", root / "relay.key"
        if external:
            cert = Path("/tls/fullchain.pem")
        else:
            subprocess.run(["openssl", "req", "-x509", "-newkey", "ed25519", "-nodes", "-days", "1",
                            "-keyout", str(key), "-out", str(cert), "-subj", "/CN=localhost",
                            "-addext", "subjectAltName=DNS:localhost", "-addext", "basicConstraints=critical,CA:FALSE",
                            "-addext", "keyUsage=critical,digitalSignature", "-addext", "extendedKeyUsage=serverAuth"], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            key.chmod(0o600)
        token = os.environ["OUR_CLIPBOARD_RELAY_TOKEN"] if external else secrets.token_hex(32)
        env = dict(os.environ, OUR_CLIPBOARD_RELAY_TOKEN=token, XDG_SESSION_TYPE="x11")
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("WAYLAND_SOCKET", None)
        server = None
        if not external:
            server = spawn([binary, "relay", "--listen", "127.0.0.1:443", "--tls-cert", str(cert),
                            "--tls-key", str(key)], env=env, log=root / "relay.log")
        def listening():
            if server is not None and server.poll() is not None:
                raise AssertionError("Relay exited")
            try:
                with socket.create_connection(("127.0.0.1", relay_port), timeout=0.1):
                    return True
            except OSError:
                return False
        wait_for(listening)
        seed(":91", "seed-request")
        seed(":92", "seed-response")
        wait_for(lambda: inspect(":91", "inspect-request") and inspect(":92", "inspect-response"))
        room = secrets.token_hex(32)  # Public routing ID, not a credential.
        clients = {}
        for side, display, peer in (("a", ":91", "b"), ("b", ":92", "a")):
            args = ["strace", "-f", "-qq", "-e", "trace=listen", "-o", str(root / f"{side}.trace"),
                    binary, "pilot", "--replace-clipboard", "--relay", f"wss://localhost:{relay_port}/our-clipboard/v1",
                    "--relay-ca", str(cert), "--room", room, "--side", side,
                    "--offer", str(root / f"{side}.offer"), "--peer-offer", str(root / f"{peer}-for-{side}.offer"),
                    "--seconds", "12", "--ttl-ms", "1000", "--max-drift-ppm", "1000",
                    "--clock-granularity-ms", "1"]
            clients[side] = spawn(args, env=dict(env, DISPLAY=display),
                                  stdin=subprocess.PIPE, log=root / f"{side}.log")
        wait_for(lambda: offer_ready(root / "a.offer") and offer_ready(root / "b.offer"))
        for side, peer in (("a", "b"), ("b", "a")):
            data = (root / f"{peer}.offer").read_bytes()
            temp = root / f"{peer}-for-{side}.tmp"
            shutil.copyfile(root / f"{peer}.offer", temp)
            temp.replace(root / f"{peer}-for-{side}.offer")
            fingerprint = "sha256:" + hashlib.sha256(data).hexdigest()
            clients[side].stdin.write((fingerprint + "\n").encode())
            clients[side].stdin.flush()
            clients[side].stdin.close()
        wait_for(lambda: all("READY:" in status(root / f"{s}.log") for s in ("a", "b")))
        assert inspect(":91", "inspect-request") and inspect(":92", "inspect-response"), "Startup contents propagated"
        seed(":91", "seed-request")  # Same text, new local copy.
        wait_for(lambda: inspect(":92", "inspect-request"), 5)
        wait_for(lambda: "Peer confirmed application." in status(root / "a.log"), 5)
        seed(":92", "seed-response")
        wait_for(lambda: inspect(":91", "inspect-response"), 5)
        wait_for(lambda: "Peer confirmed application." in status(root / "b.log"), 5)
        for side, process in clients.items():
            assert process.wait(timeout=20) == 0, "Pilot exited unsuccessfully"
            assert "listen(" not in (root / f"{side}.trace").read_text(), "Endpoint tried to listen"
        bad = spawn(["strace", "-f", "-qq", "-e", "trace=listen,connect", "-o", str(root / "bad.trace"),
                     binary, "pilot", "--replace-clipboard", "--relay", f"wss://localhost:{relay_port}/our-clipboard/v1",
                     "--relay-ca", str(cert), "--room", room, "--side", "a", "--offer", str(root / "bad.offer"),
                     "--peer-offer", str(root / "b.offer"), "--seconds", "12", "--ttl-ms", "1000",
                     "--max-drift-ppm", "1000", "--clock-granularity-ms", "1"],
                    env=dict(env, DISPLAY=":91"), stdin=subprocess.PIPE, log=root / "bad.log")
        bad.stdin.write(b"yes\n")
        bad.stdin.close()
        assert bad.wait(timeout=5) == 2, "Unverified fingerprint was accepted"
        assert "Peer fingerprint confirmation failed" in status(root / "bad.log"), "Expected trust rejection diagnostic"
        assert "connect(" not in (root / "bad.trace").read_text(), "Connected before fingerprint confirmation"
        assert "listen(" not in (root / "bad.trace").read_text(), "Rejected endpoint tried to listen"
        logs = status(root / "a.log") + status(root / "b.log")
        assert logs.count("Applied peer event.") == 2, "Unexpected repeated application/echo"
        print("Outbound WSS pilot passed: real HTTPS, pinned peer TLS, independent confirmations, startup isolation, bidirectional writes/ACKs and no client listen syscall.")
    except Exception:
        # Application statuses contain only public fingerprints and static operation names.
        for name in ("a.log", "b.log", "relay.log"):
            if (root / name).exists():
                print(status(root / name), file=sys.stderr)
        raise
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        for handle in handles:
            handle.close()

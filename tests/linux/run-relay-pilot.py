#!/usr/bin/env python3
"""Real WSS + inner mutual TLS + native clipboards; no host ports/credentials/SSH."""
import hashlib
import json
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
logind_fixture = sys.argv[3] if len(sys.argv) > 3 else None
reconnect_mode = os.environ.get("OUR_CLIPBOARD_TEST_RECONNECT", "")
launch_config = os.environ.get("OUR_CLIPBOARD_TEST_LAUNCH_CONFIG") == "1"
named_peers = os.environ.get("OUR_CLIPBOARD_TEST_NAMED_PEERS") == "1"
enroll_peers = os.environ.get("OUR_CLIPBOARD_TEST_ENROLL_PEERS") == "1"
signed_offers = os.environ.get("OUR_CLIPBOARD_TEST_SIGNED_OFFERS") == "1"
external = os.environ.get("OUR_CLIPBOARD_EXTERNAL_RELAY") == "1"
if launch_config and not named_peers:
    raise SystemExit("Launch fixture requires explicitly approved named peers")
if (named_peers or enroll_peers) and not signed_offers:
    raise SystemExit("Named peer fixture requires signed offers")
if signed_offers and external:
    raise SystemExit("Signed-offer fixtures require a private disposable keyring and relay")
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
    if signed_offers:
        return len(b) >= 164 and b[:5] == b"OCDO\x01" and len(b) == 154 + int.from_bytes(b[86:90], "big")
    return len(b) >= 10 and len(b) == 9 + int.from_bytes(b[5:9], "big")

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    try:
        display_workers = {}
        for display in (":91", ":92"):
            display_workers[display] = spawn(["Xvfb", display, "-screen", "0", "1280x720x24", "-nolisten", "tcp"])
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
        def stop_relay():
            assert server is not None and not external, "Fault injection must stay private"
            server.terminate()
            server.wait(timeout=3)
        def restart_relay(wrong_token=False):
            global server
            assert not external, "Fault injection must stay private"
            server_env = dict(env)
            if wrong_token:
                server_env["OUR_CLIPBOARD_RELAY_TOKEN"] = secrets.token_hex(32)
            server = spawn([binary, "relay", "--listen", "127.0.0.1:443", "--tls-cert", str(cert),
                            "--tls-key", str(key)], env=server_env, log=root / "relay.log")
            wait_for(listening)
        check_env = dict(env)
        for variable in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XDG_SESSION_TYPE"):
            check_env.pop(variable, None)
        check_args = ["strace", "-f", "-qq", "-e", "trace=listen", "-o", str(root / "check.trace"),
                      binary, "relay-check", "--relay", f"wss://localhost:{relay_port}/our-clipboard/v1",
                      "--relay-ca", str(cert)]
        check = subprocess.run(check_args, env=check_env, capture_output=True, timeout=20)
        assert check.returncode == 0 and b"check passed" in check.stdout, "WSS preflight failed"
        assert "listen(" not in (root / "check.trace").read_text(), "Preflight tried to listen"
        rejected = subprocess.run(check_args, env=dict(check_env, OUR_CLIPBOARD_RELAY_TOKEN=secrets.token_hex(32)),
                                  capture_output=True, timeout=20)
        assert rejected.returncode != 0 and not rejected.stdout, "Preflight accepted wrong transport credential"
        seed(":91", "seed-request")
        seed(":92", "seed-response")
        wait_for(lambda: inspect(":91", "inspect-request") and inspect(":92", "inspect-response"))
        system_env = {}
        system_workers = {}
        def system_state(side, state):
            path = root / f"{side}.system"
            temp = root / f"{side}.system.tmp"
            temp.write_text(state)
            temp.replace(path)
        def start_system(side):
            ready = root / f"{side}.system-ready"
            if ready.exists():
                ready.unlink()
            system_workers[side] = spawn([logind_fixture, str(root / f"{side}.system"), str(ready)],
                                         env=system_env[side])
            wait_for(ready.exists)
        def enter_idle(side):
            before = status(root / f"{side}.log").count("IDLE:")
            if logind_fixture:
                system_state(side, "locked")
            else:
                (root / f"{side}.idle").touch()
            wait_for(lambda: status(root / f"{side}.log").count("IDLE:") > before)
        def leave_idle(side):
            before = status(root / f"{side}.log").count("ACTIVE:")
            if logind_fixture:
                system_state(side, "active")
            else:
                (root / f"{side}.idle").unlink()
            wait_for(lambda: status(root / f"{side}.log").count("ACTIVE:") > before)
        if logind_fixture:
            for side in ("a", "b"):
                bus_path = root / f"{side}.bus"
                address = "unix:path=" + str(bus_path)
                spawn(["dbus-daemon", "--session", "--nofork", "--address=" + address])
                wait_for(bus_path.exists)
                system_env[side] = dict(env, DBUS_SYSTEM_BUS_ADDRESS=address)
                system_state(side, "locked" if side == "a" else "active")
                (root / f"{side}.pid").write_text(str(display_workers[":91" if side == "a" else ":92"].pid))
                start_system(side)
        room = secrets.token_hex(32)  # Public routing ID, not a credential.
        clients = {}
        device_fingerprints = {}
        if signed_offers:
            assert os.environ.get("OUR_CLIPBOARD_PRIVATE_KEYRING_BUS") == "1", "Private keyring required"
            for side in ("a", "b"):
                result = subprocess.run([binary, "identity", "init", "--profile", "pilot-" + side],
                                        env=env, capture_output=True, timeout=10)
                assert result.returncode == 0, "Disposable identity initialization failed"
                device_fingerprints[side] = result.stdout.decode().splitlines()[0].split(": ", 1)[1]
        (root / "a.idle").touch()  # Start without initializing a native observer.
        for side, display, peer in (("a", ":91", "b"), ("b", ":92", "a")):
            args = ["strace", "-f", "-qq", "-e", "trace=listen", "-o", str(root / f"{side}.trace"),
                    binary, "pilot", "--replace-clipboard", "--relay", f"wss://localhost:{relay_port}/our-clipboard/v1",
                    "--relay-ca", str(cert), "--room", room, "--side", side,
                    "--offer", str(root / f"{side}.offer"), "--peer-offer", str(root / f"{peer}-for-{side}.offer"),
                    "--seconds", "90" if reconnect_mode == "stale" else "30" if reconnect_mode else "14", "--ttl-ms", "30000" if reconnect_mode == "stale" else "3000" if reconnect_mode else "1000", "--idle-file", str(root / f"{side}.idle"), "--pause-file", str(root / f"{side}.pause"), "--max-drift-ppm", "1000",
                    "--clock-granularity-ms", "1"]
            if signed_offers:
                args.extend(["--identity-profile", "pilot-" + side])
            if named_peers:
                args.extend(["--peer-name", "remote"])
            if reconnect_mode:
                args.append("--reconnect")
            if logind_fixture:
                args.append("--logind-idle")
            if launch_config:
                settings = {
                    "version": 1, "relay": f"wss://localhost:{relay_port}/our-clipboard/v1",
                    "room": room, "side": side, "identity_profile": "pilot-" + side, "peer_name": "remote",
                    "offer": f"{side}.offer", "peer_offer": f"{peer}-for-{side}.offer", "relay_ca": "relay.pem",
                    "idle_file": f"{side}.idle", "pause_file": f"{side}.pause", "seconds": 30,
                    "ttl_ms": 3000, "max_drift_ppm": 1000, "clock_granularity_ms": 1,
                    "reconnect": bool(reconnect_mode),
                }
                launch = root / f"{side}.json"
                launch.write_text(json.dumps(settings))
                launch.chmod(0o600)
                checked = subprocess.run([binary, "pilot", "config-check", "--file", str(launch)],
                                         env=env, capture_output=True, timeout=5)
                assert checked.returncode == 0 and b"No credentials" in checked.stdout, "Launch static validation failed"
                from preflight_scenario import check as check_local_preflight
                check_local_preflight(root, binary, env, settings, side, device_fingerprints)
                # The fixture cwd is /tmp, not the configuration's temporary directory.
                args = args[:args.index("pilot") + 1] + ["--config", str(launch), "--replace-clipboard"]
            clients[side] = spawn(args, env=dict(system_env.get(side, env), DISPLAY=display),
                                  stdin=subprocess.PIPE, log=root / f"{side}.log")
        wait_for(lambda: offer_ready(root / "a.offer") and offer_ready(root / "b.offer"))
        for side, peer in (("a", "b"), ("b", "a")):
            data = (root / f"{peer}.offer").read_bytes()
            temp = root / f"{peer}-for-{side}.tmp"
            shutil.copyfile(root / f"{peer}.offer", temp)
            temp.replace(root / f"{peer}-for-{side}.offer")
            if signed_offers:
                fingerprint = "ed25519-sha256-v1:" + hashlib.sha256(b"our-clipboard/device-id/v1\0" + data[5:37]).hexdigest()
                assert fingerprint == device_fingerprints[peer], "Pilot did not reload its initialized device key"
            else:
                fingerprint = "sha256:" + hashlib.sha256(data).hexdigest()
            if not named_peers:
                clients[side].stdin.write((fingerprint + "\n").encode())
                clients[side].stdin.flush()
            # EOF in named mode proves it never falls back to an interactive prompt.
            clients[side].stdin.close()
        wait_for(lambda: "IDLE:" in status(root / "a.log") and "READY:" in status(root / "b.log"),
                 40 if reconnect_mode == "stale" else 10)
        assert "READY:" not in status(root / "a.log"), "Paused startup initialized the observer"
        (root / "a.idle").unlink()
        if logind_fixture:
            time.sleep(0.2)
            assert "READY:" not in status(root / "a.log"), "Manual marker bypassed locked system session"
            leave_idle("a")
        wait_for(lambda: all("READY:" in status(root / f"{s}.log") for s in ("a", "b")))
        assert inspect(":91", "inspect-request") and inspect(":92", "inspect-response"), "Startup contents propagated"
        seed(":91", "seed-request")  # Same text, new local copy.
        wait_for(lambda: inspect(":92", "inspect-request"), 5)
        wait_for(lambda: "Peer confirmed application." in status(root / "a.log"), 5)
        seed(":92", "seed-response")
        wait_for(lambda: inspect(":91", "inspect-response"), 5)
        wait_for(lambda: "Peer confirmed application." in status(root / "b.log"), 5)
        if reconnect_mode:
            from reconnect_scenario import run
            expected_apps, expected_pending, expected_exit = run(
                root, clients, mode=reconnect_mode, stop_relay=stop_relay, restart_relay=restart_relay,
                seed=seed, inspect=inspect, status=status, wait_for=wait_for)
        else:
            expected_apps, expected_pending, expected_exit = 3, 1, 0
            # While paused, local copies are not read/published and remote values are
            # buffered without writes/ACKs. Expiry keeps its original signed TTL.
            enter_idle("b")
            local_before = status(root / "b.log").count("New local copy selected.")
            seed(":92", "seed-request")
            time.sleep(0.2)
            assert inspect(":91", "inspect-response"), "Idle local copy was published"
            seed(":91", "seed-response")
            wait_for(lambda: "Peer event buffered while Idle" in status(root / "b.log"))
            assert inspect(":92", "inspect-request"), "Idle remote event wrote clipboard"
            time.sleep(1.4)
            leave_idle("b")
            wait_for(lambda: "ACTIVE:" in status(root / "b.log"))
            time.sleep(0.4)
            assert inspect(":92", "inspect-request"), "Expired Idle event was applied"
            assert inspect(":91", "inspect-response"), "Paused copy was replayed on resume"
            assert status(root / "b.log").count("New local copy selected.") == local_before, "Observer replayed an Idle change"
            # A new copy after resuming is observable, even with the same payload.
            seed(":92", "seed-response")
            wait_for(lambda: status(root / "b.log").count("Peer confirmed application.") == 2)
            # A fresh pending value is applied once after resume, without TTL renewal.
            enter_idle("b")
            buffered_before = status(root / "b.log").count("Peer event buffered while Idle")
            seed(":91", "seed-request")
            wait_for(lambda: status(root / "b.log").count("Peer event buffered while Idle") > buffered_before)
            assert inspect(":92", "inspect-response"), "Idle buffer wrote before resume"
            leave_idle("b")
            wait_for(lambda: inspect(":92", "inspect-request"))
            wait_for(lambda: status(root / "a.log").count("Peer confirmed application.") == 2)
            # Manual Pause ends the authenticated session; unlike Idle it cannot resume.
            enter_idle("a")
            seed(":92", "seed-response")
            wait_for(lambda: "Peer event buffered while Idle" in status(root / "a.log"))
            assert inspect(":91", "inspect-request"), "Idle value applied before manual Pause"
            if logind_fixture:
                for state in ("inactive", "sleep", "unknown"):
                    system_state("a", state)
                    time.sleep(0.2)
                    expected = "Unknown" if state == "unknown" else "Idle"
                    wait_for(lambda: f"System session: {expected}" in status(root / "a.log"))
                    assert inspect(":91", "inspect-request"), "System Idle wrote native clipboard"
                # A replacement well-known service must not unlock the pinned monitor.
                system_workers["a"].terminate()
                system_workers["a"].wait(timeout=3)
                system_state("a", "active")
                start_system("a")
                active_before = status(root / "a.log").count("ACTIVE:")
                time.sleep(0.5)
                assert status(root / "a.log").count("ACTIVE:") == active_before, "Replacement login1 unlocked session"
                assert inspect(":91", "inspect-request"), "Unknown system state wrote clipboard"
            (root / "a.pause").touch()
            wait_for(lambda: "PAUSED: session ended" in status(root / "a.log"))
        for side, process in clients.items():
            assert process.wait(timeout=35) == expected_exit, "Pilot exited unsuccessfully"
            assert "listen(" not in (root / f"{side}.trace").read_text(), "Endpoint tried to listen"
        bad_args = ["strace", "-f", "-qq", "-e", "trace=listen,connect", "-o", str(root / "bad.trace"),
                     binary, "pilot", "--replace-clipboard", "--relay", f"wss://localhost:{relay_port}/our-clipboard/v1",
                     "--relay-ca", str(cert), "--room", room, "--side", "a", "--offer", str(root / "bad.offer"),
                     "--peer-offer", str(root / "b.offer"), "--seconds", "12", "--ttl-ms", "1000",
                     "--max-drift-ppm", "1000", "--clock-granularity-ms", "1"]
        if signed_offers:
            bad_args.extend(["--identity-profile", "pilot-a"])
        bad = spawn(bad_args, env=dict(env, DISPLAY=":91"), stdin=subprocess.PIPE, log=root / "bad.log")
        bad.stdin.write(b"yes\n")
        bad.stdin.close()
        assert bad.wait(timeout=5) == 2, "Unverified fingerprint was accepted"
        assert "Peer fingerprint confirmation failed" in status(root / "bad.log"), "Expected trust rejection diagnostic"
        connections = [line for line in (root / "bad.trace").read_text().splitlines() if "connect(" in line]
        # Signing reloads Secret Service over local Unix IPC before the prompt.
        # No Internet/TCP connection may precede fingerprint confirmation.
        assert not connections or (signed_offers and all("AF_UNIX" in line for line in connections)), "Transport connected before fingerprint confirmation"
        assert "listen(" not in (root / "bad.trace").read_text(), "Rejected endpoint tried to listen"
        if signed_offers:
            # A signed-mode pilot must reject an otherwise valid legacy offer,
            # even if stdin would confirm it. Never silently downgrade trust.
            legacy = root / "legacy.offer"
            legacy.write_bytes((root / "b.offer").read_bytes()[90:-64])
            downgrade_args = bad_args.copy()
            for option, value in (("--offer", root / "downgrade.offer"),
                                  ("--peer-offer", legacy), ("-o", root / "downgrade.trace")):
                downgrade_args[downgrade_args.index(option) + 1] = str(value)
            downgrade = spawn(downgrade_args, env=dict(env, DISPLAY=":91"),
                              stdin=subprocess.DEVNULL, log=root / "downgrade.log")
            assert downgrade.wait(timeout=5) == 2, "Unsigned offer accepted in signed mode"
            assert "no legacy fallback" in status(root / "downgrade.log"), "Missing downgrade diagnostic"
            trace = (root / "downgrade.trace").read_text()
            assert all("AF_UNIX" in line for line in trace.splitlines() if "connect(" in line), "Downgrade reached relay"
            assert "listen(" not in trace, "Downgrade rejection opened a listener"
        if enroll_peers:
            from peer_pin_scenario import enroll
            enroll(root, binary, env, room, device_fingerprints)
        if launch_config:
            print("Launch configuration passed: relative paths, separate consent and selected peer, real WSS reconnect")
        if named_peers:
            for side in clients:
                assert "SAVED PEER VERIFIED:" in status(root / f"{side}.log"), "No explicit pin verification"
                assert "then type that full fingerprint" not in status(root / f"{side}.log"), "Named peer prompted"
            from peer_pin_scenario import reject_changed_or_unusable_trust
            reject_changed_or_unusable_trust(root, binary, env, room, bad_args,
                                            spawn, status, wait_for, offer_ready)
        logs = status(root / "a.log") + status(root / "b.log")
        assert logs.count("Applied peer event.") == expected_apps, "Unexpected repeated application/echo"
        assert logs.count("Applied pending peer event.") == expected_pending, "Buffered value was not applied exactly once"
        if signed_offers:
            for side in ("a", "b"):
                result = subprocess.run([binary, "identity", "show", "--profile", "pilot-" + side],
                                        env=env, capture_output=True, timeout=10)
                assert result.returncode == 0 and device_fingerprints[side] in result.stdout.decode(), "Device identity changed"
            print("System-store signed offers passed: stable device key, bound session/room/role, fresh TLS and confirmations")
        if reconnect_mode:
            print("Reconnect fault injection passed: " + reconnect_mode)
        print("Outbound WSS pilot passed: Idle startup/pause/resume/TTL, real HTTPS, pinned peer TLS, independent confirmations, startup isolation, bidirectional writes/ACKs and no client listen syscall.")
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

#!/usr/bin/env python3
"""Exercise the shipped unprivileged image; never publish a host port."""
import json
import os
from pathlib import Path
import secrets
import subprocess

root = Path(__file__).resolve().parents[2]
name = "our-clipboard-relay-test-" + secrets.token_hex(8)
env = dict(os.environ, OUR_CLIPBOARD_RELAY_TOKEN=secrets.token_hex(32))

def docker(*args, **kwargs):
    return subprocess.run(["docker", *args], env=env, check=True, **kwargs)

try:
    docker("volume", "create", name, stdout=subprocess.DEVNULL)
    # Test-only certificate/key, generated inside an isolated container and volume.
    docker("run", "--rm", "--network", "none", "-v", name + ":/tls",
           "our-clipboard-relay-tests", "sh", "-ec",
           'openssl req -x509 -newkey ed25519 -nodes -days 1 '
           '-keyout /tls/privkey.pem -out /tls/fullchain.pem -subj /CN=localhost '
           '-addext subjectAltName=DNS:localhost -addext basicConstraints=critical,CA:FALSE '
           '-addext keyUsage=critical,digitalSignature -addext extendedKeyUsage=serverAuth '
           '>/dev/null 2>&1; chmod 600 /tls/privkey.pem; chmod 755 /tls; chown -R 65532:65532 /tls')
    docker("run", "-d", "--name", name, "--network", "none", "--read-only",
           "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true",
           "--pids-limit", "64", "--memory", "128m", "--cpus", "1",
           "-e", "OUR_CLIPBOARD_RELAY_TOKEN", "-v", name + ":/tls:ro",
           "our-clipboard-relay", stdout=subprocess.DEVNULL)
    config = json.loads(docker("inspect", name, capture_output=True, text=True).stdout)[0]
    assert config["Config"]["User"] == "65532:65532"
    assert config["HostConfig"]["ReadonlyRootfs"]
    assert not config["HostConfig"]["PortBindings"], "Test must never publish host ports"
    docker("run", "--rm", "--network", "container:" + name,
           "-e", "OUR_CLIPBOARD_RELAY_TOKEN", "-e", "OUR_CLIPBOARD_EXTERNAL_RELAY=1",
           "-v", name + ":/tls:ro",
           "-v", str(root / "target/debug/our-clipboard") + ":/pilot:ro",
           "-v", str(root / "target/debug/examples/synthetic-clipboard-sync") + ":/helper:ro",
           "-v", str(root / "tests/linux/run-relay-pilot.py") + ":/run-relay-pilot.py:ro",
           "our-clipboard-relay-tests", "python3", "/run-relay-pilot.py", "/pilot", "/helper")
    assert docker("inspect", "--format", "{{.State.Running}}", name,
                  capture_output=True, text=True).stdout.strip() == "true"
    print("Packaged relay passed: UID 65532, read-only root, no capabilities, no host ports, native WSS exchange.")
finally:
    subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["docker", "volume", "rm", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

"""Fault injection for the private WSS/native fixture; never targets external services."""
import time

def run(root, clients, *, mode, stop_relay, restart_relay, seed, inspect, status, wait_for):
    def count(side, label):
        return status(root / f"{side}.log").count(label)
    def disconnected(number):
        wait_for(lambda: all(count(s, "DISCONNECTED:") >= number for s in clients), 5)
    def resumed(number):
        wait_for(lambda: all(count(s, "RECONNECTED:") >= number for s in clients), 10)
    if mode == "auth":
        stop_relay()
        disconnected(1)
        restart_relay(wrong_token=True)
        for side, process in clients.items():
            assert process.wait(timeout=10) == 2, "Reconnect accepted rotated transport credential"
            assert "(Authentication)" in status(root / f"{side}.log"), "Auth error was silently retried"
        return 2, 0, 2
    # Local copies selected while offline keep their original creation time/TTL.
    stop_relay()
    disconnected(1)
    before = count("a", "New local copy selected.")
    seed(":91", "seed-request")
    wait_for(lambda: count("a", "New local copy selected.") > before)
    restart_relay()
    resumed(1)
    wait_for(lambda: inspect(":92", "inspect-request"), 5)
    wait_for(lambda: count("a", "Peer confirmed application.") == 2)
    assert count("a", "Authenticated pilot connected.") == 1, "Reconnect recreated application session"
    # An older offline copy must not acquire a new TTL when transport returns.
    stop_relay()
    disconnected(2)
    before = count("a", "New local copy selected.")
    seed(":91", "seed-response")
    wait_for(lambda: count("a", "New local copy selected.") > before)
    wait_for(lambda: count("a", "Latest local event expired") >= 1, 5)
    restart_relay()
    resumed(2)
    time.sleep(0.3)
    assert inspect(":92", "inspect-request"), "Expired offline text was replayed"
    # New copy of identical text after reconnect is a new eligible event.
    seed(":91", "seed-response")
    wait_for(lambda: inspect(":92", "inspect-response"), 5)
    wait_for(lambda: count("a", "Peer confirmed application.") == 3)
    # Pause must remain responsive while reconnect attempts are outstanding.
    stop_relay()
    disconnected(3)
    (root / "b.pause").touch()
    assert clients["b"].wait(timeout=2) == 0, "Offline Pause was blocked"
    attempts_before = count("a", "RECONNECTING:")
    restart_relay()
    wait_for(lambda: count("a", "RECONNECTING:") > attempts_before, 5)
    time.sleep(0.2)  # A waits for its absent peer inside native TLS setup.
    started = time.monotonic()
    (root / "a.pause").touch()
    assert clients["a"].wait(timeout=2) == 0, "Pause waited for network setup"
    assert time.monotonic() - started < 2, "Pause was blocked by reconnect"
    for side in clients:
        assert "PAUSED: session ended" in status(root / f"{side}.log")
    return 4, 0, 0

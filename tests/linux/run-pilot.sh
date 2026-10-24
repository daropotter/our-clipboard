#!/bin/bash
set -euo pipefail
binary=$1
helper=$2
unset WAYLAND_DISPLAY WAYLAND_SOCKET
export XDG_SESSION_TYPE=x11
work=$(mktemp -d /root/our-clipboard-pilot.XXXXXX)
pids=()
cleanup() { for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done; rm -rf "$work"; }
trap cleanup EXIT
Xvfb :81 -screen 0 1280x720x24 -nolisten tcp >"$work/source.log" 2>&1 & pids+=("$!")
Xvfb :82 -screen 0 1280x720x24 -nolisten tcp >"$work/target.log" 2>&1 & pids+=("$!")
for display in :81 :82; do
  ready=0
  for attempt in {1..50}; do
    if DISPLAY="$display" "$helper" --replace-clipboard check-display >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.1
  done
  test "$ready" = 1
done
# Real SSH on private container loopback, with fresh keys and pinned known_hosts.
# No host keyring, agent, known_hosts or SSH account is used.
ssh-keygen -q -t ed25519 -N '' -f "$work/server"
ssh-keygen -q -t ed25519 -N '' -f "$work/client"
read -r algorithm public_key ignored < "$work/server.pub"
printf '[127.0.0.1]:2222 %s %s\n' "$algorithm" "$public_key" > "$work/known_hosts"
cat > "$work/sshd_config" <<CONFIG
Port 2222
ListenAddress 127.0.0.1
HostKey $work/server
PidFile $work/sshd.pid
AuthorizedKeysFile $work/client.pub
PermitRootLogin prohibit-password
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
SetEnv DISPLAY=:82 XDG_SESSION_TYPE=x11
CONFIG
/usr/sbin/sshd -D -e -f "$work/sshd_config" >"$work/sshd.log" 2>&1 & pids+=("$!")
cat > "$work/ssh" <<'SSH'
#!/bin/bash
set -euo pipefail
exec /usr/bin/ssh -oHostName=127.0.0.1 -oPort=2222 -oUser=root \
  -oIdentityFile="$OUR_CLIPBOARD_SSH_TEST_DIRECTORY/client" -oIdentitiesOnly=yes -oIdentityAgent=none \
  -oUserKnownHostsFile="$OUR_CLIPBOARD_SSH_TEST_DIRECTORY/known_hosts" "$@"
SSH
chmod +x "$work/ssh"
export OUR_CLIPBOARD_SSH_TEST_DIRECTORY="$work"
ssh_ready=0
for attempt in {1..50}; do
  if "$work/ssh" -T -oBatchMode=yes -oStrictHostKeyChecking=yes fixture.invalid true >/dev/null 2>&1; then ssh_ready=1; break; fi
  sleep 0.1
done
if [ "$ssh_ready" != 1 ]; then cat "$work/sshd.log"; exit 1; fi
DISPLAY=:81 "$helper" --replace-clipboard seed-request >"$work/seed1.log" 2>&1 & pids+=("$!")
DISPLAY=:82 "$helper" --replace-clipboard seed-response >"$work/seed2.log" 2>&1 & pids+=("$!")
sleep 0.2
DISPLAY=:81 PATH="$work:$PATH" "$binary" pilot --replace-clipboard --ssh fixture.invalid --remote-program "$binary" --seconds 12 --ttl-ms 1000 --max-drift-ppm 1000 --clock-granularity-ms 1 >"$work/stdout" 2>"$work/status" &
pilot_pid=$!; pids+=("$pilot_pid")
ready=0
for attempt in {1..100}; do
  if [ "$(grep -o 'READY:' "$work/status" | wc -l)" -ge 2 ]; then ready=1; break; fi
  if ! kill -0 "$pilot_pid" 2>/dev/null; then cat "$work/status"; exit 1; fi
  sleep 0.1
done
if [ "$ready" != 1 ]; then cat "$work/status"; exit 1; fi
# Neither endpoint may publish startup contents.
DISPLAY=:81 "$helper" --replace-clipboard inspect-request
DISPLAY=:82 "$helper" --replace-clipboard inspect-response
DISPLAY=:81 "$helper" --replace-clipboard seed-request >"$work/new-copy1.log" 2>&1 & pids+=("$!")
seen=0
for attempt in {1..50}; do
  if DISPLAY=:82 "$helper" --replace-clipboard inspect-request >/dev/null 2>&1; then seen=1; break; fi
  sleep 0.1
done
if [ "$seen" != 1 ]; then cat "$work/status"; exit 1; fi
DISPLAY=:82 "$helper" --replace-clipboard seed-response >"$work/new-copy2.log" 2>&1 & pids+=("$!")
sleep 0.1
DISPLAY=:82 "$helper" --replace-clipboard inspect-response || { cat "$work/status"; exit 1; }
seen=0
for attempt in {1..50}; do
  if DISPLAY=:81 "$helper" --replace-clipboard inspect-response >/dev/null 2>&1; then seen=1; break; fi
  sleep 0.1
done
if [ "$seen" != 1 ]; then cat "$work/status"; exit 1; fi
wait "$pilot_pid"
test ! -s "$work/stdout"
test "$(grep -c 'Peer confirmed application.' "$work/status")" -ge 2
test "$(grep -c 'Applied peer event.' "$work/status")" = 2
printf '%s\n' 'Interactive pilot over real SSH: startup isolation, same-text new copy, bidirectional native writes, receiver ACKs and no echo passed.'

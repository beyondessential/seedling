#!/usr/bin/env bash
# Start a stubbed seedling daemon + seedling-web pair for DAST scanning.
#
# Mirrors the Playwright e2e fixture (crates/web/frontend/e2e/fixture.ts): the
# daemon runs with in-memory backend stubs (no podman, systemd, nftables,
# Caddy or NAT64) so it needs no root, and seedling-web runs with
# --dev-no-auth so the HTTP surface is reachable without a login handshake.
#
# Both listeners bind loopback (seedling-web refuses --dev-no-auth on any
# non-loopback address), so a containerised scanner must share the host
# network (docker run --network=host) to reach them.
#
# Usage: etc/ci/dast-stack.sh <work-dir>
#   work-dir : scratch directory for daemon data, client key and pidfiles
#
# On success, writes <work-dir>/http-port and <work-dir>/pids, then exits 0
# once seedling-web answers /healthz. Stop the stack with:
#   kill $(cat <work-dir>/pids)
set -euo pipefail

WORK="${1:?usage: dast-stack.sh <work-dir>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="$ROOT/target/debug"

DAEMON="$BIN/seedling"
WEB="$BIN/seedling-web"
CTL="$BIN/seedling-ctl"
for b in "$DAEMON" "$WEB" "$CTL"; do
    [[ -x "$b" ]] || { echo "missing binary $b — run 'just build' first" >&2; exit 1; }
done

DATA="$WORK/daemon"
STATE="$WORK/state"
mkdir -p "$DATA" "$STATE/seedling"

# Generate a client key and authorise its fingerprint with the daemon.
FP="$(XDG_STATE_HOME="$STATE" "$CTL" client fingerprint | awk '{print $1; exit}')"
echo "$FP dast" > "$DATA/authorized_keys"

free_port() {
    python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}
OI_PORT="$(free_port)"
HTTP_PORT="$(free_port)"
WT_PORT="$(free_port)"

SEEDLING_LOG="${SEEDLING_LOG:-seedling=info,warn}" \
    nohup "$DAEMON" --stub-backends --without-btrfs \
    --data-dir "$DATA" --listen "127.0.0.1:$OI_PORT" \
    --audit-log "$DATA/audit.log" > "$WORK/daemon.log" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 300); do
    grep -q "seedling ready" "$WORK/daemon.log" && break
    kill -0 "$DAEMON_PID" 2>/dev/null || { echo "daemon exited early:" >&2; cat "$WORK/daemon.log" >&2; exit 1; }
    sleep 0.1
done

SEEDLING_WEB_LOG="${SEEDLING_WEB_LOG:-seedling_web=info,warn}" \
    nohup "$WEB" --dev-no-auth --daemon-trust-any \
    --http-port "$HTTP_PORT" --wt-port "$WT_PORT" \
    --daemon-addr "127.0.0.1:$OI_PORT" \
    --key-file "$STATE/seedling/client.key" > "$WORK/web.log" 2>&1 &
WEB_PID=$!

echo "$HTTP_PORT" > "$WORK/http-port"
echo "$DAEMON_PID $WEB_PID" > "$WORK/pids"

for _ in $(seq 1 300); do
    curl -sf "http://127.0.0.1:$HTTP_PORT/healthz" >/dev/null 2>&1 && {
        echo "web ready on 127.0.0.1:$HTTP_PORT" >&2
        exit 0
    }
    kill -0 "$WEB_PID" 2>/dev/null || { echo "web exited early:" >&2; cat "$WORK/web.log" >&2; exit 1; }
    sleep 0.1
done

echo "web failed to become ready:" >&2
cat "$WORK/web.log" >&2
exit 1

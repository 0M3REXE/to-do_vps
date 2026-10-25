#!/usr/bin/env bash
set -euo pipefail
# Install a prebuilt release binary, restart the service, and check health.
# Build on GitHub Actions or another matching Linux x86_64 machine, never here.
#
# Run from the repo root as root:
#
#     sudo ./deploy.sh /tmp/todo.new
cd "$(dirname "$0")"

fail() { echo "deploy: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || fail "run as root: sudo ./deploy.sh /path/to/prebuilt-todo"
[ "$#" -eq 1 ] || fail "usage: sudo ./deploy.sh /path/to/prebuilt-todo"
[ -s "$1" ] || fail "binary not found: $1"
[ -f .env ] || fail "no .env here — cp .env.example .env first"
[ -f /etc/systemd/system/todo.service ] || fail "units missing — run deploy/setup.sh first"

install -m 755 "$1" /usr/local/bin/todo

systemctl restart todo

echo "waiting for the app..."
for _ in $(seq 1 30); do
    if curl -fsS --max-time 2 http://127.0.0.1:3000/healthz >/dev/null 2>&1; then
        echo "healthy"
        exit 0
    fi
    sleep 2
done

echo "the app did not answer within 60s; recent logs:" >&2
journalctl -u todo -n 50 --no-pager >&2
exit 1

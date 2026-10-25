#!/usr/bin/env bash
set -euo pipefail
# Build and roll the app: regenerate the CSS when Node is available, compile
# the release binary, install it, restart the service, and wait for a real
# health response. Boot runs the embedded migrations, so a healthy answer also
# proves the app can reach sqld.
#
# Run from the repo root as root:
#
#     sudo ./deploy.sh
cd "$(dirname "$0")"

fail() { echo "deploy: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || fail "run as root: sudo ./deploy.sh"
[ -f .env ] || fail "no .env here — cp .env.example .env first"
[ -f /etc/systemd/system/practice.service ] || fail "units missing — run deploy/setup.sh first"

# rustup's cargo is not on root's default PATH.
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
command -v cargo >/dev/null 2>&1 || fail "cargo not found — run deploy/setup.sh first"

# static/app.css is include_str!-ed at compile time, so it must exist and be
# current. Without Node it must have been built and committed elsewhere.
if command -v npm >/dev/null 2>&1; then
    npm ci --no-audit --no-fund
    npm run css:build
elif [ ! -s static/app.css ]; then
    fail "static/app.css is missing and npm is not installed — install Node or commit a built CSS"
fi

cargo build --release --locked
install -m 755 target/release/practice /usr/local/bin/practice

systemctl restart practice

echo "waiting for the app..."
for _ in $(seq 1 30); do
    if curl -fsS --max-time 2 http://127.0.0.1:3000/healthz >/dev/null 2>&1; then
        echo "healthy"
        exit 0
    fi
    sleep 2
done

echo "the app did not answer within 60s; recent logs:" >&2
journalctl -u practice -n 50 --no-pager >&2
exit 1

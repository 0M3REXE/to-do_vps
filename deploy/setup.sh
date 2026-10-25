#!/usr/bin/env bash
set -euo pipefail
# One-time VPS provisioning. Run from the repo root as root:
#
#     sudo ./deploy/setup.sh
#
# Installs what the stack needs and no more: the service user, the official
# libsql-server (`sqld`) binary, Caddy, and
# the two systemd units. Idempotent — re-running is safe.
#
# It does not build or start the app; deploy.sh installs its prebuilt binary.
#
# Assumes a Debian/Ubuntu VPS (apt). No Docker involved anywhere.

SQLD_VERSION=0.24.32
SERVICE_USER=practice # Keep the existing service account and database path across app renames.
DATA_DIR=/var/lib/practice

cd "$(dirname "$0")/.."
APP_DIR="$(pwd -P)"

fail() { echo "setup: $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

[ "$(id -u)" -eq 0 ] || fail "run as root: sudo ./deploy/setup.sh"
[ -f .env ] || fail "no .env here — cp .env.example .env, set DOMAIN, then re-run"

# The domain Caddy will get a certificate for. Read from the same .env the
# app's systemd unit loads; restricted to hostname characters because it is
# substituted into the Caddyfile below.
DOMAIN="$(set -a; . ./.env; set +a; printf '%s' "${DOMAIN:-}")"
[[ "$DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] || fail "DOMAIN must be set in .env (got: ${DOMAIN:-empty})"

step "packages"
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    apt-transport-https ca-certificates curl \
    debian-archive-keyring debian-keyring gnupg unzip xz-utils

step "service user and data directory"
id -u "$SERVICE_USER" >/dev/null 2>&1 || \
    useradd --system --home-dir "$DATA_DIR" --shell /usr/sbin/nologin "$SERVICE_USER"
mkdir -p "$DATA_DIR"
chown "$SERVICE_USER:$SERVICE_USER" "$DATA_DIR"

step "libsql-server (sqld) ${SQLD_VERSION}"
case "$(uname -m)" in
    x86_64)  target=x86_64-unknown-linux-gnu ;;
    aarch64) target=aarch64-unknown-linux-gnu ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
esac
tmp="$(mktemp -d)"
curl -fsSL "https://github.com/tursodatabase/libsql/releases/download/libsql-server-v${SQLD_VERSION}/libsql-server-${target}.tar.xz" \
    | tar -xJ -C "$tmp"
install -m 755 "$tmp"/libsql-server-*/sqld /usr/local/bin/sqld
rm -rf "$tmp"

step "caddy"
if ! command -v caddy >/dev/null 2>&1; then
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | \
        gpg --batch --yes --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | \
        tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
    chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg /etc/apt/sources.list.d/caddy-stable.list
    apt-get update -qq
    apt-get install -y -qq caddy
fi

step "Caddyfile for ${DOMAIN}"
sed "s/{\$DOMAIN}/${DOMAIN}/g" Caddyfile > /etc/caddy/Caddyfile

step "systemd units"
# todo.service embeds the repo's real path; @APP_DIR@ is the placeholder.
sed "s|@APP_DIR@|${APP_DIR}|g" deploy/todo.service > /etc/systemd/system/todo.service
install -m 644 deploy/sqld.service /etc/systemd/system/sqld.service
systemctl daemon-reload
systemctl enable --now sqld.service
if [ -f /etc/systemd/system/practice.service ]; then
    if systemctl is-active --quiet practice.service; then
        systemctl stop practice.service
    fi
    systemctl disable practice.service || true
    rm -f /etc/systemd/system/practice.service
fi
systemctl daemon-reload
systemctl enable todo.service           # started by deploy.sh after binary install
systemctl restart caddy.service

step "done — services so far:"
systemctl is-enabled sqld.service caddy.service todo.service
echo
echo "Next: install a prebuilt Linux binary with sudo ./deploy.sh /path/to/todo"

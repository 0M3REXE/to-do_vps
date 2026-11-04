#!/usr/bin/env bash
# ==============================================================================
# Atomic Blue-Green Switcher Script
# Executes on the VPS via GitHub Actions deploy step or locally.
# Usage: ./switch.sh <APP_NAME> <PORT_BLUE> <PORT_GREEN> <HEALTH_PATH> <DOMAIN>
# ==============================================================================
set -euo pipefail

APP_NAME="${1:?Missing APP_NAME}"
PORT_BLUE="${2:?Missing PORT_BLUE}"
PORT_GREEN="${3:?Missing PORT_GREEN}"
HEALTH_PATH="${4:-/healthz}"
DOMAIN="${5:?Missing DOMAIN}"

APP_ROOT="/opt/apps/${APP_NAME}"
UPSTREAM_CONF="/etc/nginx/upstreams/${APP_NAME}.conf"
UPSTREAM_BLUE="/etc/nginx/upstreams/${APP_NAME}-blue.conf"
UPSTREAM_GREEN="/etc/nginx/upstreams/${APP_NAME}-green.conf"

mkdir -p "${APP_ROOT}/shared"
mkdir -p "${APP_ROOT}/bin"
mkdir -p "/etc/nginx/upstreams"

# 1. Determine currently active color
CURRENT_COLOR="none"
if [ -L "${UPSTREAM_CONF}" ]; then
    TARGET=$(readlink "${UPSTREAM_CONF}")
    if [[ "${TARGET}" == *"blue"* ]]; then
        CURRENT_COLOR="blue"
    elif [[ "${TARGET}" == *"green"* ]]; then
        CURRENT_COLOR="green"
    fi
fi

if [ "${CURRENT_COLOR}" == "blue" ]; then
    TARGET_COLOR="green"
    TARGET_PORT="${PORT_GREEN}"
    OLD_PORT="${PORT_BLUE}"
else
    TARGET_COLOR="blue"
    TARGET_PORT="${PORT_BLUE}"
    OLD_PORT="${PORT_GREEN}"
fi

echo "===> Current active slot: [${CURRENT_COLOR}]. Deploying to target slot: [${TARGET_COLOR}] on port ${TARGET_PORT}..."

# 2. Prepare upstream definitions
echo "upstream ${APP_NAME}_backend { server 127.0.0.1:${PORT_BLUE}; }" > "${UPSTREAM_BLUE}"
echo "upstream ${APP_NAME}_backend { server 127.0.0.1:${PORT_GREEN}; }" > "${UPSTREAM_GREEN}"

# 3. Create or update systemd service for the target slot
SERVICE_NAME="${APP_NAME}-${TARGET_COLOR}"
SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"

cat <<EOF | sudo /usr/bin/tee "${SERVICE_FILE}" > /dev/null
[Unit]
Description=${APP_NAME} (${TARGET_COLOR} slot)
After=network.target

[Service]
Type=simple
User=deploy
Group=www-data
WorkingDirectory=${APP_ROOT}
ExecStart=${APP_ROOT}/bin/${APP_NAME}
EnvironmentFile=-${APP_ROOT}/shared/.env
Environment=PORT=${TARGET_PORT}
Environment=BIND_ADDRESS=127.0.0.1
Environment=DATA_ROOT=${APP_ROOT}/shared
Restart=always
RestartSec=3
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

sudo /usr/bin/systemctl daemon-reload
echo "===> Starting service [${SERVICE_NAME}] on port ${TARGET_PORT}..."
sudo /usr/bin/systemctl restart "${SERVICE_NAME}"

# 4. Deep Healthcheck polling loop (Wait up to 30s)
echo "===> Probing http://127.0.0.1:${TARGET_PORT}${HEALTH_PATH}..."
RETRIES=15
HEALTHY=0

for i in $(seq 1 $RETRIES); do
    STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TARGET_PORT}${HEALTH_PATH}" || echo "000")
    if [ "$STATUS" == "200" ]; then
        echo "✅ Health check passed! HTTP status: 200 (attempt $i)"
        HEALTHY=1
        break
    else
        echo "   [Attempt $i/$RETRIES] Health status: $STATUS. Retrying in 2s..."
        sleep 2
    fi
done

if [ "$HEALTHY" -ne 1 ]; then
    echo "❌ ERROR: Target slot [${TARGET_COLOR}] failed health checks!"
    echo "===> Rolling back: stopping failed slot [${SERVICE_NAME}]..."
    sudo /usr/bin/systemctl stop "${SERVICE_NAME}" || true
    echo "===> Old slot [${CURRENT_COLOR}] was kept completely untouched and running."
    exit 1
fi

# 5. Atomic upstream link switch and zero-downtime Nginx reload
echo "===> Health check passed! Switching Nginx upstream to [${TARGET_COLOR}]..."
ln -sfn "/etc/nginx/upstreams/${APP_NAME}-${TARGET_COLOR}.conf" "${UPSTREAM_CONF}"
sudo /usr/sbin/nginx -t
sudo /usr/bin/systemctl reload nginx

# 6. Graceful shutdown of old slot
if [ "${CURRENT_COLOR}" != "none" ]; then
    OLD_SERVICE="${APP_NAME}-${CURRENT_COLOR}"
    echo "===> Gracefully stopping old slot [${OLD_SERVICE}]..."
    # Give in-flight requests 5 seconds to finish before stopping
    sleep 5
    sudo /usr/bin/systemctl stop "${OLD_SERVICE}" || true
fi

echo "=========================================================================="
echo "🎉 Zero-Downtime Deployment Succeeded! Active slot is now [${TARGET_COLOR}]"
echo "=========================================================================="

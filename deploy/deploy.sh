#!/usr/bin/env bash
# ==============================================================================
# deploy.sh — Rootless Podman + Caddy Zero-Downtime Blue/Green Deploy
#
# Usage: deploy.sh <app> <image_name> <image_tag>
#   e.g.: deploy.sh todo ghcr.io/0m3rexe/to-do/todo abc1234
# ==============================================================================
set -euo pipefail

APP="${1:?Usage: deploy.sh <app> <image_name> <image_tag>}"
IMAGE_NAME="${2:?Usage: deploy.sh <app> <image_name> <image_tag>}"
TAG="${3:?Usage: deploy.sh <app> <image_name> <image_tag>}"

APP_ROOT="/opt/apps/${APP}"
MANIFEST="${APP_ROOT}/app.toml"
CADDY_APP_FILE="/etc/caddy/apps/${APP}.caddy"

if [ ! -f "${MANIFEST}" ]; then
    echo "ERROR: Manifest ${MANIFEST} not found."
    exit 1
fi

# --- Helper to parse TOML keys -----------------------------------------------
read_toml() {
    grep -E "^\s*${1}\s*=" "${MANIFEST}" | head -1 | sed -E 's/.*=\s*"?([^"]*)"?.*/\1/' | tr -d "'" | tr -d '"' | xargs
}

read_toml_array() {
    grep -E "^\s*${1}\s*=" "${MANIFEST}" | head -1 | sed -E 's/.*\[//;s/\]//;s/"//g' | tr ',' '\n' | xargs -I{} echo {}
}

DOMAIN=$(read_toml domain)
HEALTH_PATH=$(read_toml health_path)
PORT_BLUE=$(read_toml port_blue)
PORT_GREEN=$(read_toml port_green)
MEM_LIMIT=$(read_toml memory_limit)
TB_ENABLED=$(read_toml "enabled")
TB_PORT=$(grep -A5 '\[trailbase\]' "${MANIFEST}" | grep -E '^\s*port\s*=' | sed -E 's/.*=\s*([0-9]+).*/\1/' | xargs)
TB_VERSION=$(grep -A5 '\[trailbase\]' "${MANIFEST}" | grep -E '^\s*version\s*=' | sed -E 's/.*=\s*"?([^"]*)"?.*/\1/' | tr -d '"' | xargs)
TB_MEM_LIMIT=$(read_toml tb_memory_limit)

[ -z "${PORT_BLUE}" ] && PORT_BLUE=4001
[ -z "${PORT_GREEN}" ] && PORT_GREEN=4002
[ -z "${HEALTH_PATH}" ] && HEALTH_PATH="/healthz"
[ -z "${TB_PORT}" ] && TB_PORT=4010
[ -z "${TB_VERSION}" ] && TB_VERSION="0.2.6"

FULL_IMAGE="${IMAGE_NAME}:${TAG}"

echo "=========================================================================="
echo "  Deploying App: ${APP}"
echo "  Image:         ${FULL_IMAGE}"
echo "  Domain:        ${DOMAIN}"
echo "  Slots:         blue=${PORT_BLUE}, green=${PORT_GREEN}"
echo "=========================================================================="

# --- Manage TrailBase sidecar (kept untouched across app deploys) ------------
if [ "${TB_ENABLED}" = "true" ]; then
    TB_CONTAINER="${APP}-trailbase"
    TB_DATA="${APP_ROOT}/traildepot"
    mkdir -p "${TB_DATA}"

    if ! podman ps --format "{{.Names}}" | grep -q "^${TB_CONTAINER}$"; then
        echo "====> Ensuring TrailBase sidecar is running on port ${TB_PORT}..."
        podman rm -f "${TB_CONTAINER}" 2>/dev/null || true
        podman run -d \
            --name "${TB_CONTAINER}" \
            --network=host \
            --restart=always \
            -v "${TB_DATA}:/app/traildepot:U" \
            "docker.io/trailbase/trailbase:${TB_VERSION}" \
            /app/trail --data-dir /app/traildepot run --address "127.0.0.1:${TB_PORT}"
        sleep 3
    else
        echo "====> TrailBase sidecar is healthy on port ${TB_PORT} (untouched)."
    fi
fi

# --- Determine active vs target slot -----------------------------------------
ACTIVE_SLOT_FILE="${APP_ROOT}/active_slot"
CURRENT_COLOR="none"
if [ -f "${ACTIVE_SLOT_FILE}" ]; then
    CURRENT_COLOR="$(cat "${ACTIVE_SLOT_FILE}" | tr -d '[:space:]')"
fi

if [ "${CURRENT_COLOR}" = "blue" ]; then
    TARGET_COLOR="green"
    TARGET_PORT="${PORT_GREEN}"
else
    TARGET_COLOR="blue"
    TARGET_PORT="${PORT_BLUE}"
fi

TARGET_CONTAINER="${APP}-${TARGET_COLOR}"
OLD_CONTAINER="${APP}-${CURRENT_COLOR}"

echo "====> Active slot: [${CURRENT_COLOR}] → Deploying to [${TARGET_COLOR}] on port ${TARGET_PORT}..."

# --- Pull target image -------------------------------------------------------
echo "====> Pulling image ${FULL_IMAGE}..."
podman pull "${FULL_IMAGE}"

# --- Start target container --------------------------------------------------
echo "====> Starting target container [${TARGET_CONTAINER}]..."
podman rm -f "${TARGET_CONTAINER}" 2>/dev/null || true

ENV_FILE="${APP_ROOT}/shared/.env"

RUN_CMD=(
    podman run -d
    --name "${TARGET_CONTAINER}"
    --network=host
    --restart=always
    -e "PORT=${TARGET_PORT}"
    -e "BIND_ADDRESS=127.0.0.1"
    -e "DATA_ROOT=/app/data"
    -v "${APP_ROOT}/shared:/app/data:U"
    -v "${APP_ROOT}/shared:${APP_ROOT}/shared:U"
)

if [ -n "${MEM_LIMIT}" ]; then
    RUN_CMD+=(--memory "${MEM_LIMIT}")
fi

if [ -f "${ENV_FILE}" ]; then
    RUN_CMD+=(--env-file "${ENV_FILE}")
fi

RUN_CMD+=("${FULL_IMAGE}")

"${RUN_CMD[@]}"

# --- Health check probe ------------------------------------------------------
echo "====> Probing health at http://127.0.0.1:${TARGET_PORT}${HEALTH_PATH}..."
RETRIES=20
HEALTHY=0
for i in $(seq 1 "$RETRIES"); do
    STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TARGET_PORT}${HEALTH_PATH}" || echo "000")
    if [ "$STATUS" = "200" ]; then
        echo "      Health check passed (attempt $i/20, HTTP 200)"
        HEALTHY=1
        break
    fi
    echo "      [Attempt $i/$RETRIES] status=${STATUS} — retrying in 2s..."
    sleep 2
done

if [ "$HEALTHY" -ne 1 ]; then
    echo "=========================================================================="
    echo "ERROR: Target slot [${TARGET_COLOR}] failed health checks!"
    echo "Container logs:"
    podman logs --tail 40 "${TARGET_CONTAINER}" 2>&1 || true
    echo "Aborting deployment. Removing failed container [${TARGET_CONTAINER}]..."
    podman rm -f "${TARGET_CONTAINER}" 2>/dev/null || true
    echo "Old slot [${CURRENT_COLOR}] was left untouched."
    echo "=========================================================================="
    exit 1
fi

# --- Update Caddy reverse proxy routing --------------------------------------
echo "====> Updating Caddy site configuration at ${CADDY_APP_FILE}..."

EXTRA_DOMAINS=$(read_toml_array extra_domains || true)

{
    echo "${DOMAIN} {"
    if [ "${TB_ENABLED}" = "true" ]; then
        echo "    # TrailBase Admin UI and APIs"
        echo "    handle_path /_/* {"
        echo "        reverse_proxy 127.0.0.1:${TB_PORT}"
        echo "    }"
        echo "    handle_path /api/* {"
        echo "        reverse_proxy 127.0.0.1:${TB_PORT}"
        echo "    }"
    fi
    echo ""
    echo "    # Application Slot (${TARGET_COLOR})"
    echo "    reverse_proxy 127.0.0.1:${TARGET_PORT}"
    echo "}"

    for extra in ${EXTRA_DOMAINS}; do
        [ -z "$extra" ] && continue
        echo ""
        echo "http://${extra} {"
        echo "    reverse_proxy 127.0.0.1:${TARGET_PORT}"
        echo "}"
    done
} > "${CADDY_APP_FILE}"

echo "====> Validating Caddy configuration..."
if ! sudo /usr/bin/caddy validate --config /etc/caddy/Caddyfile >/dev/null 2>&1; then
    echo "ERROR: Caddy validation failed. Rolling back Caddy config..."
    if [ "${CURRENT_COLOR}" != "none" ]; then
        CURRENT_PORT="${PORT_BLUE}"
        [ "${CURRENT_COLOR}" = "green" ] && CURRENT_PORT="${PORT_GREEN}"
        sed -i "s/127.0.0.1:${TARGET_PORT}/127.0.0.1:${CURRENT_PORT}/g" "${CADDY_APP_FILE}"
        sudo /usr/bin/systemctl reload caddy
    fi
    podman rm -f "${TARGET_CONTAINER}" 2>/dev/null || true
    exit 1
fi

echo "====> Reloading Caddy..."
sudo /usr/bin/systemctl reload caddy

# --- Record new active slot & tags -------------------------------------------
echo "${TARGET_COLOR}" > "${ACTIVE_SLOT_FILE}"

PREV_TAG_FILE="${APP_ROOT}/previous_tag"
CURRENT_TAG_FILE="${APP_ROOT}/current_tag"

if [ -f "${CURRENT_TAG_FILE}" ]; then
    cp "${CURRENT_TAG_FILE}" "${PREV_TAG_FILE}"
fi
echo "${TAG}" > "${CURRENT_TAG_FILE}"

# --- Gracefully drain and stop old container ---------------------------------
if [ "${CURRENT_COLOR}" != "none" ] && [ "${CURRENT_COLOR}" != "${TARGET_COLOR}" ]; then
    if podman ps --format "{{.Names}}" | grep -q "^${OLD_CONTAINER}$"; then
        echo "====> Draining old slot [${OLD_CONTAINER}] for 5s..."
        sleep 5
        podman stop -t 15 "${OLD_CONTAINER}" 2>/dev/null || true
        podman rm -f "${OLD_CONTAINER}" 2>/dev/null || true
    fi
fi

# Stop any legacy systemd services if they were active
if [ "${CURRENT_COLOR}" = "none" ]; then
    sudo systemctl stop "${APP}-blue" "${APP}-green" 2>/dev/null || true
    sudo systemctl disable "${APP}-blue" "${APP}-green" 2>/dev/null || true
fi

# --- Prune dangling images ---------------------------------------------------
echo "====> Pruning unused container images..."
podman image prune -f >/dev/null 2>&1 || true

echo "=========================================================================="
echo "  SUCCESS: Zero-downtime deployment finished!"
echo "  Active Slot:  ${TARGET_COLOR} (port ${TARGET_PORT})"
echo "  Current Tag:  ${TAG}"
echo "  Rollback Tag: $(cat "${PREV_TAG_FILE}" 2>/dev/null || echo 'none')"
echo "=========================================================================="

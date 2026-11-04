#!/usr/bin/env bash
# ==============================================================================
# VPS Bootstrap Script for Zero-Downtime Blue-Green Micro-PaaS
# Target OS: Ubuntu 22.04 / 24.04 LTS x86_64
# Run as root once per server.
# ==============================================================================
set -euo pipefail

echo "===> [1/6] Updating system packages and installing dependencies..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
    curl \
    ca-certificates \
    nginx \
    certbot \
    python3-certbot-nginx \
    apparmor-utils \
    jq \
    ufw

echo "===> [2/6] Configuring Ubuntu 24.04 AppArmor network socket permissions..."
mkdir -p /etc/apparmor.d/local
cat > /etc/apparmor.d/local/usr.sbin.nginx <<'EOF'
# Allow Nginx reverse proxy connections
network inet stream,
network inet6 stream,
EOF

if [ -f /etc/apparmor.d/usr.sbin.nginx ]; then
    apparmor_parser -r /etc/apparmor.d/usr.sbin.nginx 2>/dev/null || true
fi

echo "===> [3/6] Setting up deploy user and directory hierarchy..."
# Create a dedicated unprivileged deploy user if it doesn't exist
if ! id -u deploy &>/dev/null; then
    useradd -m -s /bin/bash deploy
    usermod -aG www-data deploy
fi

# Set up application, upstreams, and log roots
mkdir -p /opt/apps
mkdir -p /etc/nginx/upstreams
mkdir -p /etc/nginx/sites-available
mkdir -p /etc/nginx/sites-enabled
mkdir -p /var/log/apps

chown -R deploy:www-data /opt/apps
chown -R deploy:www-data /etc/nginx/upstreams
chown -R deploy:www-data /var/log/apps
chmod -R 775 /opt/apps
chmod -R 775 /etc/nginx/upstreams
chmod -R 775 /var/log/apps

echo "===> [4/6] Configuring sudoers permissions for deploy user..."
# The deploy user needs passwordless sudo ONLY for reloading systemd, nginx, and writing service units
cat > /etc/sudoers.d/deploy-micro-paas <<'EOF'
deploy ALL=(ALL) NOPASSWD: /usr/bin/systemctl daemon-reload
deploy ALL=(ALL) NOPASSWD: /bin/systemctl daemon-reload
deploy ALL=(ALL) NOPASSWD: /usr/bin/systemctl reload nginx
deploy ALL=(ALL) NOPASSWD: /bin/systemctl reload nginx
deploy ALL=(ALL) NOPASSWD: /usr/bin/systemctl restart nginx
deploy ALL=(ALL) NOPASSWD: /bin/systemctl restart nginx
deploy ALL=(ALL) NOPASSWD: /usr/sbin/nginx -t
deploy ALL=(ALL) NOPASSWD: /usr/bin/systemctl restart *
deploy ALL=(ALL) NOPASSWD: /bin/systemctl restart *
deploy ALL=(ALL) NOPASSWD: /usr/bin/systemctl stop *
deploy ALL=(ALL) NOPASSWD: /bin/systemctl stop *
deploy ALL=(ALL) NOPASSWD: /usr/bin/systemctl start *
deploy ALL=(ALL) NOPASSWD: /bin/systemctl start *
deploy ALL=(ALL) NOPASSWD: /usr/bin/systemctl status *
deploy ALL=(ALL) NOPASSWD: /bin/systemctl status *
deploy ALL=(ALL) NOPASSWD: /usr/bin/tee /etc/systemd/system/*
deploy ALL=(ALL) NOPASSWD: /usr/bin/cp /tmp/*.conf /etc/nginx/sites-available/
deploy ALL=(ALL) NOPASSWD: /bin/cp /tmp/*.conf /etc/nginx/sites-available/
deploy ALL=(ALL) NOPASSWD: /usr/bin/ln -sfn * /etc/nginx/sites-enabled/*
deploy ALL=(ALL) NOPASSWD: /bin/ln -sfn * /etc/nginx/sites-enabled/*
EOF
chmod 440 /etc/sudoers.d/deploy-micro-paas

echo "===> [5/6] Setting up SSH authorization for deploy user..."
mkdir -p /home/deploy/.ssh
touch /home/deploy/.ssh/authorized_keys
chmod 700 /home/deploy/.ssh
chmod 600 /home/deploy/.ssh/authorized_keys

# If Piku already has a GitHub Actions key, copy the raw public key to deploy user
if [ -f /home/piku/.ssh/authorized_keys ]; then
    grep -o "ssh-[a-z0-9]* [A-Za-z0-9+/=]* [^\"']*" /home/piku/.ssh/authorized_keys | head -n1 >> /home/deploy/.ssh/authorized_keys || true
fi
chown -R deploy:deploy /home/deploy/.ssh

echo "===> [6/6] Verifying firewall rules..."
# Allow web ports if ufw is active (does not force enable ufw)
if ufw status | grep -q "Status: active"; then
    ufw allow 80/tcp || true
    ufw allow 443/tcp || true
fi

echo "=========================================================================="
echo "✅ VPS Bootstrap Complete!"
echo "User: deploy"
echo "Root: /opt/apps"
echo "Upstreams: /etc/nginx/upstreams"
echo "=========================================================================="

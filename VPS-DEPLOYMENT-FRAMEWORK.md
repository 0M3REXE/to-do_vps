# Zero-Downtime VPS Deployment Framework

A portable, Vercel-like deployment framework for small Linux VPS servers (1–6 GB RAM). Designed for both human developers and autonomous AI coding agents. No Docker. No Kubernetes. Just systemd, Nginx, and a deploy script.

---

## Table of contents

1. [What this framework does](#1-what-this-framework-does)
2. [Micro-PaaS Web Dashboard (GUI)](#2-micro-paas-web-dashboard-gui)
3. [Architecture](#3-architecture)
4. [One-time VPS setup](#4-one-time-vps-setup)
5. [SSH key management](#5-ssh-key-management)
6. [How zero-downtime deploys work](#6-how-zero-downtime-deploys-work)
7. [How secrets management works](#7-how-secrets-management-works)
8. [First deployment: the todo app](#8-first-deployment-the-todo-app)
9. [Add another app](#9-add-another-app)
10. [TLS certificates](#10-tls-certificates)
11. [Day-to-day commands](#11-day-to-day-commands)
12. [Rollback](#12-rollback)
13. [Port allocation scheme](#13-port-allocation-scheme)
14. [Capacity planning](#14-capacity-planning)
15. [Security model](#15-security-model)
16. [Troubleshooting](#16-troubleshooting)
17. [Current deployment record](#17-current-deployment-record)
18. [Official references](#18-official-references)

---

## 1. What this framework does

This framework gives you a Vercel-like workflow on a cheap VPS:

| Capability | How it works |
|---|---|
| **Zero-downtime deploys** | Blue-green port swapping with Nginx upstream symlinks. The old process keeps serving until the new one passes health checks. |
| **Instant rollback** | If a new version fails its health check, it is killed immediately. The old version was never touched. Traffic continues uninterrupted. |
| **Git-push CI/CD** | Push to `main` → GitHub Actions builds on a free 16 GB runner → SCP binary to VPS → blue-green switch. The VPS never compiles code. |
| **Web GUI Dashboard** | Built-in web panel at `/admin/` taking only **2.9 MB RAM** to manage apps, edit secrets in a table, trigger slot switches, and generate CI/CD workflows. |
| **Encrypted secrets** | All runtime secrets live in one GitHub repository secret (`APP_ENV`). On deploy, they are piped over encrypted SSH into a `chmod 600` file. Never in shell history, process listings, or git. |
| **Multi-app isolation** | Each app gets its own directory, systemd services, Nginx upstream, and port pair. Apps share the `deploy` user but cannot read each other's secrets. |
| **Tiny footprint** | A compiled Rust binary uses ~8 MB RAM. Nginx uses ~15 MB. You can run 20+ apps on a 1 GB VPS. |

---

## 2. Micro-PaaS Web Dashboard (GUI)

The server includes a purpose-built, ultra-lightweight web dashboard (consumes only **~3 MB of RAM**, zero Docker required) running as a systemd service (`micro-dashboard.service`).

### Accessing the Dashboard

* **URL:** `https://qp90t.aiccloud.online/admin/` (or `https://todorust.duckdns.org/admin/`)
* **Default Username:** `admin`
* **Default Password:** `MicroPaas2026!`

*(To change credentials on the VPS, edit `/opt/micro-dashboard/.credentials` which stores `username:password`, and restart: `systemctl restart micro-dashboard`)*

### What the Dashboard Does

1. **Host & Engine Detection:**
   * Live visual bars for Memory usage (MB and %), Disk storage, and CPU load.
   * Automatically detects if the VPS has the **Zero-Downtime Blue/Green Engine** and/or **Legacy Piku Apps**.
   * Shows UFW firewall status.
2. **App Management:**
   * Lists both **Blue/Green apps** (`/opt/apps/*`) and **Piku apps** (`/home/piku/.piku/apps/*`).
   * Displays the active slot (`Green:4002` or `Blue:4001`) and HTTP health badge (`200 OK`).
   * One-click action buttons:
     * **📜 Logs:** View the latest streaming journalctl or Piku log entries in an in-browser console.
     * **🔐 Secrets:** Open a Vercel-style environment variable editor.
     * **🔄 Restart:** Restart the active process immediately.
     * **🔀 Switch Slot:** Manually test blue-to-green or green-to-blue zero-downtime slot switching.
3. **Secrets & Environment Variable Editor:**
   * Key-value editor table with values masked by default (`••••••••`).
   * Toggle reveal button, add variable, and delete buttons.
   * **"Save & Apply"**: Saves directly to `/opt/apps/<app>/shared/.env` with `chmod 600` (or runs `piku config:set`) and reloads the active service without dropping requests.
4. **+ New App & CI/CD Generator:**
   * Fill in the app name (e.g. `notes`), public domain (e.g. `notes.duckdns.org`), and tech stack (Rust, Go, Node, Python).
   * Automatically picks the next free port pair (e.g. `4003` and `4004`), creates the `/opt/apps/<app>` structure, configures Nginx, and prints the exact copy-pasteable `.github/workflows/deploy.yml` YAML file ready to drop into your repo!

### What this framework does NOT do

- **No Docker**: Apps run as native Linux processes. No container daemon, no image registry, no layer caching.
- **No auto-scaling**: This is a single-server framework. For horizontal scaling, use a managed platform.
- **No built-in log aggregation**: Use `journalctl` directly or ship logs to an external service.
- **No automatic TLS renewal routing**: Certs are managed by `acme.sh` or `certbot`, not by this framework. Renewal cron runs independently.

### Requirements

- A VPS running **Ubuntu 22.04 or 24.04 LTS x86_64** with at least 1 GB RAM.
- A **GitHub repository** with GitHub Actions enabled.
- A **domain name** with DNS pointed at the VPS (A record for its IPv4 address).
- SSH access to the VPS as `root` (for initial setup only).

---

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Developer / AI Agent                         │
│                          git push → main                            │
└──────────────────────────────────┬──────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                   GitHub Actions Runner (16 GB, free)                │
│                                                                     │
│  1. cargo build --release (or go build, npm run build, etc.)        │
│  2. npm ci && npm run css:build (if build-css: true)                │
│  3. SCP binary → deploy@VPS:/opt/apps/<app>/bin/<app>               │
│  4. Pipe APP_ENV secret → deploy@VPS:/opt/apps/<app>/shared/.env    │
│  5. SSH: /opt/apps/<app>/bin/switch.sh <app> <blue> <green>         │
│  6. Verify public HTTPS health check                                │
└──────────────────────────────────┬──────────────────────────────────┘
                                   │ SCP + SSH
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         VPS (1–6 GB RAM)                            │
│                                                                     │
│  /opt/apps/<app>/                                                   │
│  ├── bin/<app>              Compiled binary                         │
│  ├── bin/switch.sh          Blue-green deploy script                │
│  └── shared/                                                        │
│      ├── .env               Runtime secrets (mode 600)              │
│      └── <app>.db           Persistent data (SQLite, etc.)          │
│                                                                     │
│  Systemd                        Nginx                               │
│  ┌──────────────┐               ┌────────────────────────┐          │
│  │ <app>-blue   │  port 4001    │ upstream <app>_backend  │          │
│  │ (inactive)   │───────────────│ → symlink points to    │          │
│  ├──────────────┤               │   active slot's port   │          │
│  │ <app>-green  │  port 4002    │                        │          │
│  │ (active) ✅  │───────────────│ server_name <domain>   │          │
│  └──────────────┘               │ listen 443 ssl         │          │
│                                 └─────────┬──────────────┘          │
│                                           │                         │
│                                     HTTPS │ :443                    │
└───────────────────────────────────────────┼─────────────────────────┘
                                            │
                                       Public Internet
```

### VPS directory layout

```
/opt/apps/
└── <app-name>/
    ├── bin/
    │   ├── <app-name>        # The compiled binary
    │   └── switch.sh         # Blue-green deploy orchestrator
    └── shared/
        ├── .env              # Runtime secrets (chmod 600)
        └── <app-name>.db    # Persistent database file(s)

/etc/nginx/upstreams/
├── <app-name>.conf           # Symlink → active slot conf
├── <app-name>-blue.conf      # upstream <app>_backend { server 127.0.0.1:<blue-port>; }
└── <app-name>-green.conf     # upstream <app>_backend { server 127.0.0.1:<green-port>; }

/etc/systemd/system/
├── <app-name>-blue.service   # Runs binary on blue port
└── <app-name>-green.service  # Runs binary on green port
```

---

## 3. One-time VPS setup

Run this once on a fresh Ubuntu VPS as `root`. It is idempotent and safe to re-run.

### Step 3.1 — Run the bootstrap script

Copy `deploy/bootstrap.sh` to the server and execute it:

```bash
scp -P <SSH_PORT> deploy/bootstrap.sh root@<VPS_IP>:/tmp/bootstrap.sh
ssh -p <SSH_PORT> root@<VPS_IP> "chmod +x /tmp/bootstrap.sh && /tmp/bootstrap.sh"
```

Or on a fresh server with internet access:

```bash
curl -fsSL https://raw.githubusercontent.com/<OWNER>/<REPO>/main/deploy/bootstrap.sh -o /tmp/bootstrap.sh
chmod +x /tmp/bootstrap.sh
/tmp/bootstrap.sh
```

The script performs six steps:
1. Installs Nginx, Certbot, AppArmor tools, jq, and UFW.
2. Patches the **Ubuntu 24.04 AppArmor restriction** that silently blocks `socket(AF_INET)` for processes managed by uWSGI or systemd. Without this, any app process will crash with `Permission denied (os error 13)` when trying to bind a TCP port.
3. Creates a `deploy` user in the `www-data` group with a home directory.
4. Grants `deploy` passwordless `sudo` for **only** `systemctl`, `nginx -t`, and `tee` into `/etc/systemd/system/`. The user cannot sudo into a shell, read other users' files, or modify anything outside the deploy scope.
5. Sets up `~deploy/.ssh/authorized_keys` for key-based access. If a Piku deploy key already exists on the server, it is automatically copied.
6. Configures UFW firewall rules for ports 22, 80, 443, and the active SSH port (if non-standard).

### Step 3.2 — Verify the setup

```bash
# As root, confirm deploy user exists and sudo works
sudo -u deploy sudo -n nginx -t
sudo -u deploy sudo -n systemctl status nginx | head -3
```

Both should succeed without a password prompt.

---

## 4. SSH key management

### Create the deploy key pair

On your local machine (Windows PowerShell or macOS/Linux terminal), generate an Ed25519 key pair without a passphrase:

```powershell
ssh-keygen -t ed25519 -C "github-actions-deploy" -f "$env:USERPROFILE\.ssh\vps-deploy"
```

This creates two files:
- `vps-deploy.pub` — the **public key**. Goes on the VPS.
- `vps-deploy` — the **private key**. Goes only into a GitHub secret. Never paste this into chat, commit it, or send it anywhere else.

### Register the public key on the VPS

Append the public key to the deploy user's authorized keys:

```powershell
$pubkey = Get-Content "$env:USERPROFILE\.ssh\vps-deploy.pub"
ssh -p <SSH_PORT> root@<VPS_IP> "echo '$pubkey' >> /home/deploy/.ssh/authorized_keys"
```

### Pin the VPS host key

Host-key pinning ensures GitHub Actions connects to your real VPS, not an impostor. Fetch the host key and verify its fingerprint in the VPS provider's console:

```powershell
# Fetch the host key entry
ssh-keyscan -p <SSH_PORT> -t ed25519 <VPS_IP> | Out-File -Encoding ascii "$env:USERPROFILE\.ssh\vps-known-hosts"

# Print its fingerprint — compare with the provider console
ssh-keygen -lf "$env:USERPROFILE\.ssh\vps-known-hosts"
```

The content of `vps-known-hosts` becomes the `VPS_KNOWN_HOSTS` secret. Do not trust a `ssh-keyscan` result without the separate fingerprint comparison.

A VPS reinstall changes the host key. Repeat this step and update `VPS_KNOWN_HOSTS` after every reinstall.

### Add GitHub repository secrets

In each app repository, go to **Settings → Secrets and variables → Actions → New repository secret** and add:

| Secret | Value | Example |
|---|---|---|
| `VPS_HOST` | VPS IPv4 address | `51.68.205.225` |
| `VPS_SSH_PORT` | SSH port | `20033` |
| `VPS_SSH_KEY` | Entire contents of the private key file (`vps-deploy`) | `-----BEGIN OPENSSH PRIVATE KEY-----...` |
| `VPS_KNOWN_HOSTS` | Host key line from `vps-known-hosts` | `[51.68.205.225]:20033 ssh-ed25519 AAAA...` |
| `APP_ENV` | All runtime environment variables, one per line | See [Section 6](#6-how-secrets-management-works) |

To display the private key for copying into GitHub:

```powershell
Get-Content -Raw "$env:USERPROFILE\.ssh\vps-deploy"
```

Use **organization-level secrets** if all app repositories belong to the same GitHub organization. `VPS_HOST`, `VPS_SSH_PORT`, `VPS_SSH_KEY`, and `VPS_KNOWN_HOSTS` are the same for every app. Only `APP_ENV` differs per app.

---

## 5. How zero-downtime deploys work

The `deploy/switch.sh` script implements a blue-green port swap. Here is the exact sequence:

```
Time ──────────────────────────────────────────────────────────────────►

         Blue (port 4001)              Green (port 4002)
Step 1:  [serving traffic ✅]          [stopped]
Step 2:  [serving traffic ✅]          [starting new binary...]
Step 3:  [serving traffic ✅]          [healthz → HTTP 200 ✅]
Step 4:  [serving traffic ✅]          ← Nginx upstream symlink flipped
Step 5:  [draining 5s...  ]           [serving traffic ✅]
Step 6:  [stopped]                     [serving traffic ✅]
```

**Key properties:**

- **No dropped requests**: Between steps 3 and 4, both slots are running and healthy. The Nginx `reload` command performs a graceful handoff — existing connections finish on the old upstream, new connections go to the new upstream. No TCP connections are reset.
- **Automatic rollback on failure**: If step 3 fails (the new binary crashes, panics, or returns non-200), the script kills the new slot and exits. The old slot was never touched — it keeps serving traffic as if nothing happened.
- **No state shared between slots**: Each slot runs as an independent systemd service. They share only the `.env` file and the data directory. The binary path is the same (`/opt/apps/<app>/bin/<app>`), so both slots run whatever version is currently on disk.

### How the Nginx upstream symlink works

```
/etc/nginx/upstreams/
├── todo.conf → todo-green.conf     ← Active symlink (what Nginx reads)
├── todo-blue.conf                   ← "upstream todo_backend { server 127.0.0.1:4001; }"
└── todo-green.conf                  ← "upstream todo_backend { server 127.0.0.1:4002; }"
```

The Nginx site config uses `include /etc/nginx/upstreams/todo.conf;` and `proxy_pass http://todo_backend;`. When the switch script flips the symlink and runs `nginx -s reload`, Nginx re-reads the included file and starts sending new connections to the new port. This is an atomic filesystem operation.

---

## 6. How secrets management works

### The problem with manual secrets

Traditional secret management on a VPS means SSH-ing in and typing `export SECRET=value` or editing a file. This is bad because:
- The value appears in **shell history** (`~/.bash_history`)
- The value briefly appears in **`/proc/PID/cmdline`** (visible to all users)
- It requires **manual human intervention** on every deploy
- It is **not reproducible** — an AI agent cannot follow the process

### How this framework handles secrets

All runtime secrets are stored in **one GitHub repository secret** called `APP_ENV`. It is a multi-line string, identical in format to a `.env` file:

```env
SESSION_SECRET=e7bf444ac5a31de4de638f9ce032fbd5834cb782e475e81567647973dc5447fa
DATABASE_URL=file:/opt/apps/todo/shared/todo.db
SESSION_SECURE=true
RUST_LOG=info
APP_ALLOWED_HOST=todorust.duckdns.org,qp90t.aiccloud.online
```

During deployment, the GitHub Actions workflow does this:

```bash
printf '%s\n' "$APP_ENV" | ssh deploy@VPS "cat > /opt/apps/<app>/shared/.env && chmod 600 /opt/apps/<app>/shared/.env"
```

**Security properties:**
- The value is **never logged** — GitHub Actions automatically masks any secret that appears in output.
- The value travels over **encrypted SSH** — it never touches the network in plaintext.
- The file is **`chmod 600`** — only the `deploy` user (and root) can read it.
- The value is **never in shell history** — it is piped via `stdin`, not passed as a command argument.
- The value is **never in `/proc`** — `cat` reads from `stdin`, so the secret does not appear in its `cmdline`.

### To update a secret

1. Go to **GitHub → Settings → Secrets → Actions → `APP_ENV`** → Update.
2. Change or add the variable.
3. Push any commit to `main` (or trigger **Actions → deploy → Run workflow**).
4. The deploy workflow will overwrite `/opt/apps/<app>/shared/.env` with the new value and restart the app via blue-green switch.

### To add a secret for a new app

Create a new repository secret called `APP_ENV` with that app's variables. Each repository has its own `APP_ENV` — apps cannot read each other's secrets.

---

## 7. First deployment: the todo app

The todo app is already deployed on the current VPS using this framework. Here is its configuration for reference:

### Caller workflow (`.github/workflows/deploy.yml`)

```yaml
name: Deploy to Production

on:
  push:
    branches: [main, master]
  workflow_dispatch:

permissions:
  contents: read

jobs:
  deploy:
    uses: ./.github/workflows/deploy-zero-downtime.yml
    with:
      app-name: todo
      binary-name: todo
      domain: todorust.duckdns.org
      port-blue: '4001'
      port-green: '4002'
      health-path: '/healthz'
      build-css: true
    secrets:
      VPS_HOST: ${{ secrets.VPS_HOST }}
      VPS_SSH_PORT: ${{ secrets.VPS_SSH_PORT }}
      VPS_SSH_KEY: ${{ secrets.VPS_SSH_KEY }}
      VPS_KNOWN_HOSTS: ${{ secrets.VPS_KNOWN_HOSTS }}
      APP_ENV: ${{ secrets.APP_ENV }}
```

### App requirements

The todo app binary:
- Runs in the foreground (does not daemonize)
- Reads `BIND_ADDRESS` and `PORT` from the environment
- Serves `GET /healthz` → HTTP 200
- Stores its database in `DATA_ROOT` (set by the systemd service to `/opt/apps/todo/shared`)
- Handles `SIGTERM` gracefully with a 30-second drain deadline

### `APP_ENV` secret contents

```env
DATABASE_URL=file:/opt/apps/todo/shared/todo.db
SESSION_SECURE=true
RUST_LOG=info
APP_ALLOWED_HOST=todorust.duckdns.org,qp90t.aiccloud.online
SESSION_SECRET=<64-char-hex-string>
```

---

## 8. Add another app

Repeat this section for each new app. Each app needs its own repository, app name, port pair, domain, and `APP_ENV` secret.

### Step 8.1 — Make the app compatible

The executable must:
- Run in the **foreground** (do not daemonize).
- Bind to the address in `BIND_ADDRESS` and port in `PORT` from its environment. Use `127.0.0.1` when no bind address is set.
- Serve `GET /healthz` (or another path you configure) with HTTP 200.
- Store persistent data under `DATA_ROOT`, not in the binary's working directory.
- Handle `SIGTERM` for graceful shutdown (drain in-flight requests before exiting).

Use a **unique lowercase `app-name`** for every app. The framework uses this name for directories, systemd services, Nginx upstreams, and the deploy concurrency group.

### Step 8.2 — Pick a port pair

Each app needs two dedicated ports (blue and green). See [Section 12](#12-port-allocation-scheme) for the allocation scheme. Example:

| App | Blue port | Green port |
|---|---|---|
| `todo` | 4001 | 4002 |
| `notes` | 4003 | 4004 |
| `api` | 4005 | 4006 |

### Step 8.3 — Create Nginx site config on the VPS

SSH into the VPS as root and create the Nginx config. Replace `<app>`, `<domain>`, and certificate paths:

```nginx
# /home/piku/.piku/nginx/<app>.conf  (or /etc/nginx/sites-available/<app>.conf)

include /etc/nginx/upstreams/<app>.conf;

server {
    listen 80;
    listen [::]:80;
    server_name <domain>;

    location /.well-known/acme-challenge/ {
        root /var/www/html;
    }

    location / {
        return 301 https://$host$request_uri;
    }
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name <domain>;

    ssl_certificate     /etc/letsencrypt/live/<domain>/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/<domain>/privkey.pem;

    gzip on;
    gzip_proxied any;
    gzip_types text/plain text/xml text/css text/javascript application/javascript application/json image/svg+xml;
    gzip_comp_level 7;
    gzip_min_length 2048;
    gzip_vary on;

    location ~ /\.git { deny all; }

    location / {
        proxy_pass http://<app>_backend;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-Port $server_port;
    }
}
```

Initialize the upstream and enable the site:

```bash
# Create initial upstream (blue port)
echo "upstream <app>_backend { server 127.0.0.1:<blue-port>; }" > /etc/nginx/upstreams/<app>-blue.conf
ln -sfn /etc/nginx/upstreams/<app>-blue.conf /etc/nginx/upstreams/<app>.conf

# Enable site
ln -s /etc/nginx/sites-available/<app>.conf /etc/nginx/sites-enabled/
nginx -t && systemctl reload nginx
```

### Step 8.4 — Obtain a TLS certificate

```bash
certbot --nginx -d <domain> --non-interactive --agree-tos -m your-email@example.com
```

Or if the server already uses acme.sh (as the current VPS does via Piku's installation):

```bash
sudo -iu piku /home/piku/.acme.sh/acme.sh --issue -d <domain> --webroot /var/www/html
```

### Step 8.5 — Add the caller workflow

Create `.github/workflows/deploy.yml` in the new app's repository:

```yaml
name: Deploy to Production

on:
  push:
    branches: [main, master]
  workflow_dispatch:

permissions:
  contents: read

jobs:
  deploy:
    uses: <OWNER>/<WORKFLOW-REPO>/.github/workflows/deploy-zero-downtime.yml@main
    with:
      app-name: <app>
      binary-name: <app>
      domain: <domain>
      port-blue: '<blue-port>'
      port-green: '<green-port>'
      health-path: '/healthz'
      build-css: false
    secrets:
      VPS_HOST: ${{ secrets.VPS_HOST }}
      VPS_SSH_PORT: ${{ secrets.VPS_SSH_PORT }}
      VPS_SSH_KEY: ${{ secrets.VPS_SSH_KEY }}
      VPS_KNOWN_HOSTS: ${{ secrets.VPS_KNOWN_HOSTS }}
      APP_ENV: ${{ secrets.APP_ENV }}
```

If the reusable workflow lives in a private repository, allow the calling repository in the reusable workflow repository's **Settings → Actions → General → Access**.

### Step 8.6 — Add GitHub secrets and deploy

Add the same four VPS secrets to the new repository (or use organization secrets). Add the app-specific `APP_ENV` secret. Push to `main` or trigger **Actions → deploy → Run workflow**.

On the first push, the switch script creates both systemd services, starts the blue slot, passes the health check, sets up the Nginx upstream symlink, and begins serving traffic.

---

## 9. TLS certificates

### Current setup (acme.sh via Piku)

The current VPS uses Piku's built-in acme.sh integration. Certificates are stored at `/home/piku/.piku/nginx/<app>.crt` and `.key`. The Nginx config references these paths. The acme.sh cron job at `/home/piku/.acme.sh` handles automatic renewal.

### For new apps (certbot)

The bootstrap script installs `certbot` with the Nginx plugin. Use it to issue and auto-renew certificates:

```bash
certbot --nginx -d <domain>
```

Certbot installs a systemd timer (`certbot.timer`) that checks for renewal twice daily. No manual cron setup needed.

### Certificate file paths

| Method | Cert path | Key path |
|---|---|---|
| acme.sh (Piku) | `/home/piku/.piku/nginx/<app>.crt` | `/home/piku/.piku/nginx/<app>.key` |
| certbot | `/etc/letsencrypt/live/<domain>/fullchain.pem` | `/etc/letsencrypt/live/<domain>/privkey.pem` |

Use the correct path in the Nginx site config. Both methods auto-renew.

---

## 10. Day-to-day commands

All commands run over SSH. Use the deploy key for routine operations or root for administrative tasks.

### View app logs

```bash
# Follow live logs for the active slot
ssh -p <PORT> deploy@<VPS> sudo journalctl -u todo-green -f

# Last 100 lines
ssh -p <PORT> deploy@<VPS> sudo journalctl -u todo-green -n 100 --no-pager

# Logs for the blue slot (if running)
ssh -p <PORT> deploy@<VPS> sudo journalctl -u todo-blue -n 50 --no-pager
```

### Check app status

```bash
# Which slot is active?
ssh -p <PORT> deploy@<VPS> "readlink /etc/nginx/upstreams/todo.conf"

# Process status
ssh -p <PORT> deploy@<VPS> sudo systemctl status todo-green

# Health check
ssh -p <PORT> deploy@<VPS> "curl -s http://127.0.0.1:4002/healthz"
```

### Restart the app (without redeploying)

```bash
ssh -p <PORT> deploy@<VPS> sudo systemctl restart todo-green
```

### View current secrets (names only)

```bash
ssh -p <PORT> deploy@<VPS> "cut -d= -f1 /opt/apps/todo/shared/.env"
```

Do not pipe the full `.env` contents to your terminal or into logs.

### List all deployed apps

```bash
ssh -p <PORT> deploy@<VPS> "ls /opt/apps/"
```

---

## 11. Rollback

### Automatic rollback (built-in)

If a new binary fails its health check during the blue-green switch, the switch script:
1. Stops the failed slot
2. Exits with code 1
3. The old slot was **never stopped** — it keeps serving

The GitHub Actions job will fail, and the deployment is effectively rolled back automatically.

### Manual rollback to a previous binary

If a deployment succeeded but the new version has a bug discovered later:

1. Find the last working commit SHA from **GitHub → Actions → deploy** history.
2. Trigger the workflow on that commit: **Actions → deploy → Run workflow → Use workflow from: `<branch-or-tag>`**.

Or directly on the VPS:

```bash
# If you kept a backup of the old binary:
cp /opt/apps/todo/bin/todo.backup /opt/apps/todo/bin/todo

# Trigger a blue-green switch to restart with the old binary
/opt/apps/todo/bin/switch.sh todo 4001 4002 /healthz todorust.duckdns.org
```

### Pre-deploy binary backup (optional improvement)

You can modify `switch.sh` to automatically back up the current binary before replacing it:

```bash
cp -p /opt/apps/$APP_NAME/bin/$APP_NAME /opt/apps/$APP_NAME/bin/$APP_NAME.backup
```

---

## 12. Port allocation scheme

Each app needs two sequential ports: one for blue, one for green. Ports must not collide between apps.

**Convention**: Start at `4001` and allocate in pairs:

| App | Blue | Green |
|---|---|---|
| `todo` | 4001 | 4002 |
| `notes` | 4003 | 4004 |
| `api` | 4005 | 4006 |
| `blog` | 4007 | 4008 |

These are local-only ports (bound to `127.0.0.1`). They are not exposed to the internet. Only Nginx connects to them.

To see which ports are in use:

```bash
ss -tlnp | grep -E '400[0-9]'
```

---

## 13. Capacity planning

### Memory budget

| Component | RAM usage |
|---|---|
| Linux kernel + systemd | ~60 MB |
| Nginx | ~15 MB |
| Each Rust/Go binary | ~8–30 MB |
| SQLite (embedded) | ~0 MB extra |

On a **1 GB VPS**, you can comfortably run 10–15 lightweight apps with room for OS buffers. On a **2 GB VPS**, 30+ apps.

### Disk budget

| Component | Disk usage |
|---|---|
| Ubuntu base | ~1.5 GB |
| Each compiled binary | ~5–15 MB |
| Each SQLite database | varies |
| Nginx, certbot, logs | ~100 MB |

### CPU

Compilation happens on GitHub Actions (free, 4-core runner). The VPS only runs pre-compiled binaries. A single-core VPS handles thousands of requests per second for typical web apps.

---

## 14. Security model

### Access boundaries

| Actor | Can do | Cannot do |
|---|---|---|
| `deploy` user | Start/stop/restart systemd services, reload nginx, write to `/opt/apps/`, write systemd unit files | Open a root shell, read other users' home dirs, modify firewall |
| GitHub Actions deploy key | SSH as `deploy`, SCP files to `/opt/apps/` | SSH as root, forward ports |
| App process | Read its own `.env` and `shared/` dir, bind its assigned port | Read other apps' `.env` files (chmod 600), bind to 0.0.0.0 |

### Secret lifecycle

1. **Created**: Developer generates secret locally (e.g., `openssl rand -hex 32`).
2. **Stored**: Pasted into GitHub repository secret `APP_ENV`.
3. **Transmitted**: Piped over SSH during deploy. Never in shell history or logs.
4. **At rest**: Written to `/opt/apps/<app>/shared/.env` with mode `600`.
5. **Consumed**: Read by systemd `EnvironmentFile=` directive. Passed as environment variables to the process.
6. **Rotated**: Update `APP_ENV` in GitHub, push or trigger deploy.

### Hardening recommendations

- **Disable password authentication** for SSH. Use key-only auth.
- **Change the default SSH port** from 22 to a high port (already done: port 20033).
- **Enable UFW** and allow only ports 80, 443, and your SSH port.
- **Do not commit `.env` files** to git. The `.gitignore` should exclude them.
- **Rotate `SESSION_SECRET`** periodically. Changing it invalidates all active user sessions (users must log in again).

---

## 15. Troubleshooting

| Symptom | Check or fix |
|---|---|
| `Permission denied (os error 13)` when binding port | The AppArmor fix was not applied. Re-run `bootstrap.sh` or manually: `mkdir -p /etc/apparmor.d/local && echo -e "network inet stream,\nnetwork inet6 stream," > /etc/apparmor.d/local/usr.sbin.nginx && apparmor_parser -r /etc/apparmor.d/usr.sbin.nginx` |
| `SESSION_SECRET must be set when SESSION_SECURE=true` | `APP_ENV` secret is missing `SESSION_SECRET`, or the `.env` file was not written. Check: `cat /opt/apps/todo/shared/.env` |
| Health check returns `000` (connection refused) | The binary is not starting. Check: `sudo journalctl -u <app>-<color> -n 50 --no-pager` |
| 502 Bad Gateway from Nginx | The upstream port doesn't match the running slot. Check: `readlink /etc/nginx/upstreams/<app>.conf` and compare with `ss -tlnp | grep <port>` |
| 403 from Nginx on a provider proxy URL | Add the exact hostname to the Nginx `server_name` directive and to `APP_ALLOWED_HOST` in `APP_ENV`. |
| HTTPS certificate error | Confirm DNS points to the VPS. Run `certbot renew --dry-run` to test renewal. Check cert paths in the Nginx config match the actual cert files. |
| SSH host-key error after VPS reinstall | Re-run the host key pinning steps in [Section 4](#4-ssh-key-management) and update `VPS_KNOWN_HOSTS`. |
| Deploy succeeds but site looks unchanged | Open the latest Actions run and check every step passed. The binary may have been compiled from a stale branch. Check: `ssh deploy@VPS "/opt/apps/<app>/bin/<app> --version"` if your app supports it. |
| App starts but immediately exits | Check `journalctl` for the error. Common causes: missing env var, database migration failure, port already in use by the other slot. |
| `ETXTBSY` error during deploy | The binary was overwritten while the old process was still running it. The workflow now uses atomic `mv` to avoid this — ensure `deploy-zero-downtime.yml` uses the `.new` → `mv` pattern. |

---

## 16. Current deployment record

### VPS details

| Item | Value |
|---|---|
| Provider | AICCloud |
| OS | Ubuntu 24.04 LTS x86_64 |
| RAM | 1 GB |
| Disk | 10 GB |
| IPv4 | `51.68.205.225` |
| SSH port | `20033` |
| Public URL | `https://qp90t.aiccloud.online` |
| DuckDNS | `https://todorust.duckdns.org` |

### Todo app deployment

| Item | Value |
|---|---|
| App name | `todo` |
| Blue port | `4001` |
| Green port | `4002` |
| Active slot | `green` (port 4002) |
| Binary | `/opt/apps/todo/bin/todo` |
| Database | `/opt/apps/todo/shared/todo.db` |
| Secrets | `/opt/apps/todo/shared/.env` (mode 600) |
| Nginx conf | `/home/piku/.piku/nginx/todo.conf` (includes dynamic upstream) |
| TLS cert | `/home/piku/.piku/nginx/todo.crt` (via acme.sh, auto-renews) |
| Upstream symlink | `/etc/nginx/upstreams/todo.conf → todo-green.conf` |
| Systemd (active) | `todo-green.service` (enabled on boot) |
| Systemd (standby) | `todo-blue.service` (inactive) |
| Health check | `GET /healthz` → HTTP 200 |

### Migration notes

The VPS was originally running Piku (uWSGI-based deployment). It has been migrated to this framework. Piku's uWSGI emperor (`uwsgi-piku.service`) is stopped and disabled but not removed. The Piku user account, acme.sh installation, and TLS certificates are still in place and used by this framework for the existing todo app.

---

## 17. Official references

- [Nginx reverse proxy](https://docs.nginx.com/nginx/admin-guide/web-server/reverse-proxy/)
- [systemd service units](https://www.freedesktop.org/software/systemd/man/systemd.service.html)
- [Certbot (Let's Encrypt)](https://certbot.eff.org/)
- [acme.sh](https://github.com/acmesh-official/acme.sh)
- [GitHub Actions encrypted secrets](https://docs.github.com/en/actions/security-for-github-actions/security-guides/using-secrets-in-github-actions)
- [GitHub Actions reusable workflows](https://docs.github.com/en/actions/sharing-automations/reusing-workflows)
- [Blue-green deployment pattern](https://martinfowler.com/bliki/BlueGreenDeployment.html)

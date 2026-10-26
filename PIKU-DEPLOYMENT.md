# Piku deployment runbook

This is the complete setup and operating guide for deploying this To-do app and similar small Rust web apps to a VPS with Piku and GitHub Actions. Follow **One-time VPS setup** once per server. Follow **Add another app** once per app repository.

## What this setup does

Piku is a small self-hosted app platform. It accepts a restricted SSH connection and starts apps described by two files: `Procfile` (what process to run) and `ENV` (runtime settings). In this setup:

- GitHub Actions builds the Rust executable and optional CSS on GitHub's runner.
- Actions pushes only the executable, `Procfile`, and `ENV` to Piku. The VPS does not need Rust, Cargo, source files, or build disk space.
- Piku's Nginx receives web requests and forwards them to the app's private `PORT`.
- Each app stores its embedded libSQL database in its own Piku data directory. This is a database file, not a separate Turso service.

A 1 GB RAM VPS is suitable for a few small, low-traffic apps because compilation happens on GitHub. Piku apps share the `piku` Unix account, so this is practical separation for your own apps, not a strong security boundary for mutually untrusted apps.

## One-time VPS setup

### 1. Prepare the server and hostname

Use a fresh Ubuntu 24.04 x86_64 VPS. Keep SSH open on your chosen port. In the provider firewall, allow inbound SSH on that port and TCP ports 80 and 443; Piku uses port 80 for certificate setup and Nginx serves HTTPS on 443. If UFW is enabled inside Ubuntu, allow the same ports there too.

Choose how people will reach the server:

- **A domain pointed directly at the VPS:** create an A record for its IPv4 address. Add an AAAA record only if the VPS's IPv6 address is reachable. Piku can request HTTPS certificates for the hostnames it serves.
- **A provider-generated public URL:** add that hostname to the app's `additional-domains` setting below. The provider must forward that hostname to the VPS's Nginx. A hostname that points to a provider proxy is not automatically routed to the correct app by Piku until it is listed there.

For the current AICCloud VPS, the provider URL is `qp90t.aiccloud.online`. The To-do app serves it as an additional hostname. At the last check, direct IPv4 access to the DuckDNS hostname hit a Caddy TLS failure, so use the AICCloud URL as the verified public endpoint.

### 2. Install Piku

Sign in to the fresh server as `root` using the VPS provider's console or SSH. These commands install Piku's required server packages and services. Review the official bootstrap before running it; use it only on a fresh server.

```sh
apt-get update
apt-get install -y curl ca-certificates
curl -fsSL https://raw.githubusercontent.com/piku/piku-bootstrap/master/piku-bootstrap -o /root/piku-bootstrap
chmod 755 /root/piku-bootstrap
/root/piku-bootstrap first-run --no-interactive
/root/piku-bootstrap install
```

Piku creates the `piku` Unix account and installs Python, Git, Nginx, uWSGI, and certificate tooling. It does not install Rust or compile this app. Check that the services are running:

```sh
systemctl is-active nginx uwsgi-piku
```

Both should print `active`.

### 3. Apply the Ubuntu 24.04 AppArmor compatibility rule

On the Ubuntu 24.04 image used for this deployment, Piku's uWSGI worker could not create an IPv4 socket until its AppArmor profile had explicit network rules. Apply this once per server:

```sh
cat >/etc/apparmor.d/local/uwsgi-core <<'EOF'
network inet stream,
network inet6 stream,
EOF
apparmor_parser -r /etc/apparmor.d/uwsgi-core
systemctl restart uwsgi-piku
```

If a future Ubuntu image does not have `/etc/apparmor.d/uwsgi-core`, check the server's AppArmor profile and skip this image-specific workaround rather than creating a profile blindly.

### 4. Create the GitHub Actions SSH key

The deploy key lets GitHub Actions push releases to Piku. Piku restricts this key to Piku commands; do not use this key for root access.

On Windows PowerShell, create a separate Ed25519 key. When asked for a passphrase, press Enter twice; this workflow does not configure an SSH agent to unlock a passphrase-protected key.

```powershell
ssh-keygen -t ed25519 -C "github-actions-piku" -f "$env:USERPROFILE\.ssh\piku-deploy"
```

The `.pub` file is the **public key** and goes on the VPS. The file without `.pub` is the **private key** and goes only into a GitHub secret. Never paste the private key into chat or commit either key to a repository.

### 5. Pin the VPS SSH host key

Host-key pinning lets Actions verify it connected to your VPS, not an impostor. For a new server, first read the Ed25519 host-key fingerprint in the VPS provider's web console:

```sh
ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
```

On Windows PowerShell, fetch the public host-key entry using the VPS IP and SSH port, then compare its fingerprint with the provider-console fingerprint:

```powershell
ssh-keyscan -p 20033 -t ed25519 51.68.205.225 | Out-File -Encoding ascii "$env:USERPROFILE\.ssh\piku-known-hosts"
ssh-keygen -lf "$env:USERPROFILE\.ssh\piku-known-hosts"
```

Replace the sample IP and port for a different VPS. Continue only when the fingerprints match. Copy the complete line from `piku-known-hosts`; that line becomes the `VPS_KNOWN_HOSTS` secret. Do not trust a `ssh-keyscan` result without the separate fingerprint comparison.

For the current server, the verified entry is:

```text
[51.68.205.225]:20033 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOjVunBYF2jmn789xO4ZNwrxgJlhHIDw8xoToiSFk6eu
```

A reinstall changes the server host key. Repeat this check and update `VPS_KNOWN_HOSTS` after every reinstall.

### 6. Register the public deploy key with Piku

Copy the **public** key to the VPS. This example uses the current server; replace its address and port for another server. The known-hosts file here is the one verified in the previous step.

```powershell
scp -P 20033 -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$env:USERPROFILE\.ssh\piku-known-hosts" "$env:USERPROFILE\.ssh\piku-deploy.pub" root@51.68.205.225:/tmp/piku-deploy.pub
ssh -p 20033 -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$env:USERPROFILE\.ssh\piku-known-hosts" root@51.68.205.225
```

At the VPS root prompt, register it with Piku:

```sh
sudo -iu piku python3 /home/piku/piku.py setup:ssh /tmp/piku-deploy.pub
```

Piku prints a key fingerprint. Check that it matches the public key you generated. The deploy key is forced through Piku's command interface and cannot forward ports. It is not a root key, but it can use Piku's app commands under the shared `piku` account, so keep it private. Keep the VPS root password private; Actions does not use it.

### 7. Add GitHub repository secrets

In each app repository, open **Settings → Secrets and variables → Actions → New repository secret**. Add:

| Secret | Value |
| --- | --- |
| `VPS_HOST` | VPS address or SSH hostname; current value: `51.68.205.225` |
| `VPS_SSH_PORT` | VPS SSH port; current value: `20033` |
| `VPS_SSH_KEY` | Entire contents of `piku-deploy`, the private key without `.pub` |
| `VPS_KNOWN_HOSTS` | Entire verified host-key line from step 5 |

To display the private key locally for copying into the GitHub secret, run this in PowerShell. Paste the output directly into GitHub; do not send it here:

```powershell
Get-Content -Raw "$env:USERPROFILE\.ssh\piku-deploy"
```

There is no `VPS_USER` secret: the workflow always connects as Piku's restricted `piku` user. Use organization secrets if all app repositories belong to the same GitHub organization; otherwise add these four secrets to each repository.

## First deployment: this To-do app

The caller workflow is `.github/workflows/deploy.yml`. It runs for pushes to `main` or `master`, and can also be started from the repository's **Actions** tab using **Run workflow**. Its current settings are:

- Piku app name: `todo`
- Rust binary name: `todo` (from `Cargo.toml`)
- Main hostname: `todorust.duckdns.org`
- Extra provider hostname: `qp90t.aiccloud.online`
- CSS build: enabled

The reusable workflow `.github/workflows/piku-rust-deploy.yml` builds CSS and the Linux executable on GitHub, creates a temporary artifact-only Git repository, and pushes it to `piku@VPS_HOST:todo`. It supplies the main and additional hostnames to both Piku's virtual-host routing and the app's trusted-origin list, so requests work through a proxy that preserves the public hostname or rewrites it. Piku creates the app on its first push. It stores the database at `~/.piku/data/todo/todo.db`; deploying a new binary does not delete this data directory.

The app must listen on the `BIND_ADDRESS` and `PORT` environment variables and provide `GET /healthz` with an HTTP 200 response. Piku sets these values for each web app. The workflow uses Piku's restricted SSH command to check the app's local HTTPS route after deployment. That check verifies the app and Piku's Nginx TLS route on the VPS; it does not test every external provider proxy or DNS path.

A successful push to GitHub's deployment branch automatically builds and deploys the app. No Rust compiler or build step runs on the VPS.

## Add another Rust app

Repeat this section for each app. Each app needs its own repository, app name, binary name, hostname, caller workflow, and database file.

### 1. Make the app compatible with Piku

The executable must:

- Run in the foreground (do not daemonize itself).
- Bind to `BIND_ADDRESS` and `PORT` from its environment. Use `127.0.0.1` when no bind address is set.
- Serve `GET /healthz` with HTTP 200.
- Store persistent data under Piku's `DATA_ROOT`, not inside the temporary app release directory.

Use a unique lowercase `app-name` for every app. The reusable workflow uses that name for Piku's app, its data filename, and its deploy concurrency group.

### 2. Add a caller workflow to the new repository

Create `.github/workflows/deploy.yml` in the app repository. Replace the sample app name, binary, and hostname. Set `build-css: true` only if the repository has `package-lock.json` and the scripts `npm ci` and `npm run css:build` work there.

```yaml
name: deploy

on:
  push:
    branches: [main, master]
  workflow_dispatch:

permissions:
  contents: read

jobs:
  deploy:
    uses: 0M3REXE/to-do_vps/.github/workflows/piku-rust-deploy.yml@main
    with:
      app-name: notes
      binary-name: notes
      domain: notes.example.com
      additional-domains: ''
      build-css: false
    secrets:
      VPS_HOST: ${{ secrets.VPS_HOST }}
      VPS_SSH_PORT: ${{ secrets.VPS_SSH_PORT }}
      VPS_SSH_KEY: ${{ secrets.VPS_SSH_KEY }}
      VPS_KNOWN_HOSTS: ${{ secrets.VPS_KNOWN_HOSTS }}
```

For a hostname supplied by your VPS provider, put it in `additional-domains` instead of leaving that value empty. Use a distinct hostname for every app; one hostname cannot route to multiple apps unless the proxy is configured for path-based routing.

If the reusable workflow repository is private, allow the calling repository in the reusable workflow repository's **Settings → Actions → General → Access**. The workflow reference above assumes the reusable workflow is on branch `main`.

### 3. Add the VPS secrets and deploy

Add the same four repository secrets from step 7 to the new repository. Push to `main` or `master`, or use **Actions → deploy → Run workflow**. On the first push, Piku creates that app, configures its hostname, and starts its `web` worker. Subsequent pushes replace the executable while keeping its data directory.

For CSS-building apps, the workflow runs `npm ci` and `npm run css:build` before compiling. Askama templates are compiled into the binary; app assets must be generated or committed so the GitHub build can include them.

### Runtime secrets

Never put credentials in source code, the committed `ENV` file, or workflow logs. Set app-specific runtime values with Piku's `config:set` command; remote Piku configuration overrides values shipped in `ENV` and persists across app pushes. For example, from an authorized Piku terminal:

```sh
python3 /home/piku/piku.py config:set notes API_TOKEN=<secret-value>
python3 /home/piku/piku.py config:live notes
```

Piku keeps runtime configuration in files readable by the `piku` account and root. `config:live` prints those values; do not paste its output into public logs or chat. Piku is suitable for apps you trust and administer; do not use the shared account as isolation for hostile tenants.

## Day-to-day commands

Piku commands are run over SSH as `piku`. The deploy key can run Piku commands but cannot open a shell or forward ports. In PowerShell, set the connection options once, then run a Piku command:

```powershell
$PikuSsh = @('-p', '20033', '-i', "$env:USERPROFILE\.ssh\piku-deploy", '-o', 'IdentitiesOnly=yes', '-o', 'StrictHostKeyChecking=yes', '-o', "UserKnownHostsFile=$env:USERPROFILE\.ssh\piku-known-hosts")
ssh @PikuSsh piku@51.68.205.225 apps
ssh @PikuSsh piku@51.68.205.225 ps todo
ssh @PikuSsh piku@51.68.205.225 config:live todo
ssh @PikuSsh piku@51.68.205.225 restart todo
ssh @PikuSsh piku@51.68.205.225 logs todo web
```

`logs` streams output; press Ctrl+C to stop following it. Replace `todo` with an app's Piku name. `config:set` updates runtime settings. `config:live` prints current settings, including secrets, so treat its output as sensitive. `destroy` removes an app deployment but preserves its data directory. Back up `~/.piku/data/` off the VPS; the VPS disk is not a backup.

### Sign the To-do session cookie

The app stores session records on the server and gives the browser a random session ID. In production, it also signs that cookie using `SESSION_SECRET`. Signing prevents a client from tampering with the cookie; it does not make a stolen cookie safe to replay. Keep the secret in Piku's runtime configuration, never in the repository or deployment artifact.

Run this once **before deploying the session-cookie hardening change** in PowerShell after defining `$PikuSsh` above. It generates 64 random bytes locally, sends the value directly to Piku's `config:set`, then removes the local variables:

```powershell
$SessionSecretBytes = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Fill($SessionSecretBytes)
$SessionSecret = [BitConverter]::ToString($SessionSecretBytes).Replace("-", "").ToLowerInvariant()
ssh @PikuSsh piku@51.68.205.225 "config:set todo SESSION_SECRET=$SessionSecret"
Remove-Variable SessionSecret, SessionSecretBytes
```

Keep the same key across deployments. If it is changed or lost, users will need to sign in again. The `__Host-session` cookie name also takes effect on the next deployment, so users will sign in once after that release. If the key is missing, the app exits with an error when `SESSION_SECURE=true` and does not start. The cookie is `Secure`, `HttpOnly`, `SameSite=Lax`, host-only, and limited to the root path. Sessions expire after 60 minutes of inactivity and at most 7 days after creation. A second `__Host-device` cookie is also required; losing it forces a password confirmation. These measures reduce common cookie theft and replay risks, but cannot protect a browser already compromised by malware or an attacker who has copied an active cookie and device cookie with the same User-Agent.

Give each app its own randomly generated `SESSION_SECRET`.

## Troubleshooting

| Symptom | Check or fix |
| --- | --- |
| `socket(AF_INET): Permission denied` in app logs | Apply the AppArmor rule in step 3 and restart `uwsgi-piku`. |
| 403 from Nginx on a provider URL | Add the exact requested hostname to that app's `additional-domains`, then redeploy. The request reached Nginx, but Piku did not have a matching virtual host. |
| 502 from Nginx | Run `ps APP_NAME` and `logs APP_NAME web`. Confirm the process starts, binds `BIND_ADDRESS` and `PORT`, and stays in the foreground. |
| HTTPS/certificate error | Confirm DNS points to the expected public route, ports 80/443 reach the VPS or provider proxy, and the hostname is listed in `domain` or `additional-domains`. |
| SSH host-key error after a VPS reinstall | Repeat step 5, compare the new fingerprint in the provider console, and update `VPS_KNOWN_HOSTS`. |
| New push succeeds but the site looks unchanged | Open the latest Actions run and make sure the reusable workflow's build, Piku push, and health-check steps all succeeded. |

## Current deployment record

The To-do deployment is live at [qp90t.aiccloud.online](https://qp90t.aiccloud.online/) through AICCloud's proxy. The latest verified deployment is [Actions run 40210240728](https://github.com/0M3REXE/to-do_vps/actions/runs/40210240728), which successfully built on GitHub, pushed the artifact to Piku, and passed the health check. The public page and `/healthz` both returned HTTP 200. The DuckDNS hostname is an additional Piku host; its direct IPv4 route is not the verified public route for this AICCloud VPS.

## Official references

- [Piku project](https://github.com/piku/piku)
- [Piku installation guide](https://piku.github.io/install/)
- [Piku bootstrap](https://github.com/piku/piku-bootstrap)
- [Procfile and web workers](https://piku.github.io/configuration/procfile.html)
- [Environment variables](https://piku.github.io/configuration/env.html)
- [Management commands](https://piku.github.io/manage.html)

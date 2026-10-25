# Piku deployment for To-do

GitHub Actions compiles the Rust executable and CSS on a GitHub-hosted Ubuntu 24.04 runner. It then pushes a small package containing only the executable, `Procfile`, and `ENV` to Piku. The VPS does not install Rust or compile the app.

## Fresh VPS setup

Use a fresh Ubuntu 24.04 VPS. Point `todorust.duckdns.org` to its public IPv4 address. Publish an AAAA record only if the VPS IPv6 address is correct and reachable. Allow inbound SSH on the configured port and TCP ports 80 and 443. Port 80 is needed for HTTP-01 certificate issuance.

The official Piku bootstrap changes system packages and services, so review its playbook and run it only on a fresh server. We used the official bootstrap on this Ubuntu host; it installed the `piku` Unix account, Python, Git, Nginx, uWSGI, and `acme.sh`. It does not install Rust or compile the app. Commands:

```sh
apt-get update
apt-get install -y curl ca-certificates
curl -fsSL https://raw.githubusercontent.com/piku/piku-bootstrap/master/piku-bootstrap -o /root/piku-bootstrap
chmod 755 /root/piku-bootstrap
/root/piku-bootstrap first-run --no-interactive
/root/piku-bootstrap install
```

See the [Piku bootstrap repository](https://github.com/piku/piku-bootstrap) and [Piku install guide](https://piku.github.io/install/).

On this Ubuntu 24.04 image, add explicit network permission to the `uwsgi-core` AppArmor profile before deploying. Without it, the profile's `unconfined` label can lose its network allowance after user-namespace stacking, and Piku's app worker fails with `socket(AF_INET): Permission denied`.

```sh
cat >/etc/apparmor.d/local/uwsgi-core <<'EOF'
network inet stream,
network inet6 stream,
EOF
apparmor_parser -r /etc/apparmor.d/uwsgi-core
systemctl restart uwsgi-piku
```

Register the GitHub Actions deploy public key with Piku (not root):

```sh
sudo -iu piku
python3 ~/piku.py setup:ssh /path/to/github-actions-public-key.pub
```

Piku's `setup:ssh` restricts the key to Piku commands. Set the server DNS records before the first app push so Piku can obtain a Let's Encrypt certificate.

## GitHub Actions secrets

Set these repository secrets under **Settings → Secrets and variables → Actions**:

| Secret | Value |
| --- | --- |
| `VPS_HOST` | VPS public IPv4 address or SSH hostname |
| `VPS_SSH_PORT` | SSH port number |
| `VPS_SSH_KEY` | Full contents of the deploy private key (never the `.pub` file) |
| `VPS_KNOWN_HOSTS` | The pinned SSH host-key entry, verified from the VPS provider console |

The server reinstall changed its host key. For this server, the verified `VPS_KNOWN_HOSTS` value is:

```text
[51.68.205.225]:20033 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOjVunBYF2jmn789xO4ZNwrxgJlhHIDw8xoToiSFk6eu
```

`VPS_USER` is not used: the workflow connects as Piku's restricted `piku` user. Keep deploy keys and app credentials in GitHub Secrets or Piku's remote `config:set`, never in Git.

## Deploy To-do

The caller workflow `.github/workflows/deploy.yml` runs on pushes to `main` or `master`, and can also be started manually from the Actions tab. It calls `.github/workflows/piku-rust-deploy.yml`, which:

1. Builds Tailwind CSS and compiles the release executable on GitHub's Ubuntu runner.
2. Creates a temporary artifact-only Git repository; it does not include Rust source or Cargo files, so Piku uses its generic executable worker and does not invoke Cargo.
3. Pushes the package to `piku@VPS_HOST:todo` over the pinned SSH connection.
4. Uses Piku's restricted SSH command to check the app's HTTPS route through loopback on the VPS.
5. Piku stores the database at `~/.piku/data/todo/todo.db`, listens on its assigned private `PORT`, and routes `todorust.duckdns.org` to the app.

The check validates the app and Piku's local TLS route. It does not verify the external IPv4 route. During this deployment, IPv6 reached this VPS's Nginx/Piku stack, while IPv4 port 80 answered with a separate `Server: Caddy` endpoint and IPv4 TLS failed. IPv4 web access needs the VPS provider's port 80/443 forwarding corrected; the VPS itself has no Caddy process. The app returned HTTP 200 over HTTPS on IPv6 after applying the AppArmor rule above.

Each Piku app gets its own data directory and a separate SQLite-compatible libSQL database file. This is embedded libSQL, not a separate Turso server. Piku runs apps under its `piku` Unix account, so per-app data paths separate normal app data but are not a strong security sandbox between mutually untrusted apps.

## Add another Rust app

In an app repository, add a caller workflow modeled on `.github/workflows/deploy.yml` and point it to the reusable workflow. For an app in this repository, use `./.github/workflows/piku-rust-deploy.yml`; for a separate repository, use `OWNER/REPO/.github/workflows/piku-rust-deploy.yml@main` and allow access to that workflow if the repository is private.

Set distinct `app-name` and `domain` values. The Rust binary name must match a Cargo binary. Set `build-css: true` only when the app uses `npm ci` and `npm run css:build`. Add the same four VPS secrets to each app repository or use organization-level secrets.

To add app-specific secrets after the app's first push, use Piku's `config:set` command. Do not commit secrets in the app's `ENV`; remote config overrides app-shipped defaults.

## Operations

Piku provides `logs`, `ps`, `restart`, `config`, and `destroy` commands. `destroy` intentionally retains app data. Back up `~/.piku/data/` off-server regularly; a VPS disk is not a backup.

Piku references: [GitHub project](https://github.com/piku/piku), [Procfile](https://piku.github.io/configuration/procfile.html), [environment settings](https://piku.github.io/configuration/index.html), and [management commands](https://piku.github.io/manage.html).

## Deployment record

The first GitHub Actions build and artifact-only Piku push succeeded. The app is running as Piku's `web` process, and its `/healthz` returns HTTP 200 over local HTTPS and public IPv6. The initial public IPv4 health check failed because that route reaches a separate Caddy endpoint. The workflow now checks the app's local HTTPS route through Piku's restricted SSH command; rerun it after pushing this change to confirm CI is green.

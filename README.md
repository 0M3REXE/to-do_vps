# To-do

A small, server-rendered todo app built with Rust, Axum, Askama, HTMX, and libSQL. Accounts can sign up, sign in, change their password, and end other sessions. Each todo is scoped to its owner.

## Run locally

Install Rust. The app defaults to a local `todo.db` file, `127.0.0.1:3000`, and secure cookies. For plain HTTP development:

```powershell
$env:SESSION_SECURE = "false"
$env:APP_ALLOWED_HOST = "127.0.0.1:3000,localhost:3000"
cargo run
```

The development database is created and migrated automatically. To use a libSQL server (Turso Cloud or self-hosted `sqld`), set `DATABASE_URL` and, if required, `DATABASE_AUTH_TOKEN`. Never commit database tokens or `.env`.

## Frontend assets

Askama compiles templates into the executable. Tailwind builds `static/app.css`, which is embedded at compile time. After changing templates or styles:

```sh
npm ci
npm run css:build
cargo run
```

Commit the updated `static/app.css` so deploys can use the checked-in asset. Node is only needed to build CSS; the server does not need Node.

## Deploy to a 1 GB VPS

The VPS runs three systemd services: `sqld` (self-hosted libSQL/Turso on `127.0.0.1:8080`), `todo` (the app on `127.0.0.1:3000`), and Caddy (HTTPS on ports 80/443). The existing `practice` system account and `/var/lib/practice` database directory are retained so an existing database survives the app rename. Keep port 8080 private; `sqld` has no authentication in this setup.

**The VPS does not compile the Rust project.** GitHub Actions builds the Linux executable and transfers it to the VPS. This keeps Rust, Node, and build dependencies off the 1 GB machine. The included workflow targets x86_64 Ubuntu; use a matching Linux build runner if the VPS is ARM.

### First deployment

1. Get a hostname that points at the VPS. You do not need to buy a domain: at [DuckDNS](https://www.duckdns.org/), sign in, enter an available name such as `mytodo`, add it, and set its IP to the VPS's public IPv4 address from your provider dashboard. The full hostname will be `mytodo.duckdns.org`. Since a VPS usually keeps the same IP, no DuckDNS updater or token is needed. Put that hostname in `.env` as `DOMAIN=mytodo.duckdns.org`. Caddy can then obtain a browser-trusted HTTPS certificate when ports 80 and 443 are open. [Caddy automatic HTTPS requirements](https://caddyserver.com/docs/automatic-https).
2. On the VPS, install Debian/Ubuntu and clone the repository to `/opt/to-do`:

   ```sh
   git clone <repo-url> /opt/to-do
   cd /opt/to-do
   cp .env.example .env
   nano .env   # set DOMAIN to the real domain
   sudo ./deploy/setup.sh
   ```

   Setup installs the `sqld` prebuilt binary, Caddy, and systemd units. It does not install Rust or compile the app. `deploy/setup.sh` is safe to re-run.
3. In GitHub repository settings, add Actions secrets `VPS_HOST`, `VPS_SSH_PORT`, `VPS_USER`, `VPS_SSH_KEY`, and `VPS_KNOWN_HOSTS`. Store the private SSH key in `VPS_SSH_KEY`. Generate known-hosts data with `ssh-keyscan -p <port> <host>`, verify the fingerprint against the VPS provider console, then save it as `VPS_KNOWN_HOSTS`. The SSH user needs passwordless sudo for only `install -m 755 /tmp/todo.new /usr/local/bin/todo`, `systemctl restart todo`, and `journalctl -u todo -n 50 --no-pager`. Keep `.env` on the VPS; the workflow never uploads it.
4. Push to `main` or `master`, or run the **deploy** workflow manually from Actions. The workflow builds CSS and the release executable on GitHub, copies the executable, restarts `todo`, and checks `/healthz`.

Subsequent pushes deploy automatically. To deploy manually, build the release executable on a matching Linux x86_64 host (`npm ci && npm run css:build && cargo build --release --locked`), then copy it and install it from the repository directory:

```sh
scp target/release/todo <user>@<vps>:/tmp/todo.new
ssh <user>@<vps> 'cd /opt/to-do && sudo ./deploy.sh /tmp/todo.new'
```

### Operations

- Logs: `journalctl -u todo -f` (also `sqld` and `caddy`)
- Restart: `sudo systemctl restart todo`
- Back up the database while stopped:

  ```sh
  sudo systemctl stop todo sqld
  sudo tar czf /root/todo-db-$(date +%F).tar.gz -C /var/lib practice
  sudo systemctl start sqld todo
  ```

- Set `SESSION_SECRET` in `.env` with `openssl rand -hex 32` before production use. Keep `.env` private; systemd loads it for the app.
- `SESSION_SECURE=true` is correct behind Caddy. Auth rate limiting uses Caddy's forwarded client address.

## Account limitations

The app does not send email. Email verification and password reset are not available, so users must keep their password safe. Account deletion and task export are not currently available.

## Project map

- `src/app.rs` — routes, session layer, shared middleware, static assets
- `src/auth/` — authentication, password handling, account pages, session storage
- `src/todo/` — todo handlers, queries, and templates
- `src/db.rs` and `migrations/` — libSQL connection and schema
- `templates/` and `static/` — server-rendered UI and browser assets
- `Caddyfile`, `deploy/`, `deploy.sh` — VPS deployment (Caddy, sqld, systemd)
- `tests/` — router and data-layer coverage

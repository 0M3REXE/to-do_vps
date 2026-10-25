# Practice

A small, server-rendered todo app built with Rust, Axum, Askama, HTMX 4, and libSQL. Accounts can sign up, sign in, change their password, and end other sessions. Each todo is scoped to its owner.

## Run locally

Rust is required. The app defaults to a local `practice.db` file, `127.0.0.1:3000`, and secure cookies. For plain HTTP development, set:

```powershell
$env:SESSION_SECURE = "false"
$env:APP_ALLOWED_HOST = "127.0.0.1:3000,localhost:3000"
cargo run
```

The development database is created and migrated automatically. To use a libSQL server instead (Turso cloud or a self-hosted sqld), set `DATABASE_URL` to its `libsql://`/`http(s)://` URL and `DATABASE_AUTH_TOKEN` to its token if it requires one. Never commit either secret.

## Frontend assets

Askama compiles templates into the Rust binary. Tailwind builds `static/app.css`, which the server embeds at compile time. After changing a template or style, rebuild the CSS and restart/rebuild the app:

```sh
npm ci
npm run css:build
cargo run
```

Node is build-time only: it produces `static/app.css`, which the binary embeds. The running server never needs Node either.

Use `npm run css:watch` while editing styles. The vendored `static/vendor/htmx.min.js` is pinned to 4.0.0.

## Deploy to a VPS

Bare metal — no Docker. Three systemd services on the box:

- `sqld.service` — **Turso's libSQL server, self-hosted** from the official
  prebuilt binary, listening on `127.0.0.1:8080` only (auth off, loopback
  only). Data lives in `/var/lib/practice/data.sqld`.
- `practice.service` — the app on `127.0.0.1:3000`; systemd loads the repo's
  `.env` as its environment. Migrations run at startup, so there is no
  separate migration step.
- `caddy` — the only public ports (80/443): automatic HTTPS for `DOMAIN`,
  forwarding to the app.

Target is a Debian/Ubuntu VPS (apt). Point your domain's A record at it first.

```sh
git clone <repo> /opt/practice && cd /opt/practice
cp .env.example .env        # set DOMAIN; SESSION_SECRET optional but recommended
sudo ./deploy/setup.sh      # one-time: swap, rustup, sqld binary, Caddy, systemd units
sudo ./deploy.sh            # build + install + restart + health check
```

Updates are the last two steps again: `git pull && sudo ./deploy.sh`.
`deploy.sh` compiles on the VPS (a 1 GB box gets swap from `setup.sh`); it
rebuilds the CSS when npm exists and otherwise compiles with the committed
`static/app.css`.

Day-to-day:

- Logs: `journalctl -u practice -f` (likewise `sqld`, `caddy`)
- Restart: `sudo systemctl restart practice`
- Backups — stop the app and archive the data directory:

  ```sh
  sudo systemctl stop practice
  sudo tar czf /root/practice-db-$(date +%F).tar.gz -C /var/lib/practice data.sqld
  sudo systemctl start practice
  ```

Notes:

- After changing templates or styles, rebuild the CSS and commit it (or have
  Node on the VPS): `npm ci && npm run css:build`.
- Never expose port 8080 — sqld has no authentication.
- `SESSION_SECURE` defaults to `true` (browsers talk HTTPS to Caddy); set it
  to `false` in `.env` only for a plain-HTTP localhost smoke test.
- Auth rate limiting keys on `X-Forwarded-For`, which Caddy appends — it is
  active in deployment and stays off for local development.

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
- `AUDIT_REMEDIATION_PLAN.md` — audit findings and remaining roadmap

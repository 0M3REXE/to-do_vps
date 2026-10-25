# To-do

A small server-rendered to-do app built with Rust, Axum, Askama, HTMX, and libSQL. Accounts can sign up, sign in, change their password, and end other sessions. Each task is scoped to its owner.

## Run locally

Install Rust. The app defaults to a local `todo.db` file and listens on `127.0.0.1:3000`.

```sh
cp .env.example .env
cargo run
```

To use Turso Cloud or a self-hosted libSQL server, set `DATABASE_URL` and, if required, `DATABASE_AUTH_TOKEN`. Never commit database tokens or `.env`.

## Frontend assets

Askama compiles templates into the executable. Tailwind builds `static/app.css`, which is embedded at compile time. After changing templates or styles:

```sh
npm ci
npm run css:build
```

Commit the updated `static/app.css`. GitHub Actions builds CSS and the Linux executable on a hosted runner. The VPS receives only the executable and Piku launch files; it does not install Rust or compile the app.

## VPS deployment with Piku

The production deployment uses Piku on Ubuntu 24.04. A GitHub Actions workflow builds the `todo` Linux binary on GitHub-hosted Ubuntu, packages it without Rust source files, then pushes that package to Piku over SSH. A normal push to `main` or `master` deploys it. See [PIKU-DEPLOYMENT.md](PIKU-DEPLOYMENT.md) for server setup, required GitHub secrets, DNS/TLS, deployment verification, and repeatable onboarding for more apps.

The package configures `todorust.duckdns.org` and a separate SQLite-compatible libSQL database file under each app's Piku `DATA_ROOT`. This is an embedded database file; it does not run a separate Turso server on the VPS.

## Account limitations

The app does not send email. Email verification and password reset are unavailable, so users must keep their password safe. Account deletion and task export are not currently available.

## Project map

- `src/app.rs` — routes, session layer, shared middleware, static assets
- `src/auth/` — authentication, password handling, account pages, session storage
- `src/todo/` — task handlers, queries, and templates
- `src/db.rs` and `migrations/` — libSQL connection and schema
- `templates/` and `static/` — server-rendered UI and browser assets
- `.github/workflows/deploy.yml` — To-do deployment settings
- `.github/workflows/piku-rust-deploy.yml` — reusable Linux build and Piku deploy workflow

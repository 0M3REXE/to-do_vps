//! # One router, shared by the server and the tests
//!
//! `main.rs` used to build the router itself, which meant the auth and session
//! layers would have been added there and therefore absent from every test.
//! Building it here instead means `cargo test` drives the same session cookies,
//! the same login guard and the same CSRF check that `cargo run` does.
//!
//! ## Where the session layer sits
//!
//! Within this function, `.layer()` calls are applied last-outermost, the same
//! rule as everywhere else in axum:
//!
//! ```text
//!   require_auth   (only the todo routes)
//!   verify_origin  (everywhere)
//!   auth layer     = cookie manager → session manager → AuthManager
//! ```
//!
//! ## Embedded static assets
//!
//! The CSS and JS ship *inside* the binary rather than from disk, so there is
//! no asset directory to configure or 404 on: the deployment image carries one
//! file. Local and deployed behaviour are identical. Templates are already
//! compiled in by askama for the same reason.

use std::time::Duration;

use axum::{
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_login::{
    AuthManagerLayerBuilder,
    tower_sessions::{Expiry, SessionManagerLayer, cookie::SameSite},
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::auth::{self, AuthBackend, AuthSession, DbStore};
use crate::db::Db;
use crate::AppError;

use askama::Template as _;

/// The app stylesheet.
pub static APP_CSS: &str = include_str!("../static/app.css");
/// Service worker for offline PWA support.
pub static SERVICE_WORKER_JS: &str = include_str!("../static/service-worker.js");
/// A safe, data-free page shown when an installed app opens without a network.
pub static OFFLINE_HTML: &str = include_str!("../static/offline.html");
/// Web app manifest for PWA installation.
pub static MANIFEST_JSON: &str = include_str!("../static/manifest.json");
/// The app's header mark and platform icons.
pub static BRAND_MARK_SVG: &str = include_str!("../static/brand-mark.svg");
pub static FAVICON_ICO: &[u8] = include_bytes!("../static/favicon.ico");
pub static ICON_180_PNG: &[u8] = include_bytes!("../static/icons/icon-180x180.png");
pub static ICON_192_PNG: &[u8] = include_bytes!("../static/icons/icon-192x192.png");
pub static ICON_512_PNG: &[u8] = include_bytes!("../static/icons/icon-512x512.png");
pub static ICON_512_MASKABLE_PNG: &[u8] =
    include_bytes!("../static/icons/icon-512-maskable.png");
/// Vendored htmx, unmodified.
pub static HTMX_JS: &str = include_str!("../static/vendor/htmx.min.js");
/// Vendored Alpine (the CSP build: no `unsafe-eval`, no inline `x-data={}`).
pub static ALPINE_JS: &str = include_str!("../static/vendor/alpine.min.js");
/// One Alpine component + htmx glue for the whole app. Contents change between
/// edits, so it is served with `must-revalidate` like the stylesheet.
pub static APP_JS: &str = include_str!("../static/app.js");

/// How long a session may sit untouched before it stops working, and whether
/// the cookie is allowed to leave a plain-http connection.
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Must be `true` anywhere that is not localhost. A `Secure` cookie is
    /// never sent over plain http, so setting this wrongly on a real
    /// deployment silently logs everyone out.
    pub session_secure: bool,
    /// Comma-separated trusted hostnames for state-changing request origins.
    /// The request's own Host is also checked, so a custom domain and any
    /// preview hostname work without trusting arbitrary origins.
    pub allowed_host: String,
    /// Optional signing key for the session cookie. When set, the cookie is
    /// cryptographically signed so a client cannot tamper with or forge it.
    /// Sessions are server-side, so this is defense-in-depth, not the sole
    /// protection.
    pub session_secret: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            // Secure by default: forgetting to set it should fail closed.
            session_secure: true,
            allowed_host: "localhost:3000".to_owned(),
            session_secret: None,
        }
    }
}

impl AppConfig {
    /// Reads the settings from the environment, falling back to the defaults.
    pub fn from_env() -> Self {
        let defaults = Self::default();

        let session_secure = std::env::var("SESSION_SECURE")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(defaults.session_secure);

        let mut allowed_hosts: Vec<String> = std::env::var("APP_ALLOWED_HOST")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(str::to_owned)
            .collect();

        if allowed_hosts.is_empty() {
            allowed_hosts.push(defaults.allowed_host.clone());
        }
        let allowed_host = allowed_hosts.join(",");

        let session_secret = std::env::var("SESSION_SECRET").ok();

        Self {
            session_secure,
            allowed_host,
            session_secret,
        }
    }
}

/// A session lives a week of inactivity. Long enough that a returning visitor
/// is not asked to log in daily, short enough that a forgotten laptop does not
/// keep a live session for months.
const SESSION_IDLE_LIFETIME: time::Duration = time::Duration::days(7);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REQUEST_BODY: usize = 16 * 1024;

/// Serves an embedded asset with a long cache life. These paths are stable
/// filenames rather than content-hashed, but the content is fixed per build, so
/// "immutable" is still honest: a new build arrives as a new deployment.
fn asset(body: &'static str, content_type: &'static str) -> axum::response::Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        body,
    )
        .into_response()
}

/// Serves a raster app icon from the binary with a long cache life.
fn image_asset(body: &'static [u8]) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        axum::body::Body::from(body),
    )
        .into_response()
}

/// Serves the browser's conventional root favicon URL.
fn favicon_asset(body: &'static [u8]) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/x-icon"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        axum::body::Body::from(body),
    )
        .into_response()
}

/// The stylesheet keeps a stable URL so new deployments must revalidate it.
fn stylesheet() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "public, max-age=0, must-revalidate"),
        ],
        APP_CSS,
    )
        .into_response()
}

/// Sets safe cache headers on dynamic responses.
///
/// Static assets already set their own long-lived immutable headers via
/// `asset()`, so this middleware only touches responses that haven't
/// specified a cache policy. User-specific HTML must never be stored by
/// shared caches (CDNs, proxies) or a browser's disk cache, so private pages
/// must not be retained after logout or on a shared device.
async fn cache_control(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    if response.headers().get(header::CACHE_CONTROL).is_none() {
        let _ = response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("private, no-store"),
        );
    }
    response
}

/// The landing page. Public — no session required. A logged-in visitor is
/// sent straight to their todo app; everyone else sees the marketing page
/// with login and signup buttons.
async fn landing(session: AuthSession) -> Result<Response, AppError> {
    if session.user.is_some() {
        return Ok(Redirect::to("/app").into_response());
    }
    Ok(Html(
        crate::auth::LandingTemplate::default()
            .render()
            .map_err(|err| AppError::Render(err))?,
    )
    .into_response())
}

/// Builds the whole application: routes, session layer, auth layer, CSRF check.
pub fn build_app(db: Db, config: AppConfig) -> Router {
    let session_layer = SessionManagerLayer::new(DbStore::new(db.clone()))
        .with_name("session")
        // No JavaScript on this app reads the cookie, so nothing needs it.
        .with_http_only(true)
        // Lax, not Strict: Lax still blocks the cross-site POST that CSRF
        // depends on, while allowing a normal link from another site to arrive
        // logged in.
        .with_same_site(SameSite::Lax)
        .with_secure(config.session_secure)
        .with_expiry(Expiry::OnInactivity(SESSION_IDLE_LIFETIME));

    let app = Router::new()
        // Public first: landing page, then auth pages.
        .route("/", get(landing))
        .merge(auth::routes(db.clone()))
        .merge(auth::account_routes(db.clone()))
        // The todo app is nested under /app so its internal routes (/,
        // /todos, /todos/{id}/toggle, /todos/{id}/delete) become
        // /app/, /app/todos, etc.
        .nest("/app", crate::todo::build_router(db.clone()))
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/static/app.css",
            get(|| std::future::ready(stylesheet())),
        )
        .route(
            "/static/vendor/alpine.min.js",
            get(|| std::future::ready(asset(ALPINE_JS, "text/javascript"))),
        )
        .route(
            "/static/app.js",
            get(|| {
                std::future::ready((
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "text/javascript"),
                        // Stable URL, but contents change between edits —
                        // revalidate like the stylesheet rather than serving
                        // it immutable, so a new build is picked up at once.
                        (header::CACHE_CONTROL, "public, max-age=0, must-revalidate"),
                    ],
                    APP_JS,
                )
                    .into_response())
            }),
        )
        .route(
            "/static/service-worker.js",
            get(|| std::future::ready(asset(SERVICE_WORKER_JS, "application/javascript"))),
        )
        .route(
            "/service-worker.js",
            get(|| {
                std::future::ready((
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "application/javascript"),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
                    SERVICE_WORKER_JS,
                ))
            }),
        )
        .route(
            "/offline",
            get(|| std::future::ready(asset(OFFLINE_HTML, "text/html; charset=utf-8"))),
        )
        .route(
            "/static/manifest.json",
            get(|| std::future::ready(asset(MANIFEST_JSON, "application/json"))),
        )
        .route(
            "/static/brand-mark.svg",
            get(|| std::future::ready(asset(BRAND_MARK_SVG, "image/svg+xml"))),
        )
        .route(
            "/favicon.ico",
            get(|| std::future::ready(favicon_asset(FAVICON_ICO))),
        )
        .route(
            "/static/icons/icon-180x180.png",
            get(|| std::future::ready(image_asset(ICON_180_PNG))),
        )
        .route(
            "/static/icons/icon-192x192.png",
            get(|| std::future::ready(image_asset(ICON_192_PNG))),
        )
        .route(
            "/static/icons/icon-512x512.png",
            get(|| std::future::ready(image_asset(ICON_512_PNG))),
        )
        .route(
            "/static/icons/icon-512-maskable.png",
            get(|| std::future::ready(image_asset(ICON_512_MASKABLE_PNG))),
        )
        .route(
            "/static/vendor/htmx.min.js",
            get(|| std::future::ready(asset(HTMX_JS, "text/javascript"))),
        )
        // Cache control wraps every route handler. Static assets already set
        // their own headers via `asset()`, so this only touches dynamic HTML.
        .layer(middleware::from_fn(cache_control))
        .layer(middleware::from_fn_with_state(
            config.allowed_host.clone(),
            auth::verify_origin,
        ));

    // Apply the auth layer. When SESSION_SECRET is configured, the session
    // cookie is cryptographically signed so a client cannot tamper with it.
    // Sessions are server-side, so this is defense-in-depth.
    let app = if let Some(secret) = &config.session_secret {
        let key = tower_sessions::cookie::Key::try_from(secret.as_bytes())
            .expect("SESSION_SECRET must produce a valid signing key");
        app.layer(
            AuthManagerLayerBuilder::new(AuthBackend { db: db.clone() }, session_layer.with_signed(key))
                .with_data_key(crate::auth::AUTH_DATA_KEY)
                .build(),
        )
    } else {
        app.layer(
            AuthManagerLayerBuilder::new(AuthBackend { db: db.clone() }, session_layer)
                .with_data_key(crate::auth::AUTH_DATA_KEY)
                .build(),
        )
    };

    let mut app = app
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY))
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; \
                 base-uri 'self'; object-src 'none'; frame-ancestors 'none'; \
                 form-action 'self'; script-src 'self' \
                 'sha256-Yg5vmur3/59vdKj7wIu7Bu7aifpYCWZqdoZ74HVh5wg='; \
                 style-src 'self' https://fonts.googleapis.com; \
                 font-src 'self' https://fonts.gstatic.com; \
                 img-src 'self' data:; connect-src 'self'",
            ),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(CatchPanicLayer::new());

    if config.session_secure {
        app = app.layer(SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        ));
    }

    app
}

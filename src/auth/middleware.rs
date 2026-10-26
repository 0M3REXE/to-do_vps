//! # Two middlewares: "you must be logged in", and "this POST is not a forgery"

use axum::{
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use tower_cookies::Cookies;

use super::{AuthSession, security};
use super::security::{SECURITY_DATA_KEY, SessionPolicy, SessionSecurity};


/// htmx sets this on every request it makes, and reads `HX-Redirect` back to
/// tell it to do a full browser navigation instead of a swap.
const HX_REQUEST_TYPE: &str = "hx-request-type";
const HX_REDIRECT: &str = "hx-redirect";

/// The client address the rate limiter keys on — the one the reverse proxy
/// reports, or nothing.
///
/// Behind Caddy, `X-Forwarded-For` ends with the address Caddy itself saw, and
/// that last entry is appended by Caddy, so a client cannot forge it by
/// sending its own header. With no proxy in front (local dev, the test suite)
/// the header is absent and the result is `None`: rate limiting stays off
/// rather than lumping every local request into one shared bucket.
pub fn rate_limit_address(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_owned)
}

/// Sends anyone without a session to the login page.
///
/// An htmx request gets `HX-Redirect` instead of a 303, because a 303 would
/// make htmx swap a whole login page into `#todo-list` — leaving the visitor
/// staring at a login form in the middle of their list, with no way out.
pub async fn require_auth(req: Request, next: Next) -> Response {
    eprintln!("RA path={}", req.uri().path());
    if req
        .extensions()
        .get::<AuthSession>()
        .is_some_and(|session| session.user.is_some())
    {
        return next.run(req).await;
    }

    login_redirect(&req)
}

/// The normal login flow, plus device binding, absolute lifetime, and the
/// recent-authentication gate for sensitive account actions.
///
/// This runs inside the todo and account routers, so it sees the same path
/// that the existing tests expect (for example `/` for `/app`).
pub async fn require_session(
    req: Request,
    next: Next,
) -> Response {
    eprintln!("RS TOP path={}", req.uri().path());
    let policy = match req.extensions().get::<SessionPolicy>().cloned() {
        Some(policy) => policy,
        None => {
            println!("require_session: no policy extension");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let cookies = match req.extensions().get::<Cookies>().cloned() {
        Some(cookies) => cookies,
        None => {
            println!("require_session: no cookies extension");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let original_uri = req
        .extensions()
        .get::<OriginalUri>()
        .cloned()
        .unwrap_or_else(|| OriginalUri(req.uri().clone()));
    println!("require_session path={}", req.uri().path());

    // The body of the guard needs `req` and the extracted values. Move the
    // rest into a helper by shadowing is not possible, so keep the logic here.
    let session = match req.extensions().get::<AuthSession>() {
        Some(session) if session.user.is_some() => session.clone(),
        _ => return login_redirect(&req),
    };

    let session = session;
    let path = req.uri().path();

    // The re-authentication page must remain reachable even when the device
    // cookie is missing. It is still protected by authentication and the
    // absolute lifetime check above.
    if path == "/account/reauth" {
        return next.run(req).await;
    }

    let security = match session
        .session
        .get::<SessionSecurity>(SECURITY_DATA_KEY)
        .await
    {
        Ok(Some(security)) => security,
        Ok(None) => return step_up_redirect(&policy, &req, &original_uri.0, "missing"),
        Err(err) => {
            tracing::error!(error = %err, "session store failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let now = security::now_unix();

    // Device token: a copied session cookie without the matching device cookie
    // cannot proceed.
    let device_ok = cookies
        .get(&policy.device_cookie_name)
        .map(|cookie| security::hash_token(cookie.value()))
        .as_deref()
        == Some(security.device_hash.as_str());
    if !device_ok {
        return step_up_redirect(&policy, &req, &original_uri.0, "device");
    }

    // User-Agent: a copied cookie used in another browser changes this.
    if security::request_user_agent(req.headers()) != security.user_agent {
        return step_up_redirect(&policy, &req, &original_uri.0, "ua");
    }

    // Sensitive actions require a password confirmation within the recent
    // window. Password change is intentionally excluded because its form
    // already requires the current password.
    if needs_recent_auth(req.method(), path)
        && now.saturating_sub(security.last_step_up) > policy.step_up.whole_seconds()
    {
        return step_up_redirect(&policy, &req, &original_uri.0, "recent");
    }

    // Throttle the activity write so an active session slides forward without
    // turning every read into a database write.
    if now.saturating_sub(security.last_seen) >= security::TOUCH_INTERVAL_SECONDS {
        let mut security = security;
        security.last_seen = now;
        let ip = security::request_ip(req.headers());
        let ip_changed = security.last_ip != ip;
        if ip_changed {
            tracing::info!(
                user.id = session.user.as_ref().map(|user| user.id),
                old.ip = %security.last_ip,
                new.ip = %ip,
                "session_ip_changed"
            );
            security.last_ip = ip;
        }
        if let Err(err) = session
            .session
            .insert(SECURITY_DATA_KEY, &security)
            .await
        {
            tracing::error!(error = %err, "session store failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    next.run(req).await
}

fn needs_recent_auth(method: &axum::http::Method, path: &str) -> bool {
    if method != axum::http::Method::GET
        && method != axum::http::Method::POST
    {
        // Be conservative: unknown methods on account paths require confirmation.
        path == "/account/sessions"
            || path == "/account/logout-everywhere"
            || path.starts_with("/account/sessions/")
    } else {
        path == "/account/sessions"
            || path == "/account/logout-everywhere"
            || path.starts_with("/account/sessions/")
    }
}

fn login_redirect(req: &Request) -> Response {
    if req
        .headers()
        .get(HX_REQUEST_TYPE)
        .and_then(|value| value.to_str().ok())
        == Some("partial")
    {
        return (StatusCode::OK, [(HX_REDIRECT, "/login")]).into_response();
    }

    // Remember where they were headed so the login page can send them back.
    let target = format!("/login?next={}", encode_query_value(req.uri().path()));
    Redirect::to(&target).into_response()
}

fn step_up_redirect(
    _policy: &SessionPolicy,
    req: &Request,
    original_uri: &axum::http::Uri,
    reason: &str,
) -> Response {
    let next = if req.method() == axum::http::Method::GET || req.method() == axum::http::Method::HEAD
    {
        original_uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/app")
    } else {
        match req.uri().path() {
            "/account/sessions" | "/account/sessions/" => "/account/sessions",
            "/account/logout-everywhere" => "/account/password",
            "/account/password" => "/account/password",
            _ => "/app",
        }
    };
    let next = safe_next(next);
    let target = format!(
        "/account/reauth?reason={}&next={}",
        reason,
        encode_query_value(&next)
    );

    if req
        .headers()
        .get(HX_REQUEST_TYPE)
        .and_then(|value| value.to_str().ok())
        == Some("partial")
    {
        return (StatusCode::OK, [(HX_REDIRECT, target.as_str())]).into_response();
    }

    Redirect::to(&target).into_response()
}

/// Rejects state-changing requests that arrive from another site.
///
/// The session cookie is `SameSite=Lax`, which already stops a cross-site form
/// post from carrying credentials, so this is the second lock rather than the
/// only one. It catches a browser or extension that has been talked into sending
/// one anyway.
///
/// A request with neither header is allowed through. Browsers always send
/// `Origin` on a POST, so their absence means a non-browser client (curl, this
/// project's own test suite) rather than a forged browser request.
pub async fn verify_origin(
    State(allowed_hosts): State<String>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    if !is_state_changing(req.method()) {
        return next.run(req).await;
    }

    // Browsers send Origin on form submissions and fetch requests. Referer is
    // a fallback for older clients. If either is present but malformed or
    // foreign, reject it rather than silently treating it as absent.
    if headers.contains_key(header::ORIGIN) || headers.contains_key(header::REFERER) {
        let Some(origin_host) = presented_host(&headers) else {
            return cross_origin_refused(&headers, &req);
        };

        let request_host = req
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(normalize_host);
        let explicitly_allowed = allowed_hosts
            .split(',')
            .filter_map(normalize_host)
            .any(|host| host == origin_host);

        if !explicitly_allowed && request_host.as_deref() != Some(origin_host.as_str()) {
            return cross_origin_refused(&headers, &req);
        }
    } else if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site.eq_ignore_ascii_case("cross-site"))
    {
        // Some browser requests omit Origin and Referer. Fetch Metadata still
        // lets us reject an explicitly cross-site state change.
        return cross_origin_refused(&headers, &req);
    }

    next.run(req).await
}

fn cross_origin_refused(headers: &HeaderMap, req: &Request) -> Response {
    tracing::warn!(
        method = %req.method(),
        path = %req.uri().path(),
        presented_host = ?presented_host(headers),
        ip = %super::security::request_ip(headers),
        ua = %super::security::request_user_agent(headers),
        "cross_origin_request_refused"
    );
    (StatusCode::FORBIDDEN, "cross-origin request refused").into_response()
}

/// Only the methods that change something need the check. A GET cannot alter
/// state, so it is not a CSRF vector.
fn is_state_changing(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// The `host[:port]` the request claims to come from, preferring `Origin` and
/// falling back to `Referer`.
///
/// Only the authority is compared, not the scheme: the check is "did this come
/// from us", and `http://localhost:3000` and `https://localhost:3000` are the
/// same place.
fn presented_host(headers: &HeaderMap) -> Option<String> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        return normalize_host(origin.to_str().ok()?);
    }
    normalize_host(headers.get(header::REFERER)?.to_str().ok()?)
}

/// Accepts either a URL or a host[:port], and normalizes DNS case and trailing
/// slashes before exact comparison. Userinfo and whitespace are never valid.
fn normalize_host(value: &str) -> Option<String> {
    let value = value.trim();
    let authority = match value.split_once("://") {
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") =>
        {
            rest.split(['/', '?', '#']).next().unwrap_or_default()
        }
        Some(_) => return None,
        None => value.split(['/', '?', '#']).next().unwrap_or_default(),
    };
    if authority.is_empty()
        || authority.contains('@')
        || authority.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(authority.trim_end_matches('.').to_ascii_lowercase())
}

/// Percent-encodes the characters that would otherwise break a query string,
/// so a path containing `&` or `#` cannot smuggle in extra parameters.
fn encode_query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Turns the `?next=` value into a path we are willing to send someone to.
///
/// Only a path on this site qualifies. An absolute URL hands the visitor to
/// another site wearing our redirect; `//evil.com` and `/\evil.com` are the
/// same trick written so a naive "does it start with a slash" check misses it.
/// Anything that is not a local path falls back to the home page.
pub fn safe_next(candidate: &str) -> String {
    let trimmed = candidate.trim();
    let is_local =
        trimmed.starts_with('/') && !trimmed.starts_with("//") && !trimmed.contains('\\');

    if is_local {
        trimmed.to_owned()
    } else {
        "/".to_owned()
    }
}

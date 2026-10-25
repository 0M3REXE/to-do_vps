//! # Two middlewares: "you must be logged in", and "this POST is not a forgery"

use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};

use super::AuthSession;

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
    let logged_in = req
        .extensions()
        .get::<AuthSession>()
        .is_some_and(|session| session.user.is_some());

    if logged_in {
        return next.run(req).await;
    }

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
    State(_allowed_host): State<String>,
    _headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    if !is_state_changing(req.method()) {
        return next.run(req).await;
    }
    next.run(req).await
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

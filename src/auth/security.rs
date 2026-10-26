//! # Session security metadata and cookies
//!
//! The session cookie alone is not enough to use a session. A second,
//! HttpOnly device cookie must accompany it, and its hash is stored inside the
//! server-side session record. A copied session cookie presented without that
//! device cookie is sent through a password step-up instead of being trusted.
//!
//! The values here are deliberately small and boring: Unix seconds, SHA-256
//! hex, and a random public id for the sessions page. No raw device token is
//! ever stored server-side.

use axum::http::{HeaderMap, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tower_sessions::cookie::{Cookie, SameSite};
use tower_sessions::session::Id;

use super::middleware::rate_limit_address;

/// Reserved key in the tower-sessions data map.
pub const SECURITY_DATA_KEY: &str = "todo.security";

/// A `last_seen` write is throttled to this interval. That write is also what
/// refreshes the session layer's inactivity expiry for an active visitor.
pub const TOUCH_INTERVAL_SECONDS: i64 = 5 * 60;

/// Everything the request guard and handlers need to know about cookie names
/// and timing. Derived from `AppConfig`; contains no secret.
#[derive(Clone, Debug)]
pub struct SessionPolicy {
    pub session_cookie_name: String,
    pub device_cookie_name: String,
    pub secure: bool,
    pub idle: time::Duration,
    pub step_up: time::Duration,
    pub device_lifetime: time::Duration,
}

/// The server-side half of a session's device binding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSecurity {
    /// Unix seconds of the last successful password check.
    pub last_step_up: i64,
    /// Unix seconds of the last throttled activity write.
    pub last_seen: i64,
    /// Random, non-secret id used by the sessions page.
    pub public_id: String,
    /// SHA-256 hex of the raw device cookie value.
    pub device_hash: String,
    /// User-Agent captured at login, step-up, or password change.
    pub user_agent: String,
    /// Last observed proxy address, or `unknown`.
    pub last_ip: String,
}

pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// A fresh 128-bit random bearer value. The session layer already uses the
/// same random id type, so no extra RNG dependency is introduced.
pub fn new_token() -> String {
    Id::default().0.to_string()
}

/// A random identifier that is safe to show in HTML. It is not an
/// authentication secret.
pub fn new_public_id() -> String {
    format!("{:032x}", Id::default().0 as u128)
}

pub fn hash_token(raw: &str) -> String {
    Sha256::digest(raw.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn request_ip(headers: &HeaderMap) -> String {
    rate_limit_address(headers).unwrap_or_else(|| "unknown".to_owned())
}

pub fn request_user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(256).collect())
        .unwrap_or_else(|| "unknown".to_owned())
}

pub fn fresh_security(headers: &HeaderMap, device_token: &str) -> SessionSecurity {
    let now = now_unix();
    SessionSecurity {
        last_step_up: now,
        last_seen: now,
        public_id: new_public_id(),
        device_hash: hash_token(device_token),
        user_agent: request_user_agent(headers),
        last_ip: request_ip(headers),
    }
}

/// The device cookie. It is deliberately not signed: it is a random bearer
/// value, and only its hash is kept server-side.
pub fn device_cookie(policy: &SessionPolicy, token: &str) -> Cookie<'static> {
    Cookie::build((
        policy.device_cookie_name.clone(),
        token.to_owned(),
    ))
    .path("/")
    .http_only(true)
    .secure(policy.secure)
    .same_site(SameSite::Lax)
    .max_age(policy.device_lifetime)
    .build()
}

/// Expires the device cookie. The cookie manager sends this in the same
/// response as the session cookie removal on logout.
pub fn expired_device_cookie(policy: &SessionPolicy) -> Cookie<'static> {
    Cookie::build((policy.device_cookie_name.clone(), String::new()))
        .path("/")
        .http_only(true)
        .secure(policy.secure)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::ZERO)
        .build()
}

/// A short, human-readable label for the sessions page. The exact User-Agent
/// is still available separately; this is only a convenience.
pub fn describe_user_agent(user_agent: &str) -> String {
    if user_agent == "unknown" {
        return "Unknown device".to_owned();
    }

    let browser = if user_agent.contains("Edg/") {
        "Edge"
    } else if user_agent.contains("OPR/") || user_agent.contains("Opera") {
        "Opera"
    } else if user_agent.contains("Firefox/") {
        "Firefox"
    } else if user_agent.contains("Chrome/") || user_agent.contains("CriOS/") {
        "Chrome"
    } else if user_agent.contains("Safari/") {
        "Safari"
    } else {
        "Browser"
    };

    let platform = if user_agent.contains("Android") {
        "Android"
    } else if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        "iOS"
    } else if user_agent.contains("Windows") {
        "Windows"
    } else if user_agent.contains("Mac OS X") || user_agent.contains("Macintosh") {
        "macOS"
    } else if user_agent.contains("Linux") {
        "Linux"
    } else {
        "an unknown system"
    };

    format!("{browser} on {platform}")
}

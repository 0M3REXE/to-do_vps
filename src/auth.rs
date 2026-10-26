//! # The auth domain — module front door
//!
//! Holds `User`, the aliases, and re-exports the submodules, so callers write
//! `auth::AuthSession` and `auth::DbStore`. Split by concern: `store` (all
//! SQL), `password`, `backend` (the axum-login traits), `middleware`,
//! `handlers`, `templates`.

mod backend;
mod handlers;
pub mod middleware;
pub mod password;
pub mod security;
mod store;
mod templates;

pub use backend::{AuthBackend, Credentials};
pub use handlers::{account_routes, routes};
pub use middleware::{require_auth, require_session, safe_next, verify_origin};
pub use password::{MAX_PASSWORD_LEN, MIN_PASSWORD_LEN};
pub use security::SessionPolicy;
pub use store::{
    DbStore, SessionSummary, allow_auth_attempt, count_sessions_for_user, create_user,
    delete_session_by_key, delete_sessions_for_user, find_user, list_sessions_for_user,
    set_password, user_id_by_email,
};
pub(crate) use templates::LandingTemplate;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_login::AuthSession as AxumLoginSession;
use sha2::{Digest, Sha256};

use crate::AppError;

/// The account behind the current session.
///
/// Deliberately does **not** keep `password_hash`. axum-login stores whatever
/// `session_auth_hash` returns inside the session, and sessions end up in the
/// `sessions` table — putting the Argon2 hash there would mean a leaked session
/// row is a leaked password hash. Instead this holds a SHA-256 *of* the hash,
/// which changes whenever the password does (so changing a password still logs
/// every other device out) but is useless for cracking.
#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
    auth_hash: Vec<u8>,
}

impl User {
    pub(crate) fn new(id: i64, email: String, password_hash: &str) -> Self {
        Self {
            id,
            email,
            auth_hash: Sha256::digest(password_hash.as_bytes()).to_vec(),
        }
    }
}

/// Shorthand for the session type axum-login hands to handlers.
pub type AuthSession = AxumLoginSession<AuthBackend>;

/// The key the auth layer stores its login state under inside the session.
///
/// axum-login defaults to `"axum-login.data"`, but the session store has to
/// read the account id back out of that payload to fill in `sessions.user_id`.
/// Naming it here means the writer and the reader cannot drift apart.
pub const AUTH_DATA_KEY: &str = "practice.auth";

/// The logged-in user, as a handler argument.
///
/// `require_auth` has already redirected anyone without a session, so in a
/// correct arrangement this never fails. It still returns a `Result` rather
/// than unwrapping: a new route mounted outside the guard should get a 401 and
/// a log line, not a panic.
pub struct CurrentUser(pub User);

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthSession>()
            .and_then(|session| session.user.clone())
            .map(CurrentUser)
            .ok_or(AppError::Unauthenticated)
    }
}

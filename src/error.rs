//! # Error handling — the one error type for the whole app
//!
//! This used to live in `todo/error.rs`. It moved up to the crate root when
//! authentication arrived, because logging in can fail too (a hash that will
//! not compute, a session that will not save) and there is no reason for one
//! domain to own the error the other one returns.
//!
//! The contract is unchanged: the real cause goes to `tracing`, the client gets
//! a fixed string. A leaked error message is how internal structure gets
//! exposed.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use libsql::Error as SqlError;

#[derive(Debug)]
pub enum AppError {
    Render(askama::Error),
    Database(SqlError),
    /// The blocking-pool task that hashes a password did not finish. In practice
    /// this means the hashing library panicked, which it does not.
    Hashing(tokio::task::JoinError),
    /// The session layer refused to save, load or clear. Holds the library's
    /// own message, already rendered with `Display`.
    Session(String),
    /// A handler asked who is logged in and got nobody. `require_auth` should
    /// have redirected first, so reaching this is a bug rather than a user
    /// error — it fails closed with a 401 instead of pretending it is fine.
    Unauthenticated,
}

impl AppError {
    fn into_response_inner(self) -> Response {
        match &self {
            Self::Render(err) => tracing::error!(?err, "template render failed"),
            Self::Database(err) => tracing::error!(?err, "database query failed"),
            Self::Hashing(err) => tracing::error!(?err, "password hashing failed"),
            Self::Session(message) => tracing::error!(%message, "session operation failed"),
            Self::Unauthenticated => {
                tracing::warn!("handler ran without a logged-in user; is require_auth mounted?");
            }
        }

        match self {
            // Not a server fault, so not a 500.
            Self::Unauthenticated => StatusCode::UNAUTHORIZED.into_response(),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong").into_response(),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Render(err) => write!(f, "template render failed: {err}"),
            Self::Database(err) => write!(f, "database query failed: {err}"),
            Self::Hashing(err) => write!(f, "password hashing failed: {err}"),
            Self::Session(message) => write!(f, "session operation failed: {message}"),
            Self::Unauthenticated => write!(f, "no user is logged in"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Render(err) => Some(err),
            Self::Database(err) => Some(err),
            Self::Hashing(err) => Some(err),
            // A String has no inner error to walk to.
            Self::Session(_) | Self::Unauthenticated => None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.into_response_inner()
    }
}

impl From<askama::Error> for AppError {
    fn from(err: askama::Error) -> Self {
        Self::Render(err)
    }
}

impl From<SqlError> for AppError {
    fn from(err: SqlError) -> Self {
        Self::Database(err)
    }
}

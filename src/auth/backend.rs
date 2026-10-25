//! # The two traits axum-login asks the app to implement
//!
//! axum-login needs to know two things: "do these credentials match an
//! account?" and "which account has this id?". This file answers both, and
//! nothing else. Everything about *how* sessions are stored is axum-login's
//! problem, not ours.

use libsql::Error;

use axum_login::{AuthUser, AuthnBackend};

use crate::db::Db;

use super::{User, password, store};

/// What the login form sends.
///
/// Deliberately not `Debug`: anything deriving `Debug` can end up inside a
/// `tracing` span or a panic message, and this struct holds a plaintext
/// password. axum-login only requires `Send + Sync`, so nothing is lost.
pub struct Credentials {
    pub email: String,
    pub password: String,
}

/// The user's side of the pair: a database connection to answer questions with.
#[derive(Clone)]
pub struct AuthBackend {
    pub db: Db,
}

impl AuthnBackend for AuthBackend {
    type User = User;
    type Credentials = Credentials;
    type Error = Error;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        // Signup never accepts longer passwords. Reject oversized login
        // attempts before Argon2 so a large form value cannot waste CPU.
        if creds.password.chars().count() > password::MAX_PASSWORD_LEN {
            return Ok(None);
        }

        let found = store::find_by_email(&self.db, creds.email.trim()).await?;

        // Hash either the real password hash or a throwaway one, so that a
        // missing account and a wrong password take the same time.
        let stored_hash = match &found {
            Some(row) => row.password_hash.clone(),
            None => password::dummy_hash().await.to_owned(),
        };

        if !password::verify(creds.password, stored_hash).await {
            return Ok(None);
        }

        // Only now is the password known to be correct, so only now does the
        // row become a User — which never carries the hash.
        Ok(found.map(store::UserRow::into_user))
    }

    async fn get_user(&self, user_id: &i64) -> Result<Option<Self::User>, Self::Error> {
        let found = store::find_by_id(&self.db, *user_id).await?;
        Ok(found.map(store::UserRow::into_user))
    }
}

// The id half of the pair. `User` itself lives in the module front door.
impl AuthUser for User {
    type Id = i64;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        &self.auth_hash
    }
}

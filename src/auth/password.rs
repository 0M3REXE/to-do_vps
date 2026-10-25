//! # Password hashing — Argon2id, off the async thread
//!
//! Argon2 is deliberately slow: one hash at the library's default cost burns
//! roughly 50ms of CPU. Doing that on a tokio worker thread would stall every
//! other request sharing the thread, so every call here goes through
//! `spawn_blocking`, which moves the work to a dedicated blocking thread and
//! gives the runtime back its worker immediately.
//!
//! `password-auth` chooses the algorithm for us and picks Argon2 by default.

use tokio::sync::OnceCell;

use crate::AppError;

/// Matches what the signup form will accept. Eight is the floor NIST SP
/// 800-63B allows for a single-factor password; 128 is where a passphrase stops
/// being realistic and starts being a denial-of-service against our own CPU.
pub const MIN_PASSWORD_LEN: usize = 8;
pub const MAX_PASSWORD_LEN: usize = 128;

/// Checks a new password. Returns the message to show under the field, or
/// `Ok(())`.
///
/// There is no rule about symbols or capital letters on purpose: length is what
/// actually buys resistance, and composition rules mostly produce
/// `Password1!`-style passwords that are easier to guess, not harder.
pub fn validate(password: &str) -> Result<(), &'static str> {
    // Count characters, not bytes — a ten-character emoji password is forty
    // bytes long and perfectly reasonable.
    let length = password.chars().count();

    if password.trim().is_empty() {
        return Err("Enter a password.");
    }
    if length < MIN_PASSWORD_LEN {
        return Err("Password must be at least 8 characters.");
    }
    if length > MAX_PASSWORD_LEN {
        return Err("Password must be at most 128 characters.");
    }
    Ok(())
}

/// Hashes a password. The returned string is a PHC hash: the algorithm, its
/// parameters, the salt and the digest, all in one line.
///
/// Note that `password-auth`'s own signature cannot fail, so it panics rather
/// than returning an error. That panic happens on the blocking thread, where
/// `spawn_blocking` catches it and hands it back as a `JoinError` — which is
/// why this function still returns a `Result`.
pub async fn hash(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || password_auth::generate_hash(password))
        .await
        .map_err(AppError::Hashing)
}

/// True when the password matches the hash.
///
/// Every failure — wrong password, unparsable hash, a panic inside the hashing
/// library — comes back as `false`. The only safe answer to "I could not check
/// this" is "no".
pub async fn verify(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || password_auth::verify_password(password, &hash))
        .await
        .is_ok_and(|result| result.is_ok())
}

/// A real Argon2 hash of a password nobody has, used when the submitted email
/// is not registered.
///
/// Without it, "no such account" returns in microseconds while a real account
/// with a wrong password takes 50ms — and that difference is enough to
/// enumerate every address on the site. Hashing a throwaway value makes both
/// paths cost the same.
///
/// Built once and kept, because generating it costs the same 50ms it is
/// protecting. It is only reached when an email does not exist, so a process
/// that never sees a bad login never builds one.
pub async fn dummy_hash() -> &'static str {
    static DUMMY: OnceCell<String> = OnceCell::const_new();

    DUMMY
        .get_or_init(|| async {
            tokio::task::spawn_blocking(|| {
                password_auth::generate_hash("this password is deliberately never used")
            })
            .await
            // Unreachable in practice. An empty string simply fails to parse in
            // `verify`, which costs back the timing protection but never grants a
            // login.
            .unwrap_or_default()
        })
        .await
        .as_str()
}

//! # HTTP layer — signup, login, logout, account
//!
//! These are plain HTML forms: POST, then either a 303 to a new page or the
//! same page re-rendered with a message. No htmx, because a swapped login page
//! is a confusing thing to hand someone who is not logged in.
//!
//! Every handler returns `Result<_, AppError>`; `AppError` turns a failure into
//! a 500 with the real cause logged and nothing useful leaked to the client.

use askama::Template as _;
use axum::{
    Form, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;

use super::{
    AuthBackend, AuthSession, CurrentUser, User, middleware::safe_next, password, store, templates,
};
use crate::AppError;
use crate::db::Db;

/// Every way a login can fail says the same thing. Naming the half that was
/// wrong — "no such email" — is enough to enumerate every account on the site.
const BAD_CREDENTIALS: &str = "Invalid email or password.";

/// How many `@` checks the email gets before we believe it.
///
/// A full RFC 5322 parser would reject addresses some mail servers accept and
/// accept some they reject, which buys nothing here. The database's length
/// constraint is the real limit.
fn validate_email(email: &str) -> Result<(), &'static str> {
    if email.chars().count() > 254 {
        return Err("Email address is too long.");
    }

    match email.split_once('@') {
        Some((local, domain))
            if !local.is_empty() && !domain.is_empty() && !domain.contains('@') =>
        {
            Ok(())
        }
        _ => Err("Enter a valid email address."),
    }
}

// --- signup ------------------------------------------------------------------

async fn signup_form() -> Result<Html<String>, AppError> {
    Ok(Html(templates::SignupTemplate::default().render()?))
}

#[derive(Deserialize)]
struct SignupForm {
    email: String,
    password: String,
    password_confirm: String,
}

async fn signup(
    State(db): State<Db>,
    mut session: AuthSession,
    headers: HeaderMap,
    Form(form): Form<SignupForm>,
) -> Result<Response, AppError> {
    let email = form.email.trim().to_lowercase();

    if let Some(retry_after) = rate_limit_delay(&db, &headers, "signup", 5, 60 * 60).await? {
        return Ok(rate_limited(
            render_signup_error(
                &email,
                "Too many signup attempts. Please try again later.",
            ),
            retry_after,
        ));
    }

    let rejection = validate_email(&email)
        .and_then(|()| check_confirmation(&form))
        .and_then(|()| password::validate(&form.password))
        .err();

    if let Some(message) = rejection {
        return Ok(render_signup_error(&email, message));
    }

    let hash = password::hash(form.password).await?;

    let Some(user) = store::create_user(&db, &email, &hash).await? else {
        return Ok(render_signup_error(
            &email,
            "Could not create that account. If you already signed up, log in instead.",
        ));
    };

    session
        .login(&user)
        .await
        .map_err(|err| AppError::Session(err.to_string()))?;

    tracing::info!(user.id = user.id, "account created");
    // Straight to the list: `/` immediately redirects logged-in users to
    // `/app` anyway, so this saves the extra round trip.
    Ok(Redirect::to("/app").into_response())
}

fn check_confirmation(form: &SignupForm) -> Result<(), &'static str> {
    match form.password == form.password_confirm {
        true => Ok(()),
        false => Err("Passwords do not match."),
    }
}

/// Re-renders the form with a message. The status stays 200, matching how the
/// todo panel handles a rejected todo: the page worked, the input did not.
///
/// The submitted password is dropped on the floor — a browser would have to
/// send it again, and it must not end up in the HTML we send back.
fn render_signup_error(email: &str, message: &str) -> Response {
    let template = templates::SignupTemplate {
        email: email.to_owned(),
        error: Some(message.to_owned()),
    };

    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(?err, "template render failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- login -------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

#[derive(Deserialize)]
struct AccountQuery {
    changed: Option<String>,
    logged_out: Option<String>,
}

async fn login_form(Query(query): Query<LoginQuery>) -> Result<Html<String>, AppError> {
    Ok(Html(
        templates::LoginTemplate {
            error: None,
            next: query.next.map(|raw| safe_next(&raw)).unwrap_or_default(),
        }
        .render()?,
    ))
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    next: String,
}

async fn login(
    State(db): State<Db>,
    headers: HeaderMap,
    mut session: AuthSession,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    if let Some(retry_after) = rate_limit_delay(&db, &headers, "login", 20, 15 * 60).await? {
        return Ok(rate_limited(
            render_login_error(&form.next, "Too many login attempts. Please try again later."),
            retry_after,
        ));
    }

    let outcome = session
        .authenticate(super::Credentials {
            email: form.email,
            password: form.password,
        })
        .await
        .map_err(|err| AppError::Session(err.to_string()));

    let user = match outcome {
        Ok(Some(user)) => user,
        Ok(None) => {
            tracing::warn!("login refused");
            return Ok(render_login_error(&form.next, BAD_CREDENTIALS));
        }
        Err(err) => return Err(err),
    };

    // Always rotate the session id, never reuse it.
    //
    // axum-login's own `login` only calls `cycle_id` when the session carries
    // no auth hash, and its `logout` deletes the stored record without clearing
    // the in-memory hash — so switching accounts without this line would keep
    // the previous session id, which is textbook session fixation. Rotating
    // unconditionally is cheap and has no downside.
    session
        .session
        .cycle_id()
        .await
        .map_err(|err| AppError::Session(err.to_string()))?;

    session
        .login(&user)
        .await
        .map_err(|err| AppError::Session(err.to_string()))?;

    tracing::info!(user.id = user.id, "login succeeded");
    Ok(Redirect::to(&safe_next(&form.next)).into_response())
}

fn render_login_error(next: &str, message: &str) -> Response {
    let template = templates::LoginTemplate {
        error: Some(message.to_owned()),
        next: safe_next(next),
    };

    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(?err, "template render failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- logout ------------------------------------------------------------------

async fn logout(mut session: AuthSession) -> Result<Response, AppError> {
    let who = session
        .logout()
        .await
        .map_err(|err| AppError::Session(err.to_string()))?;

    if let Some(User { id, .. }) = who {
        tracing::info!(user.id = id, "logout");
    }

    Ok(Redirect::to("/login").into_response())
}

// --- account: change password, log out everywhere --------------------------

async fn account_form(
    State(db): State<Db>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<AccountQuery>,
) -> Result<Html<String>, AppError> {
    let sessions = store::count_sessions_for_user(&db, user.id).await?;
    let notice = if query.changed.as_deref() == Some("1") {
        Some("Your password was changed. Other devices were signed out.".to_owned())
    } else if query.logged_out.as_deref() == Some("1") {
        Some("Other devices were signed out. This device remains signed in.".to_owned())
    } else {
        None
    };
    Ok(Html(
        templates::AccountTemplate {
            email: user.email,
            sessions,
            error: None,
            notice,
        }
        .render()?,
    ))
}

#[derive(Deserialize)]
struct PasswordForm {
    current_password: String,
    new_password: String,
    new_password_confirm: String,
}

async fn change_password(
    State(db): State<Db>,
    mut session: AuthSession,
    headers: HeaderMap,
    CurrentUser(user): CurrentUser,
    Form(form): Form<PasswordForm>,
) -> Result<Response, AppError> {
    if let Some(retry_after) =
        rate_limit_delay(&db, &headers, "password-change", 10, 60 * 60).await?
    {
        return Ok(rate_limited(
            render_account_error(&user, "Too many password changes. Please try again later."),
            retry_after,
        ));
    }
    // The cheap checks come first. Telling someone their two new passwords
    // differ should not cost a 50ms Argon2 verification.
    let rejection = match check_new_password(&form) {
        Err(message) => Err(message),
        Ok(()) => check_current_password(&db, &user, &form.current_password).await,
    };

    if let Err(message) = rejection {
        return Ok(render_account_error(&user, &message));
    }

    let hash = password::hash(form.new_password).await?;
    if !store::set_password(&db, user.id, &hash).await? {
        return Ok(render_account_error(
            &user,
            "That account no longer exists.",
        ));
    }

    // The stored hash is what `session_auth_hash` is derived from, so from here
    // on every session made under the old password fails its check. This one is
    // re-issued so the person who just changed it stays signed in on this
    // device - and only this device.
    //
    // `refreshed` matters. The `user` we were handed still carries the auth
    // hash of the *old* password, and issuing a session from it would store a
    // value that no longer matches what the next request computes - which signs
    // the person straight back out again.
    let refreshed = User::new(user.id, user.email.clone(), &hash);

    session
        .session
        .cycle_id()
        .await
        .map_err(raw_session_error)?;
    session.login(&refreshed).await.map_err(session_error)?;

    tracing::info!(user.id = user.id, "password changed");
    Ok(Redirect::to("/account/password?changed=1").into_response())
}

/// Checks the two new-password fields. Kept apart from the current-password
/// check so the visitor is told about a mismatch before we spend 50ms of Argon2.
fn check_new_password(form: &PasswordForm) -> Result<(), String> {
    if form.new_password != form.new_password_confirm {
        return Err("New passwords do not match.".to_owned());
    }
    password::validate(&form.new_password).map_err(str::to_owned)
}

/// Verifies the password currently on the account.
///
/// This is the step that stops a walk-up attacker using an unlocked browser to
/// take over the account. The email alone is not enough.
async fn check_current_password(db: &Db, user: &User, current: &str) -> Result<(), String> {
    let stored = crate::db::opt_query(
        db,
        "select password_hash from users where id = ?1",
        vec![crate::db::pi(user.id)],
        |row| crate::db::rs(row, 0),
    )
    .await
    .map_err(|err| err.to_string())?
    .ok_or_else(|| "That account no longer exists.".to_owned())?;

    match password::verify(current.to_owned(), stored).await {
        true => Ok(()),
        false => Err("Current password is incorrect.".to_owned()),
    }
}

/// Ends every session for this account, including the one making the request.
///
/// The current session is re-issued first so the visitor keeps this device;
/// everything else is deleted, which is the entire point of the button.
async fn logout_everywhere(
    State(db): State<Db>,
    mut session: AuthSession,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    session
        .session
        .cycle_id()
        .await
        .map_err(raw_session_error)?;
    session.login(&user).await.map_err(session_error)?;

    let removed = store::delete_sessions_for_user(&db, user.id).await?;
    tracing::info!(user.id = user.id, removed, "logged out everywhere");

    Ok(Redirect::to("/account/password?logged_out=1").into_response())
}

fn render_account_error(user: &User, message: &str) -> Response {
    // The session count is not re-read here on purpose: an error page should
    // not need the database a second time, and the number is only decoration.
    let template = templates::AccountTemplate {
        email: user.email.clone(),
        sessions: 0,
        error: Some(message.to_owned()),
        notice: None,
    };

    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(?err, "template render failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn rate_limited(mut response: Response, retry_after: u64) -> Response {
    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&retry_after.to_string()).expect("numeric Retry-After is valid"),
    );
    response
}

async fn rate_limit_delay(
    db: &Db,
    headers: &HeaderMap,
    scope: &str,
    limit: i64,
    window_seconds: i64,
) -> Result<Option<u64>, AppError> {
    let Some(address) = super::middleware::rate_limit_address(headers) else {
        return Ok(None);
    };
    let retry_after = super::allow_auth_attempt(db, scope, &address, limit, window_seconds).await?;
    if let Some(seconds) = retry_after {
        tracing::warn!(scope, retry_after = seconds, "auth request rate limited");
    }
    Ok(retry_after)
}

fn session_error(err: axum_login::Error<AuthBackend>) -> AppError {
    AppError::Session(err.to_string())
}

/// `cycle_id` comes from tower-sessions itself rather than axum-login, so it
/// has its own error type.
fn raw_session_error(err: axum_login::tower_sessions::session::Error) -> AppError {
    AppError::Session(err.to_string())
}

// --- router ------------------------------------------------------------------

/// The unauthenticated half of the app, mounted without any guard.
///
/// Takes the database connection and applies it with `with_state`, so what
/// comes back is a plain `Router` that `src/app.rs` can merge with the todo
/// routes. The merge itself is what places the session layer above them.
///
/// Callers merge this in and add the session layer on top; `src/app.rs` shows
/// the whole arrangement.
pub fn routes(db: Db) -> Router {
    Router::new()
        .route("/signup", get(signup_form).post(signup))
        .route("/login", get(login_form).post(login))
        // POST only. A GET logout would let a third-party page end someone's
        // session with an `<img src>`.
        .route("/logout", post(logout))
        .with_state(db)
}

/// The pages that need a session but are about the account rather than the
/// list, so `todo::build_router` does not own them.
///
/// They are guarded here rather than there because that router is about todos.
/// The guard is the same middleware either way — there is one login check in
/// the app, not two that could drift apart.
pub fn account_routes(db: Db) -> Router {
    Router::new()
        .route("/account/password", get(account_form).post(change_password))
        .route("/account/logout-everywhere", post(logout_everywhere))
        .route_layer(axum::middleware::from_fn(super::require_auth))
        .with_state(db)
}

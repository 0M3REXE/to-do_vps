//! # HTTP-layer tests — the account page
//!
//! The account page is where a signed-in visitor changes their password and
//! logs out everywhere. Both actions must keep *this* session alive while
//! destroying the others, so several tests carry two clients holding two
//! sessions for the same account.

mod common;

use axum::http::StatusCode;
use common::{Client, db, make_user};
use practice::auth::password;
use practice::db::{pi, rs};

/// Two independent sessions on the same freshly created account, plus its id.
async fn two_sessions(database: &practice::db::Db, email: &str) -> (Client, Client, i64) {
    let uid = make_user(database, email).await;
    let a = Client::for_email(database, email).await;
    let b = Client::for_email(database, email).await;
    (a, b, uid)
}

// --- the page itself ---

#[tokio::test]
async fn the_account_page_requires_a_session() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, destination) = client.get_locating(&database, "/account/password").await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));
}

/// An htmx visitor gets `HX-Redirect`, not a 303 — the swap target would
/// otherwise fill up with a whole login page.
#[tokio::test]
async fn an_htmx_visitor_gets_an_hx_redirect() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, _, headers) = client.get_htmx(&database, "/account/password").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("hx-redirect").map(|v| v.to_str().unwrap_or_default()),
        Some("/login")
    );
}

#[tokio::test]
async fn the_account_page_shows_the_email() {
    let database = db().await;
    let mut client = Client::for_email(&database, "owner@example.com").await;

    let (status, body) = client.get(&database, "/account/password").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("owner@example.com"));
}

// --- change password ---

#[tokio::test]
async fn changing_the_password_needs_the_current_one() {
    let database = db().await;
    let email = "changer@example.com";
    let (mut client, _, _) = two_sessions(&database, email).await;

    let (status, body) = client
        .post_form(
            &database,
            "/account/password",
            &[
                ("current_password", "nope-wrong"),
                ("new_password", &(common::GOOD_PASSWORD.to_owned() + "2")),
                ("new_password_confirm", &(common::GOOD_PASSWORD.to_owned() + "2")),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::OK, "a refused change re-renders");
    assert!(body.contains("Current password is incorrect"));
    assert!(!body.contains(common::GOOD_PASSWORD), "no password is echoed");
}

#[tokio::test]
async fn the_new_password_has_to_match_its_confirmation() {
    let database = db().await;
    let email = "matcher@example.com";
    let (mut client, _, _) = two_sessions(&database, email).await;

    let (status, body) = client
        .post_form(
            &database,
            "/account/password",
            &[
                ("current_password", common::GOOD_PASSWORD),
                ("new_password", &(common::GOOD_PASSWORD.to_owned() + "2")),
                ("new_password_confirm", "not the same"),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("do not match"));
}

#[tokio::test]
async fn the_new_password_must_satisfy_the_policy() {
    let database = db().await;
    let email = "policy@example.com";
    let (mut client, _, _) = two_sessions(&database, email).await;

    let (status, body) = client
        .post_form(
            &database,
            "/account/password",
            &[
                ("current_password", common::GOOD_PASSWORD),
                ("new_password", "tiny"),
                ("new_password_confirm", "tiny"),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("at least 8"));
}

/// The heart of the page: the change happens, this device stays signed in, and
/// every other session stops being valid.
#[tokio::test]
async fn a_successful_change_logs_out_everywhere_but_here() {
    let database = db().await;
    let email = "victor@example.com";
    let (mut a, _, uid) = two_sessions(&database, email).await;
    let new_password = common::GOOD_PASSWORD.to_owned() + "2";

    let (status, destination) = a
        .post_form_locating(
            &database,
            "/account/password",
            &[
                ("current_password", common::GOOD_PASSWORD),
                ("new_password", &new_password.clone()),
                ("new_password_confirm", &new_password.clone()),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/account/password?changed=1"));

    // The stored hash now verifies against the new password.
    let stored_hash: String = practice::db::opt_query(
        &database,
        "select password_hash from users where id = ?1",
        vec![pi(uid)],
        |row| rs(row, 0),
    )
    .await
    .unwrap()
    .expect("the account survives");
    assert!(
        password::verify(new_password.clone(), stored_hash).await,
        "the stored hash must verify against the new password"
    );

    // This device is still signed in.
    let (status, _) = a.get(&database, "/app").await;
    assert_eq!(status, StatusCode::OK);
}

/// The other side of the same transaction: a session that was live before the
/// change must not survive it.
#[tokio::test]
async fn every_other_session_dies_when_the_password_changes() {
    let database = db().await;
    let email = "forgotten@example.com";
    let (mut a, mut b, _) = two_sessions(&database, email).await;

    a.post_form(
        &database,
        "/account/password",
        &[
            ("current_password", common::GOOD_PASSWORD),
            ("new_password", &(common::GOOD_PASSWORD.to_owned() + "2")),
            ("new_password_confirm", &(common::GOOD_PASSWORD.to_owned() + "2")),
        ],
    )
    .await;

    // B's cookie now points at a session whose auth hash no longer matches.
    let (status, destination) = b.get_locating(&database, "/app").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));
}

#[tokio::test]
async fn the_old_password_stops_working_once_the_password_changes() {
    let database = db().await;
    let email = "switch@example.com";
    let (mut client, _, _) = two_sessions(&database, email).await;

    client
        .post_form(
            &database,
            "/account/password",
            &[
                ("current_password", common::GOOD_PASSWORD),
                ("new_password", &(common::GOOD_PASSWORD.to_owned() + "2")),
                ("new_password_confirm", &(common::GOOD_PASSWORD.to_owned() + "2")),
            ],
        )
        .await;

    // The old credential is refused, with the same message as any other
    // failure — no hint that the account exists.
    let mut old_browser = Client::anonymous();
    let (status, body) = old_browser
        .post_form(
            &database,
            "/login",
            &[("email", email), ("password", common::GOOD_PASSWORD), ("next", "")],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Invalid email or password"));
}

// --- log out everywhere ---

#[tokio::test]
async fn logging_out_everywhere_keeps_this_device_in() {
    let database = db().await;
    let email = "everywhere@example.com";
    let (mut a, mut b, _) = two_sessions(&database, email).await;

    let (status, destination) = a
        .post_form_locating(&database, "/account/logout-everywhere", &[])
        .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/account/password"));

    // A is still signed in...
    let (status, _) = a.get(&database, "/app").await;
    assert_eq!(status, StatusCode::OK);

    // ...but B is not.
    let (status, destination) = b.get_locating(&database, "/app").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));
}

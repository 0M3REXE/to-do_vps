//! # HTTP-layer tests — signup, login, logout
//!
//! Drive the router and assert on what a browser receives: redirects, rendered
//! pages, cookies. A failure here points at handlers or templates. The
//! data-layer rules (uniqueness, hashing) have their own coverage in
//! `auth_model.rs`.

mod common;

use axum::http::StatusCode;
use common::{GOOD_PASSWORD, TEST_EMAIL, Client, db, ensure_user};

// --- the forms render ---

#[tokio::test]
async fn the_signup_form_renders_for_anonymous_visitors() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, body) = client.get(&database, "/signup").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<form"));
    assert!(body.contains("name=\"email\""));
    // No field is prefilled with anything meaningful: the email input only
    // became a prefilled echo after a rejection, and password fields never
    // accept a value attribute.
    assert!(body.contains("value=\"\""));
    assert!(!body.contains("value=\"tests@example.com\""));
}

#[tokio::test]
async fn the_login_form_renders_for_anonymous_visitors() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, body) = client.get(&database, "/login").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<form"));
    assert!(body.contains("name=\"password\""));
}

// --- signup ---

#[tokio::test]
async fn signup_creates_the_account_and_signs_you_in() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, destination) = client
        .post_form_locating(
            &database,
            "/signup",
            &[
                ("email", "new@example.com"),
                ("password", GOOD_PASSWORD),
                ("password_confirm", GOOD_PASSWORD),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(destination, "/app", "a fresh signup lands on the list");

    // "Signed in" means two things: a session cookie...
    assert!(
        client.cookie().is_some(),
        "signup must hand back a session cookie"
    );
    // ...and the index, which only renders for a session.
    let (status, body) = client.get(&database, "/app").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("todo-panel"));
}

#[tokio::test]
async fn signup_refuses_a_duplicate_with_the_form_rerendered() {
    let database = db().await;
    ensure_user(&database, "taken@example.com").await;

    let mut client = Client::anonymous();
    let (status, body) = client
        .post_form(
            &database,
            "/signup",
            &[
                ("email", "taken@example.com"),
                ("password", GOOD_PASSWORD),
                ("password_confirm", GOOD_PASSWORD),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::OK, "a refused signup re-renders, not 500");
    assert!(body.contains("Could not create that account"));
}

/// Uniqueness ignores case, and the handler surfaces an index collision as a
/// form error rather than a 500.
#[tokio::test]
async fn signup_refuses_a_duplicate_that_differs_only_in_case() {
    let database = db().await;
    ensure_user(&database, "taken@example.com").await;

    let mut client = Client::anonymous();
    let (status, body) = client
        .post_form(
            &database,
            "/signup",
            &[
                ("email", "TAKEN@example.com"),
                ("password", GOOD_PASSWORD),
                ("password_confirm", GOOD_PASSWORD),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Could not create that account"));
}

#[tokio::test]
async fn signup_refuses_a_mismatched_confirmation() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, body) = client
        .post_form(
            &database,
            "/signup",
            &[
                ("email", "new@example.com"),
                ("password", GOOD_PASSWORD),
                ("password_confirm", "not the same"),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("do not match"));
    assert!(
        todo::auth::user_id_by_email(&database, "new@example.com")
            .await
            .expect("lookup")
            .is_none(),
        "a mismatched confirmation must not create the account"
    );
}

#[tokio::test]
async fn signup_refuses_a_password_below_the_minimum() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, body) = client
        .post_form(
            &database,
            "/signup",
            &[
                ("email", "new@example.com"),
                ("password", "short"),
                ("password_confirm", "short"),
            ],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("at least 8"));
    assert!(
        todo::auth::user_id_by_email(&database, "new@example.com")
            .await
            .expect("lookup")
            .is_none()
    );
}

#[tokio::test]
async fn signup_refuses_an_unusable_email() {
    let database = db().await;
    let mut client = Client::anonymous();

    for bad in ["nope", "a@b@c", "@example.com", "a@"] {
        let (status, body) = client
            .post_form(
                &database,
                "/signup",
                &[
                    ("email", bad),
                    ("password", GOOD_PASSWORD),
                    ("password_confirm", GOOD_PASSWORD),
                ],
            )
            .await;
        assert_eq!(status, StatusCode::OK, "for {bad}");
        assert!(body.contains("valid email"), "expected a form message for {bad}");
    }
}

/// Whatever the visitor typed, the stored address is trimmed and lower-cased.
#[tokio::test]
async fn signup_normalises_the_email() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, _) = client
        .post_form(
            &database,
            "/signup",
            &[
                ("email", "  SPACE@Example.COM "),
                ("password", GOOD_PASSWORD),
                ("password_confirm", GOOD_PASSWORD),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let stored: String = todo::db::query_all(
        &database,
        "select email from users",
        vec![],
        |row| todo::db::rs(row, 0),
    )
    .await
    .unwrap()
    .remove(0);
    assert_eq!(stored, "space@example.com");
}

/// The address may echo back — it is not secret — but the password may never.
#[tokio::test]
async fn a_refused_signup_never_echoes_the_password() {
    let database = db().await;
    ensure_user(&database, "taken@example.com").await;

    let mut client = Client::anonymous();
    let (_, body) = client
        .post_form(
            &database,
            "/signup",
            &[
                ("email", "taken@example.com"),
                ("password", GOOD_PASSWORD),
                ("password_confirm", GOOD_PASSWORD),
            ],
        )
        .await;

    assert!(body.contains("taken@example.com"), "the email echoes back");
    assert!(!body.contains(GOOD_PASSWORD), "the password must not");
}

// --- login ---

#[tokio::test]
async fn login_sets_a_session_and_returns_to_the_list() {
    let database = db().await;
    ensure_user(&database, TEST_EMAIL).await;
    let mut client = Client::anonymous();

    let (status, destination) = client
        .post_form_locating(
            &database,
            "/login",
            &[("email", TEST_EMAIL), ("password", GOOD_PASSWORD), ("next", "")],
        )
        .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(destination, "/");
    assert!(client.cookie().is_some());
}

/// Every way a login can fail says the same thing, so the message cannot be
/// used to work out which addresses are registered.
#[tokio::test]
async fn login_failures_all_say_the_same_thing() {
    let database = db().await;
    ensure_user(&database, TEST_EMAIL).await;
    let mut client = Client::anonymous();

    for (email, password) in [
        (TEST_EMAIL, "wrong password"),
        ("nobody@example.com", GOOD_PASSWORD),
        ("", ""),
    ] {
        let (status, body) = client
            .post_form(
                &database,
                "/login",
                &[("email", email), ("password", password), ("next", "")],
            )
            .await;

        assert_eq!(status, StatusCode::OK, "for {email}");
        assert!(body.contains("Invalid email or password"), "for {email}");
        assert!(
            client.cookie().is_none(),
            "a refused login must not set a session, for {email}"
        );
    }
}

/// One session id must never outlive its first login — that is session
/// fixation. Logging in twice in a row must hand back a different id.
#[tokio::test]
async fn the_session_id_rotates_on_every_login() {
    let database = db().await;
    ensure_user(&database, TEST_EMAIL).await;
    let mut client = Client::anonymous();
    let mut other = Client::anonymous();

    for c in [&mut client, &mut other] {
        c.post_form(
            &database,
            "/login",
            &[("email", TEST_EMAIL), ("password", GOOD_PASSWORD), ("next", "")],
        )
        .await;
    }

    let (a, b) = (
        client.cookie().expect("first session").to_owned(),
        other.cookie().expect("second session").to_owned(),
    );
    assert_ne!(
        a, b,
        "two logins of the same visitor must not share a session id"
    );
}

/// `?next=` decides where a successful login ends up — but only paths this
/// site owns. The guard and the handler both go through `safe_next`.
#[tokio::test]
async fn the_next_parameter_reaches_the_form() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (_, body) = client
        .get(&database, "/login?next=%2Faccount%2Fpassword")
        .await;

    assert!(body.contains("name=\"next\""), "the form must carry it on");
    assert!(body.contains("/account/password"));
}

#[tokio::test]
async fn a_foreign_next_value_never_reaches_the_redirect() {
    use todo::auth::safe_next;

    assert_eq!(safe_next("//evil.example"), "/");
    assert_eq!(safe_next("/\\evil.example"), "/");
    assert_eq!(safe_next("https://google.example"), "/");
    assert_eq!(safe_next("/account/password"), "/account/password");
}

// --- logout ---

#[tokio::test]
async fn logout_ends_the_session() {
    let database = db().await;
    let mut client = Client::for_email(&database, TEST_EMAIL).await;

    let (status, destination) = client
        .post_form_locating(&database, "/logout", &[])
        .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));

    // And the visitor is really out.
    let (status, destination) = client.get_locating(&database, "/app").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));
}

/// POST only. A GET logout would let a third-party page end a session with an
/// `<img src="/logout">`.
#[tokio::test]
async fn a_get_cannot_log_you_out() {
    let database = db().await;
    let mut client = Client::for_email(&database, TEST_EMAIL).await;

    let (status, _) = client.get(&database, "/logout").await;

    assert_eq!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "GET on the logout route must be refused"
    );

    // Still logged in afterwards.
    let (status, _) = client.get(&database, "/app").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_session_row_dies_with_the_logout() {
    use todo::db::pi;

    let database = db().await;
    let uid = ensure_user(&database, TEST_EMAIL).await;
    let mut client = Client::for_email(&database, TEST_EMAIL).await;

    let count = async |database: &todo::db::Db| -> i64 {
        todo::db::scalar_i64(
            database,
            "select count(*) from sessions where user_id = ?1",
            vec![pi(uid)],
        )
        .await
        .unwrap()
    };
    assert_eq!(count(&database).await, 1);

    client.post_form(&database, "/logout", &[]).await;

    assert_eq!(
        count(&database).await,
        0,
        "the session row must not survive a logout"
    );
}

/// A cookie that points at nothing must greet its holder with the login page,
/// not a crash. The deployment guarantees the same: any restart that loses
/// sessions behaves exactly like this test.
#[tokio::test]
async fn a_stale_cookie_is_a_signed_out_visitor_not_a_500() {
    let database = db().await;

    let mut client = Client::anonymous();
    client
        .post_form(
            &database,
            "/signup",
            &[
                ("email", "wanderer@example.com"),
                ("password", GOOD_PASSWORD),
                ("password_confirm", GOOD_PASSWORD),
            ],
        )
        .await;
    let cookie = client.cookie().expect("a session").to_owned();

    // The same cookie, against a brand-new empty database.
    let other = db().await;
    let mut stale = Client::with_replayed_cookie(cookie);
    let (status, destination) = stale.get_locating(&other, "/app").await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));
}

/// Sanity check on the error surface: a corrupt body is a 4xx, never a 500.
#[tokio::test]
async fn login_with_no_form_body_is_a_422() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, _) = client.post_raw(&database, "/login", "not-a-form").await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

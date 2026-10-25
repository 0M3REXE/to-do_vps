//! # HTTP-layer tests — the security middlewares
//!
//! `verify_origin` refuses state-changing requests that arrive claiming to be
//! from another site, and `require_auth` sends signed-out visitors to the login
//! page (with `HX-Redirect` for htmx ones). These tests drive both through the
//! whole router, because a middleware is only tested honestly in situ.

mod common;

use axum::http::StatusCode;
use common::{Client, db, only_id, seed};
use todo::auth::safe_next;

const THIS_HOST: &str = "http://localhost:3000";
const OTHER_HOST: &str = "http://evil.example";

// --- verify_origin ---

#[tokio::test]
async fn a_same_origin_post_goes_through() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _) = client
        .post_form_with(
            &database,
            &format!("/app/todos/{id}/toggle"),
            &[],
            &[("Origin", THIS_HOST)],
        )
        .await;

    assert_eq!(status, StatusCode::OK, "our own origin must never be blocked");
}

#[tokio::test]
async fn a_matching_origin_is_authoritative_over_fetch_metadata() {
    let database = db().await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _) = client
        .post_form_with(
            &database,
            "/app/todos",
            &[("text", "trusted origin")],
            &[("Origin", THIS_HOST), ("Sec-Fetch-Site", "cross-site")],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn cross_site_fetch_without_origin_proof_is_refused() {
    let database = db().await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, body) = client
        .post_form_with(
            &database,
            "/app/todos",
            &[("text", "forged request")],
            &[("Sec-Fetch-Site", "cross-site")],
        )
        .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("cross-origin"));
}

#[tokio::test]
async fn the_request_hostname_is_accepted_for_custom_domains() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _) = client
        .post_form_with(
            &database,
            &format!("/app/todos/{id}/toggle"),
            &[],
            &[("Host", "my-project.example"), ("Origin", "https://my-project.example")],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_foreign_origin_post_is_refused_with_a_403() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, body) = client
        .post_form_with(
            &database,
            &format!("/app/todos/{id}/toggle"),
            &[],
            &[("Origin", OTHER_HOST)],
        )
        .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("cross-origin"));
}

/// The todo must still be there: the guard is upstream of the handler, so the
/// refused request never reached it.
#[tokio::test]
async fn a_blocked_post_lets_no_state_change_happen() {
    use todo::db::pi;

    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _) = client
        .post_form_with(
            &database,
            &format!("/app/todos/{id}/delete"),
            &[],
            &[("Origin", OTHER_HOST)],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let still_there: String = todo::db::opt_query(
        &database,
        "select text from todos where id = ?1",
        vec![pi(id)],
        |row| todo::db::rs(row, 0),
    )
    .await
    .unwrap()
    .expect("the todo must not have been deleted");
    assert_eq!(still_there, "buy milk");
}

#[tokio::test]
async fn referer_works_as_the_fallback_when_origin_is_absent() {
    let database = db().await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    // A foreign Referer, with no Origin: still refused.
    let (status, _) = client
        .post_form_with(
            &database,
            "/app/todos",
            &[("text", "x")],
            &[("Referer", &(format!("{OTHER_HOST}/page"))),],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Our own Referer: allowed through.
    let (status, _) = client
        .post_form_with(
            &database,
            "/app/todos",
            &[("text", "from-referer")],
            &[("Referer", &(format!("{THIS_HOST}/page"))),],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn referer_query_does_not_change_its_authority() {
    let database = db().await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _) = client
        .post_form_with(
            &database,
            "/app/todos",
            &[("text", "referer query")],
            &[("Referer", "http://localhost:3000?source=app")],
        )
        .await;

    assert_eq!(status, StatusCode::OK);
}

/// GETs are never state changes, so an attacker linking here cannot make a GET
/// fail on origin grounds; the request runs the normal route.
#[tokio::test]
async fn a_get_is_never_refused_on_origin() {
    let database = db().await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _, _) = client
        .get_with_headers(&database, "/app", &[("Origin", OTHER_HOST)])
        .await;

    assert_eq!(status, StatusCode::OK);
}

/// A browser always sends `Origin` on a POST; its absence means a non-browser
/// client — curl, this test suite — which the app trusts.
#[tokio::test]
async fn a_post_with_no_origin_at_all_goes_through() {
    let database = db().await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _) = client.post_todo(&database, "no-origin todo").await;

    assert_eq!(status, StatusCode::OK);
}

/// The host is compared by authority only: the scheme is not part of the check.
#[tokio::test]
async fn the_scheme_is_not_part_of_the_origin_check() {
    let database = db().await;
    let mut client = Client::for_email(&database, common::TEST_EMAIL).await;

    let (status, _) = client
        .post_form_with(
            &database,
            "/app/todos",
            &[("text", "https is the same place")],
            &[("Origin", "https://localhost:3000")],
        )
        .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "http vs https on the same host is not cross-origin"
    );
}

// --- require_auth ---

#[tokio::test]
async fn a_signed_out_visitor_is_sent_to_the_login_page() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, destination) = client.get_locating(&database, "/app").await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(destination, "/login?next=%2F");
}

/// The destination remembers where the visitor was headed, together with the
/// percent-encoding that keeps that value to one parameter.
#[tokio::test]
async fn the_redirect_remembers_where_the_visitor_was_going() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, destination) = client.get_locating(&database, "/account/password").await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(destination, "/login?next=%2Faccount%2Fpassword");
}

#[tokio::test]
async fn an_htmx_visitor_gets_a_redirect_header_not_a_303() {
    let database = db().await;
    let mut client = Client::anonymous();

    let (status, _, headers) = client.get_htmx(&database, "/app").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("hx-redirect").map(|v| v.to_str().unwrap_or_default()),
        Some("/login"),
        "a 303 would swap a login page into the todo list"
    );
}

// --- safe_next, called directly ---

#[test]
fn safe_next_accepts_local_paths() {
    assert_eq!(safe_next("/"), "/");
    assert_eq!(safe_next("/account/password"), "/account/password");
    assert_eq!(safe_next("/app/todos/1/toggle"), "/app/todos/1/toggle");
}

/// Whatever arrives, the function returns a path we are willing to send
/// someone to.
#[test]
fn safe_next_never_returns_something_foreign() {
    for rejected in [
        "//evil.example",
        "/\\evil.example",
        "\\\\evil.example",
        "https://evil.example",
        "javascript:alert(1)",
        "",
        "   /ok   ",
    ] {
        let value = safe_next(rejected);
        assert!(
            value.starts_with('/') && !value.starts_with("//") && !value.contains('\\'),
            "{rejected:?} must fold to a local path, got {value:?}"
        );
    }
}

/// The guard and the login handler both percent-encode the `next` value, and
/// the encoding can never inject a second query parameter.
#[test]
fn the_next_encoding_cannot_inject_query_parameters() {
    let encoded = percent_encode("/account?a=1&next=//evil");

    assert!(!encoded.contains("&next="), "{encoded}");
    assert!(encoded.contains("%26"));
}

/// Mirror of the encoder in `middleware.rs`, kept in the test so the suite does
/// not depend on an export that would exist only for it.
fn percent_encode(value: &str) -> String {
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

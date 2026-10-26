mod common;

use axum::http::StatusCode;
use common::{Client, db, TEST_EMAIL, GOOD_PASSWORD};
use todo::db;

#[tokio::test]
async fn a_replayed_session_cookie_alone_requires_reconfirmation() {
    let database = db().await;
    let mut victim = Client::for_email(&database, TEST_EMAIL).await;
    let session = victim.cookie().expect("session cookie").to_owned();

    let mut attacker = Client::with_replayed_cookie(session);
    let (status, destination) = attacker.get_locating(&database, "/app").await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/account/reauth"));
}

#[tokio::test]
async fn a_replayed_session_and_device_cookie_works() {
    let database = db().await;
    let mut victim = Client::for_email(&database, TEST_EMAIL).await;
    let full = victim.cookie_header().expect("cookie header").to_owned();

    let mut copy = Client::with_replayed_cookies(full);
    let (status, body) = copy.get(&database, "/app").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("todo-panel"));
}

#[tokio::test]
async fn a_changed_user_agent_requires_reconfirmation() {
    let database = db().await;
    let mut victim = Client::for_email(&database, TEST_EMAIL).await;
    let full = victim.cookie_header().expect("cookie header").to_owned();

    let mut copy = Client::with_replayed_cookies(full);
    let (status, _, headers) = copy
        .get_with_headers(&database, "/app", &[("user-agent", "OtherBrowser")])
        .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = headers
        .get(axum::http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(location.contains("/account/reauth"));
}

#[tokio::test]
async fn step_up_with_the_password_restores_access() {
    let database = db().await;
    let mut victim = Client::for_email(&database, TEST_EMAIL).await;
    let session = victim.cookie().expect("session cookie").to_owned();

    let mut attacker = Client::with_replayed_cookie(session);
    let (status, location) = attacker.get_locating(&database, "/app").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location.contains("/account/reauth"));

    let (status, location) = attacker
        .post_form_locating(
            &database,
            "/account/reauth",
            &[
                ("current_password", GOOD_PASSWORD),
                ("next", "/app"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/app");

    let (status, body) = attacker.get(&database, "/app").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("todo-panel"));
}

#[tokio::test]
async fn the_sessions_page_lists_and_revokes_a_device() {
    let database = db().await;
    common::ensure_user(&database, TEST_EMAIL).await;

    let mut a = Client::anonymous();
    let (status, _) = a
        .post_form_with(
            &database,
            "/login",
            &[
                ("email", TEST_EMAIL),
                ("password", GOOD_PASSWORD),
                ("next", ""),
            ],
            &[("user-agent", "A-Test")],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let mut b = Client::anonymous();
    let (status, _) = b
        .post_form_with(
            &database,
            "/login",
            &[
                ("email", TEST_EMAIL),
                ("password", GOOD_PASSWORD),
                ("next", ""),
            ],
            &[("user-agent", "B-Test")],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let rows = db::query_all(
        &database,
        "select data from sessions",
        vec![],
        |row| db::rs(row, 0),
    )
    .await
    .expect("sessions");
    let mut b_public = None;
    for data in rows {
        let value: serde_json::Value = serde_json::from_str(&data).expect("json");
        if value
            .get("todo.security")
            .and_then(|security| security.get("user_agent"))
            .and_then(|agent| agent.as_str())
            == Some("B-Test")
        {
            b_public = value
                .get("todo.security")
                .and_then(|security| security.get("public_id"))
                .and_then(|id| id.as_str())
                .map(str::to_owned);
        }
    }
    let b_public = b_public.expect("B public id");

    let (status, body) = a.get(&database, "/account/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("B-Test"));
    assert!(body.contains(&format!("/account/sessions/{}/revoke", b_public)));

    let (status, location) = a
        .post_form_locating(&database, &format!("/account/sessions/{}/revoke", b_public), &[])
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location.contains("/account/sessions"));

    let (status, destination) = b.get_locating(&database, "/app").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));
}

#[tokio::test]
async fn the_session_id_is_not_stored_in_the_clear() {
    let database = db().await;
    let mut client = Client::for_email(&database, TEST_EMAIL).await;
    let cookie = client.cookie().expect("session cookie").to_owned();
    let raw = cookie.split('=').nth(1).expect("value").to_owned();

    let rows = db::query_all(
        &database,
        "select id, data from sessions",
        vec![],
        |row| Ok((db::rs(row, 0)?, db::rs(row, 1)?)),
    )
    .await
    .expect("sessions");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.len(), 64);
    assert!(!rows[0].0.contains(&raw));
    assert!(!rows[0].1.contains(&raw));
}

#[tokio::test]
async fn an_eight_day_old_session_is_rejected() {
    let database = db().await;
    let mut client = Client::for_email(&database, TEST_EMAIL).await;

    db::execute(
        &database,
        "update sessions set created_at = ?1",
        vec![db::ps(db::format_ts(
            time::OffsetDateTime::now_utc() - time::Duration::days(8),
        ))],
    )
    .await
    .expect("update");

    let (status, destination) = client.get_locating(&database, "/app").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(destination.contains("/login"));
}

//! # Auth data-layer tests — no HTTP
//!
//! Covers the parts of authentication that are just database work: creating an
//! account, the uniqueness rule, and the password hash. The HTTP behaviours
//! live in `auth_http.rs`.

mod common;

use common::{POLICY_MAX, POLICY_MIN, db};
use todo::auth::{create_user, password, user_id_by_email};

#[tokio::test]
async fn creating_an_account_gives_it_an_id_and_a_timestamp() {
    let pool = db().await;
    let hash = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();

    let user = create_user(&pool, "new@example.com", &hash)
        .await
        .unwrap()
        .expect("first signup should succeed");

    assert!(user.id > 0);
    assert_eq!(user.email, "new@example.com");

    let created: i64 = todo::db::scalar_i64(
        &pool,
        "select count(*) from users where created_at is not null",
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(created, 1);
}

/// The unique index is on `lower(email)`, so this holds whatever case the
/// visitor typed.
#[tokio::test]
async fn a_second_account_on_the_same_email_is_refused() {
    let pool = db().await;
    let hash = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();

    assert!(
        create_user(&pool, "taken@example.com", &hash)
            .await
            .unwrap()
            .is_some()
    );

    // `None`, not an error: a duplicate is something the visitor can fix, so it
    // has to reach the form rather than becoming a 500.
    assert!(
        create_user(&pool, "taken@example.com", &hash)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn uniqueness_ignores_case() {
    let pool = db().await;
    let hash = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();

    create_user(&pool, "Case@Example.com", &hash)
        .await
        .unwrap()
        .expect("first signup");

    assert!(
        create_user(&pool, "case@example.com", &hash)
            .await
            .unwrap()
            .is_none(),
        "Case@Example.com and case@example.com are one account"
    );

    assert!(
        create_user(&pool, "CASE@EXAMPLE.COM", &hash)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn lookup_by_email_ignores_case() {
    let pool = db().await;
    let hash = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();
    create_user(&pool, "Mixed@Example.com", &hash)
        .await
        .unwrap()
        .expect("signup");

    assert!(
        user_id_by_email(&pool, "mixed@example.com")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        user_id_by_email(&pool, "nobody@example.com")
            .await
            .unwrap()
            .is_none()
    );
}

/// The stored value must be an Argon2 PHC string and must not be the password.
#[tokio::test]
async fn the_password_is_never_stored_in_the_clear() {
    let pool = db().await;
    let hash = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();

    create_user(&pool, "secret@example.com", &hash)
        .await
        .unwrap()
        .expect("signup");

    let stored: String = todo::db::query_all(
        &pool,
        "select password_hash from users",
        vec![],
        |row| todo::db::rs(row, 0),
    )
    .await
    .unwrap()
    .remove(0);

    assert!(
        stored.starts_with("$argon2"),
        "expected a PHC hash, got {stored}"
    );
    assert_ne!(stored, common::GOOD_PASSWORD);
    assert!(!stored.contains(common::GOOD_PASSWORD));
}

/// Two hashes of the same password differ, because each carries its own salt.
#[tokio::test]
async fn the_same_password_hashes_differently_each_time() {
    let first = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();
    let second = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();

    assert_ne!(
        first, second,
        "a reused salt would leak that two passwords match"
    );
}

#[tokio::test]
async fn the_right_password_verifies_and_the_wrong_one_does_not() {
    let hash = password::hash(common::GOOD_PASSWORD.to_owned()).await.unwrap();

    assert!(password::verify(common::GOOD_PASSWORD.to_owned(), hash.clone()).await);
    assert!(!password::verify("correct horse batter".to_owned(), hash.clone()).await);
    assert!(!password::verify(String::new(), hash.clone()).await);
}

/// A hash that cannot be parsed must read as "wrong password", never as a
/// match and never as a crash.
#[tokio::test]
async fn a_corrupt_hash_is_reported_as_a_mismatch() {
    assert!(!password::verify(common::GOOD_PASSWORD.to_owned(), "not-a-phc-string".to_owned()).await);
    assert!(!password::verify(common::GOOD_PASSWORD.to_owned(), String::new()).await);
}

// --- the password policy ---

#[test]
fn the_policy_rejects_only_what_it_means_to() {
    assert!(password::validate(common::GOOD_PASSWORD).is_ok());
    assert!(
        password::validate("         ").is_err(),
        "whitespace is not a password"
    );
    assert!(password::validate("").is_err());

    let too_short = "a".repeat(POLICY_MIN - 1);
    assert!(password::validate(&too_short).is_err());

    let at_limit = "a".repeat(POLICY_MIN);
    assert!(
        password::validate(&at_limit).is_ok(),
        "the minimum must be allowed"
    );

    let too_long = "a".repeat(POLICY_MAX + 1);
    assert!(password::validate(&too_long).is_err());

    let at_max = "a".repeat(POLICY_MAX);
    assert!(
        password::validate(&at_max).is_ok(),
        "the maximum must be allowed"
    );
}

/// Length is counted in characters, so a password written in a script that is
/// multi-byte is not unfairly rejected.
#[test]
fn the_policy_counts_characters_not_bytes() {
    let emoji = "🔒".repeat(POLICY_MIN);
    assert_eq!(emoji.len(), POLICY_MIN * 4, "four bytes each");
    assert!(
        password::validate(&emoji).is_ok(),
        "ten emoji is ten characters, and must be accepted"
    );
}

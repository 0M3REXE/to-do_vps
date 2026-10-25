//! # Data-layer tests — 14, no HTTP
//!
//! Each test builds its own fresh in-memory libSQL database (see
//! `common::db`), migrated from the embedded schema, so tests run in parallel
//! without interfering.
//!
//! Every store call takes a `user_id`, because todos are per-user. The tests
//! below pass one explicitly, because that is the point of the change: the
//! owner is now part of every query.

mod common;

use common::{count, db, done_of, ensure_user, ids, only_id, seed, text_of, user_id};
use practice::db::{pi, ps};
use practice::todo::{MAX_TEXT_LEN, add, flip_done, open_count, remove, sorted};

#[tokio::test]
async fn add_assigns_incrementing_ids_starting_at_one() {
    let pool = db().await;
    let me = user_id(&pool).await;
    add(&pool, me, "first").await.unwrap();
    add(&pool, me, "second").await.unwrap();

    assert_eq!(ids(&pool).await, vec![1, 2]);
}

#[tokio::test]
async fn add_trims_whitespace() {
    let pool = db().await;
    let me = user_id(&pool).await;
    add(&pool, me, "  buy milk  ").await.unwrap();

    assert_eq!(
        text_of(&pool, only_id(&pool).await).await.unwrap(),
        "buy milk"
    );
}

#[tokio::test]
async fn add_rejects_blank_text_without_inserting() {
    let pool = db().await;
    let me = user_id(&pool).await;
    assert!(!add(&pool, me, "").await.unwrap());
    assert!(!add(&pool, me, "   ").await.unwrap());
    assert!(!add(&pool, me, "\t\n ").await.unwrap());
    assert_eq!(count(&pool).await, 0);
}

#[tokio::test]
async fn add_rejects_text_longer_than_the_limit() {
    let pool = db().await;
    let me = user_id(&pool).await;
    let at_limit = "a".repeat(MAX_TEXT_LEN);
    assert!(
        add(&pool, me, &at_limit).await.unwrap(),
        "at the limit must fit"
    );

    let over_limit = "a".repeat(MAX_TEXT_LEN + 1);
    assert!(
        !add(&pool, me, &over_limit).await.unwrap(),
        "one over must not"
    );
    assert_eq!(count(&pool).await, 1);
}

#[tokio::test]
async fn the_length_limit_is_measured_after_trimming() {
    let pool = db().await;
    let me = user_id(&pool).await;
    let padded = format!("{}   ", "a".repeat(MAX_TEXT_LEN));
    assert!(add(&pool, me, &padded).await.unwrap());

    let len: i64 = practice::db::scalar_i64(
        &pool,
        "select length(text) from todos",
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(len as usize, MAX_TEXT_LEN);
}

/// The CHECK constraint is the backstop; this proves the Rust check and the
/// database agree, so a bad todo is a 200 rather than a constraint error.
#[tokio::test]
async fn the_database_rejects_what_the_handler_rejects() {
    let pool = db().await;
    let me = user_id(&pool).await;

    practice::db::execute(
        &pool,
        "insert into todos (text, user_id) values (?1, ?2)",
        vec![ps(String::new()), pi(me)],
    )
    .await
    .expect_err("the CHECK constraint must reject blank text directly");

    practice::db::execute(
        &pool,
        "insert into todos (text, user_id) values (?1, ?2)",
        vec![practice::db::ps("a".repeat(MAX_TEXT_LEN + 1)), pi(me)],
    )
    .await
    .expect_err("the CHECK constraint must reject over-long text directly");
}

/// Needs no database: the template and the handler must advertise the same
/// limit so the browser agrees with the server.
#[test]
fn the_template_advertises_the_same_limit_it_enforces() {
    let panel = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/templates/todo_panel.html"
    ))
    .expect("panel template readable");

    assert!(
        panel.contains(&format!("data-maxlength=\"{MAX_TEXT_LEN}\"")),
        "input must advertise the server's character limit"
    );
}

#[tokio::test]
async fn a_stale_request_against_a_deleted_id_does_not_touch_another_todo() {
    let pool = db().await;
    let me = user_id(&pool).await;
    seed(&pool, &["a", "b"]).await;
    let second = ids(&pool).await[1];
    remove(&pool, me, second).await.unwrap();

    assert!(!flip_done(&pool, me, second).await.unwrap());
    assert!(!remove(&pool, me, second).await.unwrap());

    add(&pool, me, "c").await.unwrap();
    assert!(
        text_of(&pool, second).await.is_none(),
        "the deleted id stayed deleted"
    );
}

#[tokio::test]
async fn toggle_flips_state_and_reports_unknown_ids() {
    let pool = db().await;
    let me = user_id(&pool).await;
    add(&pool, me, "a").await.unwrap();
    let id = only_id(&pool).await;

    assert!(flip_done(&pool, me, id).await.unwrap());
    assert!(done_of(&pool, id).await);

    assert!(flip_done(&pool, me, id).await.unwrap());
    assert!(!done_of(&pool, id).await);

    assert!(!flip_done(&pool, me, 9999).await.unwrap());
}

#[tokio::test]
async fn delete_removes_and_reports_unknown_ids() {
    let pool = db().await;
    let me = user_id(&pool).await;
    seed(&pool, &["a", "b"]).await;
    let all = ids(&pool).await;

    assert!(remove(&pool, me, all[0]).await.unwrap());
    assert_eq!(count(&pool).await, 1);
    assert_eq!(text_of(&pool, all[1]).await.unwrap(), "b");

    assert!(!remove(&pool, me, all[0]).await.unwrap());
}

/// Identity columns never reuse a value, so a new id must be strictly greater
/// than the deleted one.
#[tokio::test]
async fn ids_are_never_reused_after_delete() {
    let pool = db().await;
    let me = user_id(&pool).await;
    seed(&pool, &["a", "b", "c"]).await;
    let all = ids(&pool).await;

    remove(&pool, me, all[1]).await.unwrap();
    assert_eq!(ids(&pool).await, vec![all[0], all[2]]);

    add(&pool, me, "d").await.unwrap();
    let new_id = ids(&pool).await[2];
    assert!(
        new_id > all[1],
        "a deleted id must never be handed out again: {new_id} vs {}",
        all[1]
    );
}

#[tokio::test]
async fn sorted_puts_completed_below_active_within_order() {
    let pool = db().await;
    let me = user_id(&pool).await;
    seed(&pool, &["one", "two", "three", "four"]).await;
    let all = ids(&pool).await;

    flip_done(&pool, me, all[0]).await.unwrap();
    flip_done(&pool, me, all[1]).await.unwrap();

    let order: Vec<String> = practice::db::query_all(
        &pool,
        "select text from todos order by done asc, id asc",
        vec![],
        |row| practice::db::rs(row, 0),
    )
    .await
    .unwrap();
    assert_eq!(order, vec!["three", "four", "one", "two"]);

    assert_eq!(
        sorted(&pool, me)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.text)
            .collect::<Vec<_>>(),
        order
    );
}

#[tokio::test]
async fn sorted_with_nothing_is_empty() {
    let pool = db().await;
    let me = user_id(&pool).await;
    assert!(sorted(&pool, me).await.unwrap().is_empty());
}

// --- ownership ---

/// The whole multi-user guarantee, asserted directly against the store rather
/// than through HTTP, so a bug here is caught even if a handler stops calling
/// one of these functions.
///
/// Every function takes a `user_id`, and every one of them must be scoped by it.
/// If any dropped its filter, `mine` and `theirs` would cross over.
#[tokio::test]
async fn no_store_function_ever_crosses_the_owner_boundary() {
    let pool = db().await;
    let mine = user_id(&pool).await;
    let theirs = ensure_user(&pool, "somebody-else@example.com").await;

    add(&pool, mine, "mine").await.unwrap();
    add(&pool, theirs, "theirs").await.unwrap();
    let their_id = *ids(&pool).await.last().expect("two todos");

    // read: neither the list nor the count may include the other account
    let texts = sorted(&pool, mine)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect::<Vec<_>>();
    assert_eq!(texts, vec!["mine"], "sorted must not cross accounts");
    assert_eq!(
        open_count(&pool, mine).await.unwrap(),
        1,
        "count must be per account"
    );

    // write: their row is invisible, so it reads as missing rather than denied
    assert!(!flip_done(&pool, mine, their_id).await.unwrap());
    assert!(!remove(&pool, mine, their_id).await.unwrap());
    assert!(!done_of(&pool, their_id).await, "the row must be untouched");
    assert_eq!(
        text_of(&pool, their_id).await.as_deref(),
        Some("theirs"),
        "the row must still exist"
    );

    // and it is all still there when they ask
    let theirs_text = sorted(&pool, theirs)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect::<Vec<_>>();
    assert_eq!(theirs_text, vec!["theirs"]);
}

// --- Injection ---

/// Every query is parameterized, so a payload that would otherwise close a
/// string literal is stored as literal text and the schema survives.
#[tokio::test]
async fn sql_injection_payloads_are_stored_as_text() {
    let pool = db().await;
    let payloads = [
        "'; drop table todos; --",
        "1; DELETE FROM todos WHERE 1=1; --",
        "' OR '1'='1",
        "1 UNION SELECT null, version(), null --",
    ];
    seed(&pool, &payloads).await;

    assert_eq!(count(&pool).await, payloads.len() as i64);

    let stored: Vec<String> = practice::db::query_all(
        &pool,
        "select text from todos order by id",
        vec![],
        |row| practice::db::rs(row, 0),
    )
    .await
    .unwrap();
    assert_eq!(
        stored,
        payloads.to_vec(),
        "payloads must round-trip verbatim"
    );

    // The table still exists and holds everything.
    assert_eq!(count(&pool).await, 4);
}

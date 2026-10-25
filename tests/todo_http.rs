//! # HTTP-layer tests — 29
//!
//! Drive the router and assert on what a browser receives: status codes,
//! fragments, and escaping. A failure here points at handlers or templates
//! rather than at SQL.
//!
//! Everything sits behind the login guard now, so each test signs in first —
//! via `common`'s helpers, which do it for free. The per-test scope is the same
//! as before: each `db()` call is its own private in-memory libSQL database.

mod common;

use axum::http::StatusCode;
use common::{count, db, get_index, get_to, ids, only_id, post_to, post_todo, seed, user_id};
use practice::db::{execute, pi};

// --- Read ---

#[tokio::test]
async fn index_renders_the_page() {
    let database = db().await;

    let body = get_index(&database).await;

    assert!(body.contains("<title>Todos · Todo List</title>"));
    assert!(body.contains("todo-panel"));
    assert!(body.contains("todo-list"));
}

#[tokio::test]
async fn index_lists_a_seeded_todo() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;

    assert!(get_index(&database).await.contains("buy milk"));
}

#[tokio::test]
async fn index_escapes_html_in_todo_text() {
    let database = db().await;
    seed(&database, &["<script>alert(1)</script>"]).await;

    let body = get_index(&database).await;
    let text_row = body
        .lines()
        .find(|line| line.contains("class=\"todo-text\""))
        .expect("todo text rendered");

    assert!(text_row.contains("&#60;script&#62;"));
    assert!(!text_row.contains("<script>"));
}

#[tokio::test]
async fn index_shows_the_empty_state_when_there_are_no_todos() {
    let database = db().await;

    let body = get_index(&database).await;

    assert!(body.contains("todo-empty"));
    assert!(body.contains("Nothing on the list"));
}

#[tokio::test]
async fn index_shows_the_open_count() {
    let database = db().await;
    seed(&database, &["a", "b"]).await;
    let me = user_id(&database).await;
    let id = only_id(&database).await;
    execute(
        &database,
        "update todos set done = true where id = ?1 and user_id = ?2",
        vec![pi(id), pi(me)],
    )
    .await
    .expect("update");

    assert!(get_index(&database).await.contains("1 open"));
}

// --- Create ---

#[tokio::test]
async fn create_appends_a_todo() {
    let database = db().await;

    let (status, body) = post_todo(&database, "text=buy+milk").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("buy milk"));
    assert_eq!(count(&database).await, 1);
}

#[tokio::test]
async fn create_returns_the_panel_not_a_full_page() {
    let database = db().await;

    let (_, body) = post_todo(&database, "text=buy+milk").await;

    assert!(!body.contains("<!DOCTYPE html>"));
    assert!(body.contains("todo-panel"));
}

#[tokio::test]
async fn create_rerenders_the_input_empty_so_the_box_clears() {
    let database = db().await;

    let (_, body) = post_todo(&database, "text=buy+milk").await;

    let input = body
        .split_once("<input")
        .expect("input rendered")
        .1
        .split_once('>')
        .expect("input tag closed")
        .0;
    assert!(!input.contains("value="));
}

#[tokio::test]
async fn create_rejects_blank_text_without_adding() {
    let database = db().await;

    let (status, body) = post_todo(&database, "text=%20%20%20").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(count(&database).await, 0);
    assert!(!body.contains("%20"));
}

#[tokio::test]
async fn create_returns_a_422_when_the_body_cannot_be_parsed() {
    let database = db().await;

    let (status, _) = post_todo(&database, "not-a-form-body").await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn create_escapes_html_in_the_todo_text() {
    let database = db().await;

    let (_, body) = post_todo(&database, "text=%3Cscript%3Ealert%3C%2Fscript%3E").await;

    let text_row = body
        .lines()
        .find(|line| line.contains("class=\"todo-text\""))
        .expect("todo text rendered");

    assert!(text_row.contains("&#60;script&#62;"));
    assert!(!text_row.contains("<script>"));
}

// --- Toggle ---

/// The toggle must swap `#todo-list`, never `#todo-panel`. Panel-wide swaps
/// would re-render the add input and discard text the user typed but had not
/// yet submitted.
#[tokio::test]
async fn toggle_targets_the_list_not_the_panel() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;

    let body = get_index(&database).await;
    let checkbox = body
        .split_once("todo-toggle")
        .expect("checkbox rendered")
        .1
        .split_once('>')
        .expect("input tag closed")
        .0;

    assert!(checkbox.contains("hx-target=\"#todo-list\""));
    assert!(!checkbox.contains("#todo-panel"));
}

#[tokio::test]
async fn toggle_marks_a_todo_done() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;

    let (status, body) = post_to(&database, &format!("/app/todos/{id}/toggle")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("todo-row--done"));
    assert!(body.contains("checked"));
    assert!(body.contains("id=\"todo-count\""));
    assert!(body.contains("0 open"));
}

#[tokio::test]
async fn toggle_returns_only_the_list_fragment() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;

    let (_, body) = post_to(&database, &format!("/app/todos/{id}/toggle")).await;

    assert!(!body.contains("<!DOCTYPE html>"));
    assert!(!body.contains("todo-input"));
    assert!(body.contains("todo-list"));
}

#[tokio::test]
async fn toggle_twice_returns_to_undone() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let uri = format!("/app/todos/{}/toggle", only_id(&database).await);

    post_to(&database, &uri).await;
    let (status, body) = post_to(&database, &uri).await;

    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("todo-row--done"));
}

#[tokio::test]
async fn completed_sorts_below_active_in_the_rendered_fragment() {
    let database = db().await;
    seed(&database, &["one", "two", "three"]).await;
    let me = user_id(&database).await;
    let all = ids(&database).await;
    execute(
        &database,
        "update todos set done = true where id = ?1 and user_id = ?2",
        vec![pi(all[0]), pi(me)],
    )
    .await
    .expect("update");

    let (_, body) = post_to(&database, &format!("/app/todos/{}/toggle", all[1])).await;

    let one = body.find(">one<").expect("one rendered");
    let two = body.find(">two<").expect("two rendered");
    let three = body.find(">three<").expect("three rendered");
    assert!(three < one && one < two, "completed must sort below active");
}

// --- Delete ---

#[tokio::test]
async fn delete_removes_a_todo() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;

    let (status, body) = post_to(&database, &format!("/app/todos/{id}/delete")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("buy milk"));
    assert!(body.contains("todo-empty"));
    assert!(body.contains("0 open"));
    assert_eq!(count(&database).await, 0);
}

#[tokio::test]
async fn the_row_carries_a_delete_control() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;

    let body = get_index(&database).await;
    let button = body
        .split_once("todo-delete")
        .expect("delete control rendered")
        .1
        .split_once('>')
        .expect("button tag closed")
        .0;

    assert!(button.contains(&format!("hx-post=\"/app/todos/{id}/delete\"")));
    assert!(button.contains("hx-target=\"#todo-list\""));
    assert!(button.contains("hx-swap=\"outerHTML\""));
    // A stray click must not destroy work.
    assert!(button.contains("hx-confirm="));
}

/// The delete button is a real control, not decoration, and its accessible
/// name says which todo it destroys.
#[tokio::test]
async fn the_delete_control_is_labelled() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;

    assert!(
        get_index(&database)
            .await
            .contains("aria-label=\"Delete buy milk\"")
    );
}

/// The list fragment must carry no add form, so the button may not introduce
/// one or reuse the add input's class.
#[tokio::test]
async fn the_delete_fragment_carries_no_add_form() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;

    let (_, body) = post_to(&database, &format!("/app/todos/{id}/delete")).await;

    assert!(!body.contains("todo-input"));
    assert!(!body.contains("<form"));
}

// --- Unknown ids and routing ---

#[tokio::test]
async fn unknown_id_is_404_with_the_current_list_as_the_body() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;

    let (status, body) = post_to(&database, "/app/todos/9999/toggle").await;

    // Honest status. htmx 4 swaps 4xx by default, so the list renders unchanged.
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("buy milk"));
}

#[tokio::test]
async fn unknown_id_delete_is_404_and_leaves_state_alone() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;

    let (status, _) = post_to(&database, "/app/todos/9999/delete").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(count(&database).await, 1);
}

#[tokio::test]
async fn non_numeric_id_is_rejected_by_the_path_extractor() {
    let database = db().await;

    let (status, _) = post_to(&database, "/app/todos/abc/toggle").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_injection_payload_as_a_path_id_is_rejected() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;

    let (status, _) = post_to(&database, "/app/todos/1;DROP%20TABLE%20todos/toggle").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(count(&database).await, 1, "the row must still be there");
}

#[tokio::test]
async fn an_unknown_path_returns_404() {
    let database = db().await;

    let (status, _) = get_to(&database, "/no-such-route").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_wrong_method_on_a_known_path_returns_405() {
    let database = db().await;
    seed(&database, &["buy milk"]).await;
    let id = only_id(&database).await;

    let (status, _) = get_to(&database, &format!("/app/todos/{id}/toggle")).await;

    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

// --- Injection ---

/// Every stored string is user input, so each of these is a real attempt to
/// break out of the text node. Askama must escape all of them, and no angle
/// bracket may survive into the rendered row.
#[tokio::test]
async fn xss_payloads_are_escaped_on_the_read_path() {
    let payloads = [
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "\"><img src=x onerror=alert(1)>",
        "'><svg/onload=alert(1)>",
        "<svg><animate onbegin=alert(1) attributeName=x dur=1s>",
        "javascript:alert(1)",
        "<iframe src=javascript:alert(1)></iframe>",
        "</span><script>alert(1)</script><span>",
    ];
    let database = db().await;
    seed(&database, &payloads).await;

    let body = get_index(&database).await;
    for line in body.lines().filter(|l| l.contains("class=\"todo-text\"")) {
        // Isolate the text node: everything between the span's own `>` and
        // `</span>`. If no angle bracket survives here, no tag can be
        // reconstructed, and an inert `onerror=` inside text is just text.
        let inner = line
            .split_once('>')
            .expect("span opens")
            .1
            .rsplit_once('<')
            .expect("span closes")
            .0;
        assert!(
            !inner.contains('<'),
            "raw `<` escaped its text node: {line}"
        );
        assert!(
            !inner.contains('>'),
            "raw `>` escaped its text node: {line}"
        );
    }

    // Escaped entities prove the text survived rather than being stripped.
    assert!(body.contains("&#60;script&#62;alert(1)&#60;/script&#62;"));
}

/// The same payload must survive the create round trip, which is the path an
/// attacker actually reaches first.
#[tokio::test]
async fn xss_payloads_are_escaped_through_create() {
    let database = db().await;

    let (_, body) = post_todo(&database, "text=%3Cscript%3Ealert%281%29%3C%2Fscript%3E").await;

    assert!(!body.contains("<script>alert"));
    assert!(body.contains("&#60;script&#62;"));
}

/// A payload containing quotes must not escape an attribute. The todo text
/// renders into text content, but the delete button's aria-label sits in an
/// attribute, so this proves the escaping is correct in both positions.
#[tokio::test]
async fn quote_payloads_cannot_break_out_of_attributes() {
    let database = db().await;
    seed(&database, &["\" onmouseover=\"alert(1)"]).await;

    let body = get_index(&database).await;
    assert!(
        !body.contains("aria-label=\"Delete \" onmouseover="),
        "the attribute was terminated by the payload"
    );
    let label = body
        .split_once("aria-label=\"Delete ")
        .expect("aria-label rendered")
        .1;
    let value = &label[..label.find('"').expect("attribute terminated")];
    assert!(
        !value.contains('"'),
        "an unescaped quote reached the attribute value: {value}"
    );
    assert!(
        value.contains("onmouseover="),
        "payload text was stripped: {value}"
    );
}

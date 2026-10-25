//! # HTTP layer — 4 handlers + router
//!
//! Handlers take axum extractors (`State`, `Path`, `Form`, `CurrentUser`) and
//! return `Result<_, AppError>`. `Path(id): Path<i64>` rejects a non-numeric id
//! before any SQL runs. Three of the four return fragments, not pages — htmx
//! swaps them into the page.
//!
//! Every handler starts with `CurrentUser`, which is how a todo gets tied to
//! the account that owns it. There is no route here that can reach the store
//! without one.

use askama::Template as _;
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{Html, IntoResponse, Response},
    routing::get,
};

use super::{
    TodoStore,
    store::{add, flip_done, remove, sorted_with_count},
    templates::{IndexTemplate, PanelTemplate, TodoUpdateTemplate},
};
use crate::AppError;
use crate::auth::CurrentUser;

async fn index(
    State(db): State<TodoStore>,
    CurrentUser(user): CurrentUser,
) -> Result<Html<String>, AppError> {
    // One query instead of two: the window function computes open_count
    // alongside every todo row in a single database round trip.
    let (todos, open_count) = sorted_with_count(&db, user.id).await?;
    // Read what is needed before moving `user.email` out, or the borrow below
    // would be of a value that has already been partially moved.
    Ok(Html(
        IndexTemplate {
            todos,
            open_count,
            email: user.email,
        }
        .render()?,
    ))
}

#[derive(serde::Deserialize)]
struct NewTodo {
    text: String,
}

async fn create(
    State(db): State<TodoStore>,
    CurrentUser(user): CurrentUser,
    axum::extract::Form(form): axum::extract::Form<NewTodo>,
) -> Result<Html<String>, AppError> {
    add(&db, user.id, &form.text).await?;
    let (todos, open_count) = sorted_with_count(&db, user.id).await?;
    Ok(Html(PanelTemplate { todos, open_count }.render()?))
}

/// Runs `mutate`, then renders the list and its out-of-band open count.
/// `mutate` returns false when the id was not found **for this user**, which
/// becomes a 404 whose body is still the current list.
///
/// The list is fetched after the write completes. Running them concurrently can
/// observe the pre-write state and return stale rows or a stale open count.
async fn with_list<F>(db: &TodoStore, user_id: i64, mutate: F) -> Result<Response, AppError>
where
    F: std::future::Future<Output = Result<bool, AppError>>,
{
    let found = mutate.await?;
    let (todos, open_count) = sorted_with_count(db, user_id).await?;
    let body = TodoUpdateTemplate { todos, open_count }.render()?;
    let status = if found {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    Ok((status, Html(body)).into_response())
}

async fn toggle(
    State(db): State<TodoStore>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    with_list(&db, user.id, flip_done(&db, user.id, id)).await
}

async fn delete(
    State(db): State<TodoStore>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    with_list(&db, user.id, remove(&db, user.id, id)).await
}

/// The todo routes, already behind the login guard.
///
/// `route_layer` applies to every route defined in this router and no others,
/// so the todo pages require a session while `/login` and `/signup` — merged in
/// separately by `src/app.rs` — stay reachable. A new todo route added here is
/// protected automatically; forgetting to guard it is not a mistake you can
/// make.
pub fn build_router(db: TodoStore) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/todos", axum::routing::post(create))
        .route("/todos/{id}/toggle", axum::routing::post(toggle))
        .route("/todos/{id}/delete", axum::routing::post(delete))
        .route_layer(middleware::from_fn(crate::auth::require_auth))
        .with_state(db)
}

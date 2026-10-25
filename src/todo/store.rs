//! # Data access — every statement in the todo domain
//!
//! Six functions, one query each. Values always go in as query parameters
//! rather than interpolated, so injection payloads arrive as literal text.
//! Mutations return `bool` from rows-changed, which is how handlers pick 200
//! vs 404.
//!
//! Every function takes a `user_id` and filters on it. That filter is the whole
//! of the multi-user security model: a todo belonging to somebody else is not
//! hidden by a check in the handler, it simply is not in the result set.

use crate::db::{self, Db, Row};
use libsql::Error;

use super::Todo;
use crate::AppError;

// The table also carries a CHECK constraint on the trimmed length. Validating
// here first is what keeps a blank or over-long todo a 200 with an unchanged
// panel instead of a constraint violation surfacing as a 500.
pub const MAX_TEXT_LEN: usize = 280;

/// Returns the trimmed text, or `None` when it is blank or over-long. Mirrors
/// the `todos_text_length` CHECK constraint so the common cases never reach
/// the database.
fn validated(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_TEXT_LEN {
        return None;
    }
    Some(text)
}

/// Decodes a row in `id, text, done` order — the column order every SELECT in
/// this file uses.
fn todo_row(row: &Row) -> Result<Todo, Error> {
    Ok(Todo {
        id: db::ri(row, 0)?,
        text: db::rs(row, 1)?,
        done: db::rb(row, 2)?,
    })
}

/// Active todos first, completed after, insertion order within each group.
/// `ORDER BY done` puts false before true, which is the SQL equivalent of the
/// old in-memory `sort_by_key(done)`, and `id` is monotonic so it stands in for
/// insertion order.
pub async fn sorted(db: &Db, user_id: i64) -> Result<Vec<Todo>, AppError> {
    Ok(db::query_all(
        db,
        "select id, text, done from todos where user_id = ?1 order by done asc, id asc",
        vec![db::pi(user_id)],
        todo_row,
    )
    .await?)
}

/// Returns the sorted todo list *and* the count of open todos in a single
/// database round trip. The window function `sum(...) over ()` computes the
/// open count alongside every row without a second scan or query.
pub async fn sorted_with_count(
    db: &Db,
    user_id: i64,
) -> Result<(Vec<Todo>, i64), AppError> {
    let rows = db::query_all(
        db,
        "select id, text, done,
               sum(case when done = 0 then 1 else 0 end) over () as open_count
        from todos
        where user_id = ?1
        order by done asc, id asc",
        vec![db::pi(user_id)],
        |row| {
            Ok((
                Todo {
                    id: db::ri(row, 0)?,
                    text: db::rs(row, 1)?,
                    done: db::rb(row, 2)?,
                },
                db::ri(row, 3)?,
            ))
        },
    )
    .await?;

    let open_count = rows.first().map(|(_, count)| *count).unwrap_or(0);
    let todos: Vec<Todo> = rows.into_iter().map(|(todo, _)| todo).collect();
    Ok((todos, open_count))
}

pub async fn open_count(db: &Db, user_id: i64) -> Result<i64, AppError> {
    Ok(db::scalar_i64(
        db,
        "select count(*) from todos where user_id = ?1 and done = 0",
        vec![db::pi(user_id)],
    )
    .await?)
}

/// Inserts unless the text is rejected. Returns false when nothing was written.
pub async fn add(db: &Db, user_id: i64, text: &str) -> Result<bool, AppError> {
    let Some(text) = validated(text) else {
        return Ok(false);
    };
    db::execute(
        db,
        "insert into todos (text, user_id) values (?1, ?2)",
        vec![db::ps(text), db::pi(user_id)],
    )
    .await?;
    Ok(true)
}

/// Flips `done`. Returns false when no todo has this id **and** this owner.
///
/// The `AND user_id = ?2` is what makes somebody else's todo behave exactly
/// like a missing one: zero rows, so the handler answers 404 and never reveals
/// that the id exists.
pub async fn flip_done(db: &Db, user_id: i64, id: i64) -> Result<bool, AppError> {
    let affected = db::execute(
        db,
        "update todos set done = 1 - done where id = ?1 and user_id = ?2",
        vec![db::pi(id), db::pi(user_id)],
    )
    .await?;
    Ok(affected > 0)
}

/// Removes the item. Returns false when no todo has this id and this owner.
pub async fn remove(db: &Db, user_id: i64, id: i64) -> Result<bool, AppError> {
    let affected = db::execute(
        db,
        "delete from todos where id = ?1 and user_id = ?2",
        vec![db::pi(id), db::pi(user_id)],
    )
    .await?;
    Ok(affected > 0)
}

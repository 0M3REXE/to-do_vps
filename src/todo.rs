//! # The todo domain — module front door
//!
//! Holds the shared types and re-exports the submodules, so callers write
//! `todo::build_router`. Split by concern: `store` (all SQL), `templates`,
//! `handlers`. The error type used to live here as `todo/error.rs`; it moved to
//! `src/error.rs` when authentication needed one too.
//! Walkthrough: `docs/architecture.md`.

mod handlers;
mod store;
mod templates;

pub use handlers::build_router;
pub use store::{MAX_TEXT_LEN, add, flip_done, open_count, remove, sorted};

use crate::db::Db;

/// The database connection is the store. Aliasing it keeps the handler
/// signatures readable and gives the name a place to change if the storage
/// does.
pub type TodoStore = Db;

/// One row of the list, as the templates receive it.
#[derive(Debug, Clone)]
pub struct Todo {
    pub id: i64,
    pub text: String,
    pub done: bool,
}

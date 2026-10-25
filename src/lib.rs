//! # The library root
//!
//! Everything reusable lives here. `main.rs` runs it; `tests/*.rs` import it.
//! Integration tests are separate crates and cannot import a binary — without
//! this file the tests would have to live inside `src/`.
//!
//! Full walkthrough: `docs/architecture.md`.

mod error;

pub mod app;
pub mod auth;
pub mod db;
pub mod todo;

pub use app::AppConfig;
pub use error::AppError;

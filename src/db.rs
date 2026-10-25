//! # Database plumbing — the one place that knows libSQL
//!
//! Everything above this layer deals with `Db` (a libSQL connection) and plain
//! data; everything below talks to Turso. Swapping drivers later means editing
//! this file and a migration file, and nothing else.
//!
//! ## Why a bare connection instead of a pool
//!
//! Postgres wanted pooling because a connection is a socket. Turso is reached
//! over HTTP, and a libSQL connection is a thin handle a request can clone
//! cheaply — so `Db` is a `Connection` and every handler just passes it on.
//!
//! ## Timestamps
//!
//! SQLite has no datetime type and no `now()`, so the app never asks the
//! database for the time. Every timestamp is rendered in Rust with
//! [`TIME_FORMAT`] — UTC, always the same width — stored as TEXT, and compared
//! with plain string comparisons, which agree with time as long as every value
//! has the same shape.

use libsql::{Error, Value};
use time::{OffsetDateTime, format_description, macros::format_description};

/// The connection every part of the app talks to.
pub type Db = libsql::Connection;

/// The type a row comes back as.
pub use libsql::Row;

/// The format every timestamp in the database uses, as a compile-time-checked
/// format description.
///
/// Three sub-second digits, never fewer: `2026-10-23T12:00:00.000Z`. A plain
/// RFC 3339 writer would drop the `.000` for whole seconds, and then
/// `...00.500Z` would sort *before* `...00.000Z`, silently breaking every
/// "is this expired?" comparison — which is decided in SQL on the raw text.
/// Fixed width removes the whole class of bug.
pub static TIME_FORMAT: &[format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

/// The time now, formatted the way the database stores it.
pub fn now_ts() -> String {
    format_ts(OffsetDateTime::now_utc())
}

/// Formats a timestamp exactly as it is stored.
pub fn format_ts(ts: OffsetDateTime) -> String {
    ts.format(TIME_FORMAT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".to_owned())
}

/// Parses a stored value back into a timestamp.
pub fn parse_ts(raw: &str) -> Result<OffsetDateTime, Error> {
    OffsetDateTime::parse(raw, TIME_FORMAT)
        .map_err(|err| Error::Misuse(format!("{raw:?} is not a stored timestamp: {err}")))
}

/// Opens the database named in the environment, and migrates it.
///
/// `DATABASE_URL` accepts:
///
/// - `libsql://...` or `http(s)://...` — a libSQL server over HTTP: Turso
///   cloud, or a self-hosted sqld such as the one in `compose.yaml`.
///   `DATABASE_AUTH_TOKEN` supplies the token when the server requires one;
///   a self-hosted sqld on a private network has authentication off, so the
///   variable may be unset there.
/// - `file:...` — a local file, for development without a network.
///
/// Migrations are embedded, so a deploy needs no separate step. A failure here
/// is fatal in the callers: serving against half a schema would fail later,
/// less clearly.
pub async fn open_from_env() -> Result<Db, Error> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "file:practice.db".to_owned());

    let db = match url.as_str() {
        remote
            if remote.starts_with("libsql://")
                || remote.starts_with("https://")
                || remote.starts_with("http://") =>
        {
            let token = std::env::var("DATABASE_AUTH_TOKEN").unwrap_or_default();
            libsql::Builder::new_remote(remote.to_owned(), token).build().await?
        }
        file => {
            let path = file.strip_prefix("file:").unwrap_or(file);
            libsql::Builder::new_local(path).build().await?
        }
    };

    let conn = db.connect()?;
    migrate(&conn).await?;
    Ok(conn)
}

/// Opens a fresh in-memory database with all migrations applied.
///
/// What the tests use instead of a throwaway server: every call gets its own
/// private database, so tests run in parallel without interfering.
pub async fn open_in_memory() -> Result<Db, Error> {
    let db = libsql::Builder::new_local(":memory:").build().await?;
    let conn = db.connect()?;
    migrate(&conn).await?;
    Ok(conn)
}

/// Applies any un-applied embedded migrations, in order.
///
/// Two cold starts can race each other on a fresh Turso database and both see
/// the schema missing; `if not exists` and `insert or ignore` make both
/// arrivals converge instead of one of them failing.
pub async fn migrate(conn: &Db) -> Result<(), Error> {
    const MIGRATIONS: &[(i64, &str)] = &[
        (1, include_str!("../migrations/0001_libsql_schema.sql")),
        (2, include_str!("../migrations/0002_auth_rate_limits.sql")),
    ];

    conn.execute(
        "create table if not exists _migrations (
             version    integer primary key,
             applied_at text not null
         )",
        (),
    )
    .await?;

    for (version, sql) in MIGRATIONS {
        let applied = scalar_i64(
            conn,
            "select count(*) from _migrations where version = ?1",
            vec![pi(*version)],
        )
        .await?
            > 0;
        if applied {
            continue;
        }

        for statement in split_statements(sql) {
            conn.execute(&statement, ()).await?;
        }
        conn.execute(
            "insert or ignore into _migrations (version, applied_at) values (?1, ?2)",
            (pi(*version), ps(now_ts())),
        )
        .await?;
    }

    Ok(())
}

/// Cuts a migration file into executable statements.
///
/// Files are written one statement per `;`, with `--` comments allowed
/// anywhere. Splitting on `;` is safe for this codebase because no statement
/// contains a semicolon inside a string, and comments are stripped so a chunk
/// that is pure comment turns into nothing rather than empty SQL.
fn split_statements(sql: &str) -> Vec<String> {
    sql.split(';')
        .map(|chunk| {
            chunk
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned()
        })
        .filter(|statement| !statement.is_empty())
        .collect()
}

// --- parameters ----------------------------------------------------------------

/// An `i64` parameter.
pub fn pi(n: i64) -> Value {
    Value::Integer(n)
}

/// A boolean parameter. SQLite stores integers, so `true` is 1.
pub fn pb(b: bool) -> Value {
    Value::Integer(b as i64)
}

/// A string parameter.
pub fn ps(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

/// An optional value as a parameter: `NULL` when there is none.
pub fn pn(value: Option<Value>) -> Value {
    value.unwrap_or(Value::Null)
}

/// A timestamp parameter, formatted the way it is stored.
pub fn pt(ts: OffsetDateTime) -> Value {
    ps(format_ts(ts))
}

// --- row helpers ---------------------------------------------------------------

/// Reads an `i64` column.
pub fn ri(row: &Row, index: usize) -> Result<i64, Error> {
    Ok(row.get::<i64>(index as i32)?)
}

/// Reads a boolean column, which SQLite stores as an integer.
///
/// `1` is true and `0` is false; anything else refuses, so a corrupted value
/// cannot quietly mean "done".
pub fn rb(row: &Row, index: usize) -> Result<bool, Error> {
    match row.get_value(index as i32)? {
        Value::Integer(1) => Ok(true),
        Value::Integer(0) => Ok(false),
        other => Err(Error::Misuse(format!(
            "expected a 0/1 boolean column, found {other:?}"
        ))),
    }
}

/// Reads a string column.
pub fn rs(row: &Row, index: usize) -> Result<String, Error> {
    Ok(row.get::<String>(index as i32)?)
}

/// Reads a nullable string column.
pub fn ros(row: &Row, index: usize) -> Result<Option<String>, Error> {
    match row.get_value(index as i32)? {
        Value::Null => Ok(None),
        Value::Text(content) => Ok(Some(content)),
        other => Err(Error::Misuse(format!(
            "expected a text column, found {other:?}"
        ))),
    }
}

/// Reads a nullable timestamp column, stored the way [`format_ts`] writes it.
pub fn rots(row: &Row, index: usize) -> Result<Option<OffsetDateTime>, Error> {
    match row.get_value(index as i32)? {
        Value::Null => Ok(None),
        Value::Text(ref raw) => Ok(Some(parse_ts(raw)?)),
        other => Err(Error::Misuse(format!(
            "expected a stored timestamp, found {other:?}"
        ))),
    }
}

// --- small query helpers ---------------------------------------------------------

/// Runs a scalar query and returns the single integer in the first column.
pub async fn scalar_i64(db: &Db, sql: impl AsRef<str>, params: Vec<Value>) -> Result<i64, Error> {
    let mut rows = db.query(sql.as_ref(), params).await?;
    let Some(row) = rows.next().await? else {
        return Err(Error::QueryReturnedNoRows);
    };
    ri(&row, 0)
}

/// Runs a query that may return no row, mapping the row to a type.
pub async fn opt_query<T, F>(
    db: &Db,
    sql: impl AsRef<str>,
    params: Vec<Value>,
    of: F,
) -> Result<Option<T>, Error>
where
    F: Fn(&Row) -> Result<T, Error>,
{
    let mut rows = db.query(sql.as_ref(), params).await?;
    match rows.next().await? {
        None => Ok(None),
        Some(row) => Ok(Some(of(&row)?)),
    }
}

/// Runs a query, mapping every row to a type.
pub async fn query_all<T, F>(
    db: &Db,
    sql: impl AsRef<str>,
    params: Vec<Value>,
    of: F,
) -> Result<Vec<T>, Error>
where
    F: Fn(&Row) -> Result<T, Error>,
{
    let mut rows = db.query(sql.as_ref(), params).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(of(&row)?);
    }
    Ok(out)
}

/// Runs a statement, returning how many rows it changed.
pub async fn execute(db: &Db, sql: impl AsRef<str>, params: Vec<Value>) -> Result<u64, Error> {
    let affected = db.execute(sql.as_ref(), params).await?;
    Ok(affected)
}

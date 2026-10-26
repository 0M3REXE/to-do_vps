//! # Data access — user SQL, and the session store
//!
//! Two things live here because both are "talk to the database": the three
//! queries the `users` table needs, and the four methods `tower-sessions`
//! calls to keep its own records.
//!
//! ## Why the session store is hand-written
//!
//! There is no official libSQL session store for `tower-sessions`. Four short
//! methods against the helper functions in `src/db.rs` are cheaper than
//! patching upstream crates together with version restrictions.

use async_trait::async_trait;
use axum_login::tower_sessions::{
    SessionStore,
    session::{Id, Record},
    session_store::{Error as SessionError, Result as SessionResult},
};
use libsql::Error;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use super::{
    AUTH_DATA_KEY, User,
    security::{self, SECURITY_DATA_KEY, SessionSecurity},
};
use crate::db::{self, Db, Row};

/// The hard upper bound on a session's life, regardless of activity. The
/// database keeps `created_at`; `save` never moves it.
const MAX_SESSION_AGE_DAYS: i64 = 7;

fn session_age_cutoff() -> String {
    db::format_ts(
        time::OffsetDateTime::now_utc() - time::Duration::days(MAX_SESSION_AGE_DAYS),
    )
}

/// The primary key is a hash of the cookie's session id. A database dump must
/// not contain a value that can be replayed as a cookie.
fn session_key(id: &Id) -> String {
    security::hash_token(&id.0.to_string())
}

/// Every `SELECT` on users selects exactly these columns, in exactly this
/// order — `user_row` decodes positionally, so drifting apart is not possible.
const USER_COLUMNS: &str = "id, email, password_hash";

/// What a row looks like before the password has been checked.
///
/// [`User`] deliberately does not keep the hash, so the login path works with
/// this instead and only converts once the password has been verified.
pub(super) struct UserRow {
    pub(super) id: i64,
    pub(super) email: String,
    pub(super) password_hash: String,
}

/// Decodes the three [`USER_COLUMNS`] out of a row, by position.
fn user_row(row: &Row) -> Result<UserRow, Error> {
    Ok(UserRow {
        id: db::ri(row, 0)?,
        email: db::rs(row, 1)?,
        password_hash: db::rs(row, 2)?,
    })
}

impl UserRow {
    pub(super) fn into_user(self) -> User {
        User::new(self.id, self.email, &self.password_hash)
    }
}

/// True when an insert failed because it broke a UNIQUE constraint — in this
/// app, only `users_email_key`. Both drivers, local and remote, surface the
/// same phrase in the message, so the check is on text rather than assuming an
/// error-code API that the two paths may not share.
fn is_unique_violation(err: &Error) -> bool {
    err.to_string().to_uppercase().contains("UNIQUE CONSTRAINT")
}

/// Creates an account. Returns `None` when the email is already registered —
/// which is a form error the visitor can fix, not a server fault, so it never
/// becomes a 500.
///
/// The unique index on `lower(email)` is what actually guarantees one account
/// per address: a simultaneous signup on the same address is a race here, and
/// the loser gets the same "already registered" outcome.
pub async fn create_user(db: &Db, email: &str, password_hash: &str) -> Result<Option<User>, Error> {
    let id = match db::opt_query(
        db,
        "insert into users (email, password_hash, created_at) values (?1, ?2, ?3) returning id",
        vec![db::ps(email), db::ps(password_hash), db::ps(db::now_ts())],
        |row| db::ri(row, 0),
    )
    .await
    {
        Ok(Some(id)) => id,
        Ok(None) => return Ok(None),
        Err(err) if is_unique_violation(&err) => return Ok(None),
        Err(err) => return Err(err),
    };
    Ok(Some(User::new(id, email.to_owned(), password_hash)))
}

/// Finds an account by email, case-insensitively, matching the unique index.
pub async fn find_by_email(db: &Db, email: &str) -> Result<Option<UserRow>, Error> {
    db::opt_query(
        db,
        &format!("select {USER_COLUMNS} from users where lower(email) = lower(?1)"),
        vec![db::ps(email)],
        user_row,
    )
    .await
}

/// Finds an account by id. This is the lookup behind every authenticated
/// request, so it goes by primary key.
pub async fn find_by_id(db: &Db, id: i64) -> Result<Option<UserRow>, Error> {
    db::opt_query(
        db,
        &format!("select {USER_COLUMNS} from users where id = ?1"),
        vec![db::pi(id)],
        user_row,
    )
    .await
}

/// The id of an account, if one exists on that email.
///
/// Deliberately returns only the id: it exists so tests and future flows can
/// ask "is there an account here?" without carrying a password hash around.
pub async fn user_id_by_email(db: &Db, email: &str) -> Result<Option<i64>, Error> {
    db::opt_query(
        db,
        "select id from users where lower(email) = lower(?1)",
        vec![db::ps(email)],
        |row| db::ri(row, 0),
    )
    .await
}

/// Finds an account by id and returns the `User`.
///
/// `find_by_id` is the raw row, because login needs the password hash before it
/// can decide anything. This is the version for everything that has already
/// established *who* is asking and only wants the profile.
pub async fn find_user(db: &Db, id: i64) -> Result<Option<User>, Error> {
    Ok(find_by_id(db, id).await?.map(UserRow::into_user))
}

/// Replaces an account's password hash.
///
/// The hash is what `session_auth_hash` is derived from, so changing it here
/// invalidates every session that was created from the old one — which is how
/// "log every other device out" works without touching the sessions table.
///
/// Returns false when no account has that id.
pub async fn set_password(db: &Db, user_id: i64, hash: &str) -> Result<bool, Error> {
    let affected = db::execute(
        db,
        "update users set password_hash = ?1 where id = ?2",
        vec![db::ps(hash), db::pi(user_id)],
    )
    .await?;
    Ok(affected > 0)
}

// --- the session store -------------------------------------------------------

/// Keeps session records in the `sessions` table.
#[derive(Clone, Debug)]
pub struct DbStore {
    db: Db,
}

impl DbStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// A plain INSERT, used by `create` so that a colliding id fails loudly
    /// instead of overwriting somebody else's session.
    ///
    /// `user_id` is copied out of the session payload on the way in, which is
    /// what later makes "log out everywhere" a single indexed DELETE.
    async fn insert_new(&self, record: &Record) -> Result<(), Error> {
        // Serialising is not a database concern, so it is folded into a
        // misuse error rather than widening every signature.
        let data = encode_data(record).map_err(|err| Error::Misuse(err.to_string()))?;
        let created_at = db::now_ts();
        let affected = db::execute(
            &self.db,
            "insert into sessions (id, data, expiry_date, user_id, created_at)
             values (?1, ?2, ?3, ?4, ?5)",
            vec![
                db::ps(session_key(&record.id)),
                db::ps(data),
                db::pt(record.expiry_date),
                db::pn(owner_of(record).map(db::pi)),
                db::ps(created_at),
            ],
        )
        .await?;
        // A duplicate id under `insert` reports itself differently on the two
        // drivers: locally as an error, remotely as zero rows changed. Both
        // must be a refusal to hand over somebody else's session id.
        if affected == 0 {
            return Err(Error::Misuse("session id already exists".to_owned()));
        }
        Ok(())
    }

    fn is_session_id_collision(err: &Error) -> bool {
        err.to_string().contains("session id already exists") || is_unique_violation(err)
    }
}

/// What a one-time lookup is for. Sessions hold a JSON payload; the owner's id
/// is nested under `AUTH_DATA_KEY` when somebody is logged in.
///
/// Returns `None` for a session with nobody logged in, which is normal.
fn owner_of(record: &Record) -> Option<i64> {
    let data = record.data.get(AUTH_DATA_KEY)?.as_object()?;
    data.get("user_id")?.as_i64()
}

#[async_trait]
impl SessionStore for DbStore {
    async fn create(&self, record: &mut Record) -> SessionResult<()> {
        // Session ids are 128 bits of randomness, so a collision should not
        // happen. If one does, ask for a fresh id and try again — never let a
        // second visitor inherit the first visitor's session.
        for _ in 0..3 {
            match self.insert_new(record).await {
                Ok(()) => return Ok(()),
                Err(err) if Self::is_session_id_collision(&err) => record.id = Id::default(),
                Err(err) => return Err(backend(err)),
            }
        }
        Err(SessionError::Backend(
            "no free session id after 3 attempts".into(),
        ))
    }

    async fn save(&self, record: &Record) -> SessionResult<()> {
        let data = encode_data(record)?;
        db::execute(
            &self.db,
            "insert into sessions (id, data, expiry_date, user_id, created_at)
             values (?1, ?2, ?3, ?4, ?5)
             on conflict (id) do update
                 set data = excluded.data,
                     expiry_date = excluded.expiry_date,
                     user_id = excluded.user_id",
            vec![
                db::ps(session_key(&record.id)),
                db::ps(data),
                db::pt(record.expiry_date),
                db::pn(owner_of(record).map(db::pi)),
                db::ps(db::now_ts()),
            ],
        )
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> SessionResult<Option<Record>> {
        let _ = std::fs::write(
            "C:\\Users\\OMER\\Desktop\\to-do\\debug2.txt",
            format!("load called key={}", session_key(session_id)),
        );
        // Expiry is decided by the query, so an idle or absolutely expired
        // session is never handed back and never has to be parsed. `created_at`
        // is null only for pre-migration sessions, which are intentionally
        // invalidated because their age cannot be established safely.
        let now = db::now_ts();
        let age_cutoff = session_age_cutoff();
        db::opt_query(
            &self.db,
            "select data, expiry_date from sessions
             where id = ?1 and expiry_date > ?2 and created_at > ?3",
            vec![
                db::ps(session_key(session_id)),
                db::ps(now),
                db::ps(age_cutoff),
            ],
            |row| {
                let json = db::rs(row, 0)?;
                let expiry = db::rs(row, 1)?;
                decode_data(session_id, &json, &expiry).map_err(|err| Error::Misuse(err.to_string()))
            },
        )
        .await
        .map_err(backend)
    }

    async fn delete(&self, session_id: &Id) -> SessionResult<()> {
        db::execute(
            &self.db,
            "delete from sessions where id = ?1",
            vec![db::ps(session_key(session_id))],
        )
        .await
        .map_err(backend)?;
        Ok(())
    }
}

/// Throws away every session belonging to an account, returning how many went.
///
/// This is the "log out everywhere" and "my password changed" primitive. It
/// cannot reach an account it is not given, because `user_id` comes from the
/// caller's own session rather than from anything in the request.
///
/// The `ON DELETE CASCADE` on `sessions.user_id` means a deleted account takes
/// its sessions with it, so this is only needed for a live account.
pub async fn delete_sessions_for_user(db: &Db, user_id: i64) -> Result<u64, Error> {
    db::execute(
        db,
        "delete from sessions where user_id = ?1",
        vec![db::pi(user_id)],
    )
    .await
}

/// How many live sessions an account has. Shown on the account page so the
/// visitor can see that "log out everywhere" did something.
pub async fn count_sessions_for_user(db: &Db, user_id: i64) -> Result<i64, Error> {
    let age_cutoff = session_age_cutoff();
    db::scalar_i64(
        db,
        "select count(*) from sessions
         where user_id = ?1 and expiry_date > ?2 and created_at > ?3",
        vec![db::pi(user_id), db::ps(db::now_ts()), db::ps(age_cutoff)],
    )
    .await
}

/// Atomically consumes one fixed-window auth attempt shared across app instances.
/// Only the hash of the client address is stored in the database.
pub async fn allow_auth_attempt(
    db: &Db,
    scope: &str,
    client_address: &str,
    limit: i64,
    window_seconds: i64,
) -> Result<Option<u64>, Error> {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let window_start = now.div_euclid(window_seconds) * window_seconds;
    let key = format!("{scope}:{client_address}");
    let bucket: String = Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    // Discard expired rate-limit buckets opportunistically; no scheduled job is needed.
    db::execute(
        db,
        "delete from auth_rate_limits where window_start < ?1",
        vec![db::pi(now - 24 * 60 * 60)],
    )
    .await?;

    let attempts = db::scalar_i64(
        db,
        "insert into auth_rate_limits (bucket, window_start, attempts)
         values (?1, ?2, 1)
         on conflict (bucket) do update set
             attempts = case when window_start = excluded.window_start then attempts + 1 else 1 end,
             window_start = excluded.window_start
         returning attempts",
        vec![db::ps(bucket), db::pi(window_start)],
    )
    .await?;

    Ok((attempts > limit).then_some((window_start + window_seconds - now) as u64))
}

/// One live session as the sessions page needs it. `key` is the hashed
/// database primary key and must never be rendered.
#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub key: String,
    pub created_at: String,
    pub security: SessionSecurity,
}

/// Lists a user's live sessions. Rows without security metadata are skipped:
/// they cannot be identified for revocation, and the payload migration clears
/// the only realistic source of them.
pub async fn list_sessions_for_user(
    db: &Db,
    user_id: i64,
) -> Result<Vec<SessionSummary>, Error> {
    let rows = db::query_all(
        db,
        "select id, data, created_at from sessions
         where user_id = ?1 and expiry_date > ?2 and created_at > ?3",
        vec![
            db::pi(user_id),
            db::ps(db::now_ts()),
            db::ps(session_age_cutoff()),
        ],
        |row| {
            let key = db::rs(row, 0)?;
            let json = db::rs(row, 1)?;
            let created_at = db::ros(row, 2)?.unwrap_or_default();
            let data: HashMap<String, Value> = match serde_json::from_str(&json) {
                Ok(data) => data,
                Err(_) => return Ok(None),
            };
            let Some(raw) = data.get(SECURITY_DATA_KEY) else {
                return Ok(None);
            };
            let security = match serde_json::from_value::<SessionSecurity>(raw.clone()) {
                Ok(security) => security,
                Err(_) => return Ok(None),
            };
            Ok(Some(SessionSummary {
                key,
                created_at,
                security,
            }))
        },
    )
    .await?;

    Ok(rows.into_iter().flatten().collect())
}

/// Deletes a row by its already-hashed database key. The public id check is
/// done by the caller; the key is never accepted from the browser.
pub async fn delete_session_by_key(db: &Db, key: &str) -> Result<u64, Error> {
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(0);
    }

    db::execute(
        db,
        "delete from sessions where id = ?1",
        vec![db::ps(key)],
    )
    .await
}

/// The session record is a library type. Store only its data map: the raw
/// session id and expiry already live in their own columns and must not be
/// duplicated into JSON.
fn encode_data(record: &Record) -> Result<String, SessionError> {
    serde_json::to_string(&record.data).map_err(|err| SessionError::Encode(err.to_string()))
}

fn decode_data(id: &Id, json: &str, expiry: &str) -> Result<Record, SessionError> {
    let data: HashMap<String, Value> =
        serde_json::from_str(json).map_err(|err| SessionError::Decode(err.to_string()))?;
    let expiry_date = time::PrimitiveDateTime::parse(expiry, crate::db::TIME_FORMAT)
        .map(|dt| dt.assume_utc())
        .map_err(|err| SessionError::Decode(err.to_string()))?;
    Ok(Record {
        id: *id,
        data,
        expiry_date,
    })
}

/// The store's error type is its own, so the detail is carried across as text.
/// It is still logged by whatever surfaces the failure.
fn backend(err: Error) -> SessionError {
    SessionError::Backend(err.to_string())
}

-- The whole schema, in one file.
--
-- This app originally used Postgres, whose migrations live in git history.
-- Turbo/libSQL speaks SQLite, so both the store code and the tables were
-- rewritten once, as if starting fresh: the old files describe a database this
-- app no longer connects to, and tax every reader.
--
-- Timestamps are TEXT, formatted by src/db.rs with a fixed number of
-- sub-second digits and always in UTC. Fixed width matters: "less than now"
-- is decided by a plain string comparison, and "less than" only agrees with
-- time when every value has the same shape. Rust sets every timestamp itself,
-- so SQLite needs no datetime function and none is used.
create table if not exists users (
    id            integer primary key autoincrement,
    email         text not null,
    password_hash text not null,
    created_at    text not null,

    constraint users_email_length check (length(trim(email)) between 3 and 254)
);

-- Uniqueness is decided on lower(email): "Alice@example.com" and
-- "alice@example.com" are one account without rewriting the stored value, so
-- what the user sees is what we kept.
create unique index if not exists users_email_key on users (lower(email));

-- Session records for tower-sessions, via the store in src/auth/store.rs.
--
-- `id` is a 128-bit random integer kept as text. `data` is the whole session
-- record as JSON, in a plain text column — there is no jsonb type here, and
-- the only reader is serde_json in Rust.
create table if not exists sessions (
    id          text primary key,
    data        text not null,
    expiry_date text not null,

    -- Copied out of the session payload when the session is written, so
    -- "log out everywhere" is one indexed DELETE instead of a query into
    -- JSON. Kill sessions when the account they belong to dies too.
    user_id     integer references users (id) on delete cascade
);

create index if not exists sessions_expiry_idx on sessions (expiry_date);
create index if not exists sessions_user_id_idx on sessions (user_id);

create table if not exists todos (
    id      integer primary key autoincrement,
    text    text not null,
    done    integer not null default 0,
    user_id integer not null references users (id) on delete cascade,

    constraint todos_text_length check (length(trim(text)) between 1 and 280)
);

-- Every query filters by user_id before anything else, so it leads the index.
create index if not exists todos_user_done_id_idx on todos (user_id, done, id);

-- Track when each server-side session was first created so an active stolen
-- cookie cannot extend its lifetime forever by keeping the idle timer alive.
-- Existing sessions have no trusted creation time and are rejected by the
-- session store after this migration.
alter table sessions add column created_at text;

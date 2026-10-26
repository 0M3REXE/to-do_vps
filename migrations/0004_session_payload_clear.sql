-- The session payload changed from a whole tower-sessions record to a
-- data-only map, and the primary key is now a SHA-256 hash. Any row created by
-- the previous build cannot be read by the new store, so clear them instead of
-- leaving rows that could produce 500s. This is a one-time logout.
delete from sessions;

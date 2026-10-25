-- Shared fixed-window counters so auth throttling works across serverless instances.
create table if not exists auth_rate_limits (
    bucket       text primary key,
    window_start integer not null,
    attempts     integer not null
);

create index if not exists auth_rate_limits_window_idx on auth_rate_limits (window_start);

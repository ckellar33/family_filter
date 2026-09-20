-- Schema for the online filter library (Neon Postgres).
--
-- Run this once against a fresh Neon database, in the Neon console's SQL
-- editor (or `psql "$NEON_DATABASE_URL" -f db/schema.sql`). Safe to re-run
-- against an existing database too (every statement is `if not exists`/
-- `create or replace`-shaped) -- that's how the `filter_editor` role below
-- gets added to a database that only ever had `filter_reader` before. It
-- creates one table, one row per (title, service) filter entry -- the same
-- unit `control::FilterTile`/`MediaEntry` already thinks in on the app
-- side, so downloading a row is just wrapping it back into a one-entry
-- filter file (see src-tauri/src/online.rs).
--
-- `status` exists from day one even though publishing a *new* title is
-- still owner-only (see publish_filter.py) -- editing an *existing* entry,
-- on the other hand, can now be pushed straight from the app itself via the
-- `filter_editor` role below, and always lands as 'approved' (no review
-- queue yet -- see online.rs's doc comment for the plan to eventually gate
-- that behind 'pending', the same status this column already anticipated
-- for a future "let anyone submit a whole new filter" flow). Nothing about
-- the app's *read* path needs to change when that happens: `filter_reader`'s
-- Row Level Security policy below already only ever exposes 'approved'
-- rows, so review just means flipping a row's status.
--
-- After this: create a `filter_reader` role for the app itself to connect
-- as (see the bottom of this file) and give its connection string to
-- src-tauri/src/online.rs's `READER_CONNECTION_STRING`. That string is safe
-- to commit/ship -- the whole point of the role is that it can't do
-- anything beyond what this policy already allows.
--
-- (This project's Data API is also enabled from an earlier attempt at an
-- HTTP-only read path -- abandoned once it turned out every Data API
-- request, even ones meant for its own "anonymous" role, needs a real
-- signed JWT from a registered auth provider. The `anonymous` Postgres role
-- that exists in this database is a leftover from that; it's harmless to
-- leave alone and unrelated to anything below.)

create table if not exists public.online_filters (
    id bigint generated always as identity primary key,
    title text not null,
    -- Same normalization filter::normalize_title does client-side --
    -- duplicated here (not derived via a generated column) so it can be
    -- part of the uniqueness constraint below without depending on Postgres
    -- and Rust's lower()/trim() agreeing on every possible title forever.
    normalized_title text not null,
    -- '' is the generic/unspecified-service entry, same convention as
    -- filter::MediaEntry::service.
    service text not null default '',
    -- The MediaEntry itself: {"title": ..., "service": ..., "cues": [...]}
    -- -- start/end may be plain seconds or "HH:MM:SS.ss" strings, either
    -- way, since FilterList::load accepts both (see filter.rs's
    -- convert_hms_strings_to_seconds). Stored exactly as extracted from a
    -- source filter file, no reformatting needed at publish time.
    media jsonb not null,
    status text not null default 'pending' check (status in ('pending', 'approved', 'rejected')),
    -- Who submitted this row -- unused today (publish_filter.py always
    -- publishes as Christopher Kellar), reserved for the future public
    -- submission flow.
    submitted_by text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (normalized_title, service)
);

create index if not exists online_filters_status_idx on public.online_filters (status);

-- Row Level Security: only 'approved' rows are ever visible, and only for
-- SELECT. There's deliberately no INSERT/UPDATE policy for `filter_reader`
-- -- publish_filter.py writes as the database owner instead, which bypasses
-- RLS entirely (Postgres owners always do), so it needs no policy of its
-- own here.
alter table public.online_filters enable row level security;

-- The role the app itself connects as (see online.rs's
-- READER_CONNECTION_STRING) -- login-capable but otherwise as unprivileged
-- as Postgres allows: no grants beyond CONNECT/USAGE/SELECT, and the SELECT
-- it does get is further narrowed to 'approved' rows by the policy below.
-- Replace 'change-me' with a real generated password before running this
-- against a fresh project (Neon's console SQL editor or `psql` will prompt
-- you if you'd rather not hardcode it here even transiently).
do $$
begin
    if not exists (select from pg_roles where rolname = 'filter_reader') then
        create role filter_reader with login password 'change-me';
    end if;
end
$$;

grant connect on database neondb to filter_reader;
grant usage on schema public to filter_reader;
grant select on public.online_filters to filter_reader;

drop policy if exists filter_reader_select_approved on public.online_filters;
create policy filter_reader_select_approved on public.online_filters
    for select
    to filter_reader
    using (status = 'approved');

-- The write-capable counterpart to `filter_reader` -- see online.rs's
-- EDITOR_CONNECTION_STRING doc comment for the tradeoff this represents
-- (it ships in every build, so it is not "harmless to leak" the way the
-- reader role's connection string is) and why its grants below are kept as
-- narrow as the in-app "publish an edit" feature allows: INSERT and UPDATE
-- only, nothing else -- no DELETE (an edit can only ever add/overwrite a
-- row, never remove one from the app), no ownership, no DDL. Password here
-- must match EDITOR_CONNECTION_STRING's -- unlike filter_reader's
-- 'change-me' placeholder above, there's no confidentiality reason to keep
-- this one out of the repo (it's shipped in the binary either way), so
-- it's the real value on purpose.
do $$
begin
    if not exists (select from pg_roles where rolname = 'filter_editor') then
        create role filter_editor with login password 'qpQECm0ZInwFNSfhvSBLaMBs6fFYZfcb';
    end if;
end
$$;

grant connect on database neondb to filter_editor;
grant usage on schema public to filter_editor;
grant select, insert, update on public.online_filters to filter_editor;

-- Read access matches filter_reader's -- an upsert needs to see the row
-- it's about to conflict with -- but insert/update are unrestricted by row
-- (`using (true)`/`with check (true)`): there's no per-user identity in
-- this app to scope "which rows can this install touch" any tighter than
-- "any of them", so the Postgres grant above (INSERT/UPDATE, no DELETE) is
-- the actual boundary, not this policy.
drop policy if exists filter_editor_select on public.online_filters;
create policy filter_editor_select on public.online_filters
    for select
    to filter_editor
    using (true);

drop policy if exists filter_editor_insert on public.online_filters;
create policy filter_editor_insert on public.online_filters
    for insert
    to filter_editor
    with check (true);

drop policy if exists filter_editor_update on public.online_filters;
create policy filter_editor_update on public.online_filters
    for update
    to filter_editor
    using (true)
    with check (true);

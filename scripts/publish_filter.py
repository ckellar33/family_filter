#!/usr/bin/env python3
"""Publishes a local filter file's entries to the online filter library
(Neon Postgres -- see db/schema.sql), so they show up under the app's
"Online" tab for every install to browse/download (see
src-tauri/src/online.rs).

Deliberately *not* something the app itself can do yet -- this connects
directly with the database owner's connection string (bypassing the
`anonymous`-role-only Row Level Security policy the app's read path is
limited to), which is only ever meant to live on your own machine, never
shipped in the app. See db/schema.sql's doc comment for why `status` exists
now even though this is the only writer today.

Setup:
    pip install psycopg2-binary
    export NEON_DATABASE_URL=postgres://<owner>:<password>@<host>/<db>?sslmode=require
    (the direct connection string from the Neon console -- Dashboard ->
    Connect -- not the Data API URL the app itself uses)

Usage:
    python3 scripts/publish_filter.py sample-filters/the-princess-bride.json
    python3 scripts/publish_filter.py sample-filters/*.json

Re-running for a title+service already published overwrites its cues (and
bumps updated_at) rather than erroring or duplicating -- same
"re-publish after an edit" workflow build_language_filter.py already
supports for local files.
"""

import argparse
import json
import os
import sys

try:
    import psycopg2
    from psycopg2.extras import Json
except ImportError:
    print("Missing dependency -- run: pip install psycopg2-binary", file=sys.stderr)
    sys.exit(1)


def normalize_title(title: str) -> str:
    # Must match filter::normalize_title exactly -- it's part of this
    # table's uniqueness constraint (see db/schema.sql).
    return title.strip().lower()


def publish_file(cur, path: str) -> int:
    with open(path, "r", encoding="utf-8") as f:
        doc = json.load(f)

    entries = doc.get("media", [])
    if not entries:
        print(f"{path}: no media entries, skipping")
        return 0

    count = 0
    for entry in entries:
        title = entry.get("title", "").strip()
        if not title:
            print(f"{path}: skipping an entry with no title")
            continue
        service = entry.get("service", "") or ""

        cur.execute(
            """
            insert into public.online_filters (title, normalized_title, service, media, status)
            values (%s, %s, %s, %s, 'approved')
            on conflict (normalized_title, service) do update
                set title = excluded.title,
                    media = excluded.media,
                    status = 'approved',
                    updated_at = now()
            """,
            (title, normalize_title(title), service, Json(entry)),
        )
        count += 1
        print(f"  published {title!r} ({service or 'generic'}) -- {len(entry.get('cues', []))} cues")

    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("files", nargs="+", help="Filter JSON file(s) to publish")
    args = parser.parse_args()

    database_url = os.environ.get("NEON_DATABASE_URL")
    if not database_url:
        print("Set NEON_DATABASE_URL to your Neon direct connection string first.", file=sys.stderr)
        sys.exit(1)

    conn = psycopg2.connect(database_url)
    try:
        total = 0
        with conn:
            with conn.cursor() as cur:
                for path in args.files:
                    print(f"{path}:")
                    total += publish_file(cur, path)
        print(f"Published {total} entr{'y' if total == 1 else 'ies'}.")
    finally:
        conn.close()


if __name__ == "__main__":
    main()

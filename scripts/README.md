# scripts/

## publish_filter.py

Publishes a local filter file's entries (e.g. anything in `sample-filters/`)
to the online filter library the app's Select Filter → Online tab browses --
see `db/schema.sql` for the table this writes to and
`src-tauri/src/online.rs` for how the app reads it back.

### One-time Neon setup

1. Create a project at [neon.tech](https://neon.tech) (the free tier is
   plenty for this).
2. Run `db/schema.sql` against it -- easiest from the Neon console's SQL
   editor, or `psql "$NEON_DATABASE_URL" -f db/schema.sql`. Give
   `filter_reader` a real generated password in place of the placeholder
   before running it (e.g. `python3 -c "import secrets; print(secrets.token_urlsafe(24))"`).
3. Build `filter_reader`'s connection string (same host as your owner
   connection string, different user/password -- see below) and paste it
   into `src-tauri/src/online.rs`'s `READER_CONNECTION_STRING`. That one
   *is* meant to be committed and shipped in every build, the same way
   `metadata.rs`'s TMDB key is -- `filter_reader` can't do anything beyond
   what `db/schema.sql`'s RLS policy already allows (SELECT, `approved`
   rows only), so there's nothing meaningful for it to leak.
4. Grab the **owner** connection string too (Neon console → Connect) -- that
   one's only ever for this script, never the app itself. Don't commit it.
   (An earlier attempt at an HTTP-only read path via Neon's Data API is why
   you may also see an `anonymous` Postgres role in the console -- abandoned
   once every Data API request turned out to need a signed JWT even for
   that role; harmless to ignore.)

### Setup

```bash
pip install psycopg2-binary
export NEON_DATABASE_URL=postgres://<owner>:<password>@<host>/<db>?sslmode=require
```

### Usage

```bash
python3 scripts/publish_filter.py sample-filters/the-princess-bride.json
python3 scripts/publish_filter.py sample-filters/*.json
```

Every entry publishes as `approved` immediately -- this script is the only
writer today (see its doc comment for why: it connects with the database
owner's credentials, which bypass the Row Level Security policy the app
itself is limited to). Re-running for a title+service already published
overwrites its cues rather than duplicating.

## build_language_filter.py

Downloads a movie's captions from [SubDL](https://subdl.com/),
scans them for profanity and blasphemy, and writes/merges a cue entry into a
`sample-filters/*.json` file using the app's normal `{start, end, action,
category}` cue format. Categories come out as `language-profanity` and
`language-blasphemy` (flat strings, "language" prefix keeps them grouped —
see `src/lib/types.ts`'s `Cue.category`).

### Setup

```bash
pip install requests
export SUBDL_API_KEY=<key from https://subdl.com/panel/api>
```

Free SubDL keys are capped at 2,000 requests/day (a SubDL Pro key raises that
to 30,000/day) — plenty for one-off runs, but worth knowing if you're
batch-processing a big library.

### Usage

```bash
python3 scripts/build_language_filter.py "The Princess Bride" \
    --year 1987 --service "Disney+" \
    --output sample-filters/the-princess-bride.json
```

Running it again for the same title/service on the same `--output` file
replaces just that entry — other titles already in the file are left alone.

Useful flags:

- `--interactive` — pick which SubDL search result to use instead of
  auto-selecting the top hit.
- `--min-severity mild|moderate|strong` (default `moderate`) — how far down
  the word list to go. `mild` includes words like "damn"/"hell"/"crap".
- `--action mute|skip` (default `mute`).
- `--pad` / `--merge-gap` — seconds padded around each hit, and how close two
  same-category hits need to be before they're merged into one cue.
- `--srt-file <path>` — skip the SubDL download and scan a subtitle
  file you already have (handy for testing the word list offline).
- `--save-srt <path>` — also save the raw downloaded subtitle file.

### Word list

`scripts/wordlists/language_filter_words.json` holds the words/phrases each
category matches. Entries are base64-encoded (`word_b64`) so the file itself
doesn't show profanity in plaintext when you open it — that's obfuscation,
not real security, but it keeps the raw words out of your editor/terminal
during normal maintenance. It's just a starting point, not a definitive list.

Manage it with `scripts/wordlist_tool.py` instead of hand-editing the JSON:

```bash
# Add a word/phrase; prompts for it with hidden (password-style) input, so
# it's never echoed or left in shell history.
python3 scripts/wordlist_tool.py add --category profanity --severity strong

# List entries redacted (first letter only) -- severities/indexes without
# revealing words.
python3 scripts/wordlist_tool.py list

# Remove or re-tier an entry by the index shown in `list`.
python3 scripts/wordlist_tool.py remove --category profanity --index 3
python3 scripts/wordlist_tool.py severity --category profanity --index 3 --severity mild

# Deliberately print one entry's word -- the only command that does.
python3 scripts/wordlist_tool.py reveal --category profanity --index 3
```

### Caveats

- SubDL matching is fuzzy; always sanity-check the picked title
  (printed as `Using subtitle match: ...`), or pass `--interactive` /
  `--year` to be sure you got the right release.
- Word matching is literal (normalized for case/punctuation), so it won't
  catch censored spellings like `f**k` or `sh!t` — add likely variants to the
  word list if a given release's captions use them.
- Cue timing follows the subtitle line's own timing, which is sometimes a
  beat early/late relative to the actual audio — spot check a few cues
  against playback before trusting the file.

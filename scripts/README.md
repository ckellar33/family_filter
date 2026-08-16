# scripts/

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

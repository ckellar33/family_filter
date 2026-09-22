#!/usr/bin/env python3
"""Build a family_filter cue file for profanity/blasphemy from a movie's captions.

Downloads a subtitle file for the given title from SubDL, scans the
dialogue for profanity and blasphemy using an editable word list, and writes
(or merges into) a filter JSON file in the format family_filter expects:

    {
      "media": [
        {
          "title": "...",
          "service": "...",
          "cues": [
            { "start": "00:02:03.40", "end": "00:02:05.00", "action": "mute", "category": "language-profanity", "word": "s#@!" },
            ...
          ]
        }
      ]
    }

"language-profanity" and "language-blasphemy" are used as the category so the
app's flat `category` string still groups everything under "language" while
keeping the two kinds of hit distinguishable (see scripts/wordlists).

Each cue's "word" is whichever wordlist entry actually matched, masked the
same way the app's Filters tab masks it for display (first letter, then a
grawlix symbol per remaining character -- see censor_word, a Python port of
format.ts's censorWord) -- so the word never sits in the *output* file in
plaintext either, matching scripts/wordlists' own files, which base64-encode
entries for the same reason. When a burst of profanity gets merged into one
cue (see --merge-gap), "word" holds every distinct masked word involved,
joined with " / ". Pass --no-words to leave "word" off entirely instead --
even masked, it still reveals a word's first letter and length, which
--no-words avoids for anyone who'd rather the file carry no word info at
all.

Setup
-----
    pip install requests
    export SUBDL_API_KEY=<your key from https://subdl.com/panel/api>

Usage
-----
    python3 scripts/build_language_filter.py "The Princess Bride" \\
        --year 1987 --service "Disney+" \\
        --output sample-filters/the-princess-bride.json

    # Pick which search result to use instead of auto-selecting the top hit:
    python3 scripts/build_language_filter.py "Star Wars" --year 1977 --interactive \\
        --output sample-filters/star-wars.json

    # Skip the network call and scan a subtitle file you already have (handy
    # for testing the word list, or if you sourced captions another way):
    python3 scripts/build_language_filter.py "The Princess Bride" \\
        --srt-file ~/Downloads/princess-bride.srt \\
        --service "Disney+" --output sample-filters/the-princess-bride.json

    # Cross-check against multiple subtitle releases instead of trusting a
    # single one: downloads the top N SubDL matches (WEB-tagged only by
    # default -- see --release-filter), scans each separately, and for hits
    # on the same word that land within --cross-check-tolerance seconds of
    # each other across sources, uses the timing from whichever source has
    # agreed with the others most often rather than whichever file's own
    # sync happens to be off:
    python3 scripts/build_language_filter.py "The Princess Bride" \\
        --year 1987 --sources 3 --service "Disney+" \\
        --output sample-filters/the-princess-bride.json
"""

from __future__ import annotations

import argparse
import base64
import io
import json
import os
import re
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import requests

SUBDL_SEARCH_URL = "https://api.subdl.com/api/v1/subtitles"
SUBDL_LINK_PREFIX = "https://dl.subdl.com"
USER_AGENT = "family-filter-caption-scanner/1.0"
DEFAULT_WORDLIST_PATH = Path(__file__).parent / "wordlists" / "language_filter_words.json"
SEVERITY_ORDER = {"mild": 0, "moderate": 1, "strong": 2}
# Matches release/file names tagged WEB in any form -- WEBRip, WEB-DL,
# WEB.DL, WEBHD, etc. (case-insensitive, applied via re.IGNORECASE at
# compile time). A stricter "WEB-DL only" pattern was tried first, but
# WEB-DL turns out to be rare on SubDL -- most streaming-sourced subtitles
# there are tagged WEBRip -- so it filtered out results without leaving
# anything to actually use. "web" alone still excludes BluRay/DVD/HDTV
# disc-rip releases (different cut/runtime than what's actually streaming)
# while keeping every release sourced from a streaming service, which is
# most subtitles' realistic best match to what family_filter users are
# watching. This is the default --release-filter pattern.
DEFAULT_RELEASE_FILTER = r"web"
TIME_RE = re.compile(
    r"(\d{2}):(\d{2}):(\d{2})[,.](\d{3})\s*-->\s*(\d{2}):(\d{2}):(\d{2})[,.](\d{3})"
)
TAG_RE = re.compile(r"<[^>]+>|\{[^}]*\}")
NON_WORD_RE = re.compile(r"[^a-z0-9'\s]")
WHITESPACE_RE = re.compile(r"\s+")


@dataclass
class SubtitleEntry:
    start: float
    end: float
    text: str


@dataclass
class Cue:
    start: float
    end: float
    action: str
    category: str
    # Whichever wordlist entry matched, already masked (see censor_word) by
    # the time scan_entries puts it here -- this field never holds
    # plaintext. "" until scan_entries finds one, and merge_cues combines
    # every distinct masked word from cues it folds together (see
    # _combine_words). Written out under the "word" key, but only when
    # non-empty (see write_filter_file) -- --no-words clears every cue's
    # word back to "" before writing so the key is omitted entirely,
    # matching the pre-word-field output.
    word: str = ""


# --------------------------------------------------------------------------
# Subtitle parsing
# --------------------------------------------------------------------------

def _timecode_to_seconds(h: str, m: str, s: str, ms: str) -> float:
    return int(h) * 3600 + int(m) * 60 + int(s) + int(ms) / 1000.0


def parse_subtitles(raw: str) -> list[SubtitleEntry]:
    """Parses .srt (and .vtt, which uses the same '-->' timecode line) text."""
    text = raw.replace("\r\n", "\n").replace("\r", "\n")
    entries: list[SubtitleEntry] = []
    for block in re.split(r"\n\s*\n", text.strip()):
        lines = [line for line in block.split("\n") if line.strip() != ""]
        for i, line in enumerate(lines):
            m = TIME_RE.search(line)
            if not m:
                continue
            start = _timecode_to_seconds(*m.group(1, 2, 3, 4))
            end = _timecode_to_seconds(*m.group(5, 6, 7, 8))
            body = "\n".join(lines[i + 1 :])
            if body.strip():
                entries.append(SubtitleEntry(start, end, body))
            break
    return entries


def normalize(text: str) -> str:
    """Strips subtitle formatting/tags and punctuation, lowercases, collapses
    whitespace -- so word-list phrases match regardless of styling or
    punctuation around them."""
    text = TAG_RE.sub(" ", text)
    text = text.lower()
    text = NON_WORD_RE.sub(" ", text)
    text = WHITESPACE_RE.sub(" ", text).strip()
    return text


# Comic-strip grawlix symbols -- must stay identical to format.ts's GRAWLIX
# (same set, same order) so a word masked here by this script and a word
# masked by the app's own display logic look the same, whichever one
# actually did the masking.
GRAWLIX = ["#", "@", "&", "!", "*", "%"]


def censor_word(word: str) -> str:
    """Python port of format.ts's censorWord: first letter kept, every other
    character replaced with a grawlix symbol keyed off that character's own
    code point -- e.g. "shit" -> "s#@!". Multi-word phrases censor each word
    separately, same as the TS version. Applied to a word/phrase *before* it
    ever reaches a Cue, so the plaintext match never makes it into the
    output file -- see scan_entries."""
    def censor_one(w: str) -> str:
        if len(w) <= 1:
            return w
        return w[0] + "".join(GRAWLIX[ord(ch) % len(GRAWLIX)] for ch in w[1:])

    return " ".join(censor_one(w) for w in word.split(" "))


# --------------------------------------------------------------------------
# Word list
# --------------------------------------------------------------------------

def entry_word(entry: dict) -> str:
    """Words are stored base64-encoded ('word_b64') so the wordlist file
    doesn't show profanity in plaintext; a plain 'word' key still works too,
    for entries added by hand."""
    if "word_b64" in entry:
        return base64.b64decode(entry["word_b64"]).decode("utf-8")
    return entry["word"]


def load_matchers(wordlist_path: Path, min_severity: str) -> dict[str, list[tuple[str, re.Pattern]]]:
    """Returns each category's patterns paired with the (normalized,
    plaintext) word each one matches -- scan_entries needs the word itself,
    not just whether *some* pattern in the category hit, so it can record
    which word landed in each cue."""
    with open(wordlist_path, "r", encoding="utf-8") as f:
        raw = json.load(f)

    min_rank = SEVERITY_ORDER[min_severity]
    matchers: dict[str, list[tuple[str, re.Pattern]]] = {}
    for category in ("profanity", "blasphemy"):
        patterns = []
        for entry in raw.get(category, []):
            if SEVERITY_ORDER.get(entry.get("severity", "mild"), 0) < min_rank:
                continue
            word = normalize(entry_word(entry))
            if not word:
                continue
            patterns.append((word, re.compile(r"\b" + re.escape(word) + r"\b")))
        matchers[category] = patterns
    return matchers


# --------------------------------------------------------------------------
# Scanning + cue merging
# --------------------------------------------------------------------------

def scan_entries(
    entries: list[SubtitleEntry], matchers: dict[str, list[tuple[str, re.Pattern]]]
) -> list[Cue]:
    hits: list[Cue] = []
    for entry in entries:
        normalized = normalize(entry.text)
        if not normalized:
            continue
        for category, patterns in matchers.items():
            # One hit per *word* that matched, not just one per category --
            # a line with two different profane words (e.g. "fuck this
            # shit") needs both recorded so merge_cues can combine them into
            # the merged cue's word field, rather than only ever knowing
            # "something in this category matched somewhere in the line".
            # Masked immediately (see censor_word) so the plaintext match
            # never travels any further than this loop, let alone into the
            # output file.
            for word, pattern in patterns:
                if pattern.search(normalized):
                    hits.append(Cue(entry.start, entry.end, "mute", f"language-{category}", word=censor_word(word)))
    return hits


def _combine_words(a: str, b: str) -> str:
    """Folds `b` into `a`'s " / "-joined word list, in first-seen order,
    without duplicating a word two merged cues both happened to match."""
    if not a:
        return b
    if not b:
        return a
    words = a.split(" / ")
    if b not in words:
        words.append(b)
    return " / ".join(words)


def merge_cues(cues: list[Cue], pad: float, merge_gap: float, action: str) -> list[Cue]:
    """Pads each cue, then merges overlapping/near cues within the same
    category so a burst of profanity in one exchange doesn't chop the audio
    into a dozen tiny mutes -- folding every distinct word involved into the
    merged cue's own word (see _combine_words) rather than keeping only
    whichever word happened to start the group."""
    by_category: dict[str, list[Cue]] = {}
    for cue in cues:
        by_category.setdefault(cue.category, []).append(cue)

    merged: list[Cue] = []
    for category, group in by_category.items():
        group.sort(key=lambda c: c.start)
        current: Optional[Cue] = None
        for cue in group:
            start = max(0.0, cue.start - pad)
            end = cue.end + pad
            if current is None:
                current = Cue(start, end, action, category, word=cue.word)
                continue
            if start <= current.end + merge_gap:
                current.end = max(current.end, end)
                current.word = _combine_words(current.word, cue.word)
            else:
                merged.append(current)
                current = Cue(start, end, action, category, word=cue.word)
        if current is not None:
            merged.append(current)

    merged.sort(key=lambda c: c.start)
    return merged


def reconcile_sources(
    sources_hits: list[list[Cue]], tolerance: float, source_names: Optional[list[str]] = None
) -> list[Cue]:
    """Cross-checks raw hits scanned from several subtitle releases of the
    same title and folds them into one hit list, using timing agreement
    between releases as a signal of which timestamp is trustworthy.

    SubDL's search API doesn't return a rating, vote count, or download
    count for individual subtitle files to prefer the "best" one by -- see
    subdl_search_sources. This builds a proxy instead: different releases
    of the same subtitle are frequently offset from each other (different
    frame rate, a few extra/missing lines of credits, remuxed cut, ...), so
    a release whose hits keep landing close to *other* releases' hits for
    the same word is, empirically, one of the better-synced ones, while a
    release that's out of step will keep drifting outside the tolerance
    window and showing up alone.

    Hits are grouped by (category, masked word) -- since scan_entries
    already masks the plaintext, this is the closest thing available to
    "the same word" -- then, within each group, hits that land within
    `tolerance` seconds of each other are clustered as (probably) the same
    utterance seen across releases (pass 1). Each source is then scored by
    what fraction of its hits fell into a cluster corroborated by at least
    one other source (pass 2's input). A cluster backed by two or more
    sources takes its final timing from whichever contributing source has
    the *highest* score -- literally "use the release that's agreed with
    everyone else most often" -- falling back to the median of tied top
    scorers; a singleton cluster is kept as-is (better to keep an
    uncorroborated hit than silently drop real profanity/blasphemy just
    because only one release carried that line).
    """
    n = len(sources_hits)
    tagged = [
        (src_idx, cue) for src_idx, hits in enumerate(sources_hits) for cue in hits
    ]
    by_key: dict[tuple[str, str], list[tuple[int, Cue]]] = {}
    for src_idx, cue in tagged:
        by_key.setdefault((cue.category, cue.word), []).append((src_idx, cue))

    # Pass 1: cluster by time proximity within each (category, word) group.
    clusters: list[list[tuple[int, Cue]]] = []
    for items in by_key.values():
        items.sort(key=lambda t: t[1].start)
        current: list[tuple[int, Cue]] = []
        for src_idx, cue in items:
            if current and cue.start - current[-1][1].start > tolerance:
                clusters.append(current)
                current = []
            current.append((src_idx, cue))
        if current:
            clusters.append(current)

    # Same source can contribute more than one nearby hit within a cluster
    # (e.g. a word repeated across two adjacent lines); collapse those down
    # to one timing per source before judging agreement, so a single
    # chatty release can't outvote a second release that only logged the
    # line once.
    deduped: list[list[tuple[int, Cue]]] = []
    for cluster in clusters:
        by_source: dict[int, Cue] = {}
        for src_idx, cue in cluster:
            if src_idx not in by_source or cue.start < by_source[src_idx].start:
                by_source[src_idx] = cue
        deduped.append(list(by_source.items()))

    # Score each source: what fraction of its own hits ended up agreeing
    # (within tolerance) with at least one other source.
    corroborated_count = [0] * n
    total_count = [0] * n
    for cluster in deduped:
        agreed = len(cluster) > 1
        for src_idx, _ in cluster:
            total_count[src_idx] += 1
            if agreed:
                corroborated_count[src_idx] += 1
    scores = [(corroborated_count[i] / total_count[i]) if total_count[i] else 0.0 for i in range(n)]

    if n > 1:
        print("Source agreement (proxy for sync accuracy -- SubDL exposes no rating/vote/download field to rank by):")
        for rank, i in enumerate(sorted(range(n), key=lambda i: (-scores[i], -total_count[i])), start=1):
            label = source_names[i] if source_names else f"source {i}"
            print(f"  {rank}. {label!r}: {corroborated_count[i]}/{total_count[i]} hits agreed with another source ({scores[i]:.0%})")

    # Pass 2: finalize each cluster's timing, preferring the highest-scored
    # contributing source over a flat median.
    reconciled: list[Cue] = []
    corroborated_total = 0
    singleton_total = 0
    for cluster in deduped:
        if len(cluster) == 1:
            singleton_total += 1
            reconciled.append(cluster[0][1])
            continue

        corroborated_total += 1
        best_score = max(scores[src_idx] for src_idx, _ in cluster)
        best = [cue for src_idx, cue in cluster if scores[src_idx] == best_score]
        if len(best) == 1:
            reconciled.append(best[0])
        else:
            starts = sorted(c.start for c in best)
            ends = sorted(c.end for c in best)
            mid = len(starts) // 2
            if len(starts) % 2:
                final_start, final_end = starts[mid], ends[mid]
            else:
                final_start = (starts[mid - 1] + starts[mid]) / 2
                final_end = (ends[mid - 1] + ends[mid]) / 2
            ref = best[0]
            reconciled.append(Cue(final_start, final_end, ref.action, ref.category, word=ref.word))

    reconciled.sort(key=lambda c: c.start)
    if n > 1:
        print(
            f"Cross-checked {n} subtitle sources: {corroborated_total} hit(s) corroborated by 2+ sources "
            f"(using the highest-agreement source's timing), {singleton_total} found in only one source."
        )
    return reconciled


# --------------------------------------------------------------------------
# SubDL API
# --------------------------------------------------------------------------

class SubDLError(RuntimeError):
    pass


def _matches_release_filter(result: dict, pattern: re.Pattern) -> bool:
    haystack = f"{result.get('release_name', '')} {result.get('name', '')}"
    return bool(pattern.search(haystack))


def subdl_search_list(
    api_key: str,
    title: str,
    year: Optional[int],
    language: str,
    release_filter: Optional[re.Pattern] = None,
) -> list[dict]:
    """Returns the list of search-result dicts from SubDL, best match
    first -- both subdl_search (single-result) and subdl_search_sources
    (cross-check, multi-result) pick from this. When `release_filter` is
    given, results whose release_name/name don't match it (case-
    insensitively) are dropped before returning -- see
    DEFAULT_RELEASE_FILTER for why WEB-tagged releases are the default."""
    params = {
        "api_key": api_key,
        "film_name": title,
        "type": "movie",
        "languages": language,
        "subs_per_page": 30,
    }
    if year:
        params["year"] = str(year)

    resp = requests.get(
        SUBDL_SEARCH_URL,
        headers={"User-Agent": USER_AGENT},
        params=params,
        timeout=30,
    )
    data = resp.json() if resp.content else {}
    if resp.status_code != 200 or not data.get("status", False):
        raise SubDLError(f"Search failed ({resp.status_code}): {data.get('error') or data.get('message') or resp.text}")

    subtitles = data.get("subtitles", [])
    if not subtitles:
        raise SubDLError(f"No subtitles found for {title!r} (year={year}, lang={language}).")

    if release_filter is not None:
        filtered = [s for s in subtitles if _matches_release_filter(s, release_filter)]
        if not filtered:
            raise SubDLError(
                f"{len(subtitles)} subtitle(s) found for {title!r} but none matched "
                f"--release-filter {release_filter.pattern!r}. Pass --release-filter '' to "
                "consider all releases."
            )
        subtitles = filtered
    return subtitles


def _result_url_and_title(result: dict, fallback_title: str) -> tuple[str, str]:
    url = result.get("url")
    if not url:
        raise SubDLError("Chosen search result has no downloadable file.")
    matched_title = result.get("release_name") or result.get("name") or fallback_title
    return SUBDL_LINK_PREFIX + url, matched_title


def subdl_search(
    api_key: str,
    title: str,
    year: Optional[int],
    language: str,
    interactive: bool,
    release_filter: Optional[re.Pattern] = None,
) -> tuple[str, str]:
    """Single-result lookup (the pre-cross-check behavior): returns
    (download_url, matched_release_name) for either the top hit, or
    whichever one the user picks in --interactive mode."""
    subtitles = subdl_search_list(api_key, title, year, language, release_filter)

    if interactive:
        print(f"\nFound {len(subtitles)} result(s) for {title!r}:")
        for i, s in enumerate(subtitles):
            print(
                f"  [{i}] {s.get('release_name', s.get('name', '?'))} "
                f"-- lang: {s.get('lang', '?')} -- author: {s.get('author', '?')}"
            )
        choice = input(f"Pick a result [0-{len(subtitles) - 1}] (default 0): ").strip()
        index = int(choice) if choice else 0
    else:
        index = 0

    return _result_url_and_title(subtitles[index], title)


def subdl_search_sources(
    api_key: str,
    title: str,
    year: Optional[int],
    language: str,
    count: int,
    release_filter: Optional[re.Pattern] = None,
) -> list[tuple[str, str]]:
    """Cross-check lookup: returns up to `count` (download_url,
    matched_release_name) pairs, taken off the top of SubDL's ranked
    results, for reconcile_sources to compare against each other. Always
    non-interactive -- picking one release defeats the point of
    cross-checking several."""
    subtitles = subdl_search_list(api_key, title, year, language, release_filter)
    if len(subtitles) < count:
        print(
            f"warning: only {len(subtitles)} subtitle result(s) available for {title!r}, "
            f"requested --sources {count}.",
            file=sys.stderr,
        )
    return [_result_url_and_title(s, title) for s in subtitles[:count]]


def subdl_download(download_url: str) -> str:
    resp = requests.get(download_url, headers={"User-Agent": USER_AGENT}, timeout=30)
    resp.raise_for_status()
    content = resp.content

    # SubDL normally packages subtitles in a zip; a handful of "unpacked"
    # links serve the raw file directly, so only unzip when it looks zipped.
    if content[:2] == b"PK":
        with zipfile.ZipFile(io.BytesIO(content)) as zf:
            candidates = [n for n in zf.namelist() if n.lower().endswith((".srt", ".vtt"))]
            if not candidates:
                raise SubDLError("Downloaded zip contained no .srt/.vtt file.")
            # Prefer .srt over .vtt if both are present; otherwise first match.
            candidates.sort(key=lambda n: (not n.lower().endswith(".srt"), n))
            content = zf.read(candidates[0])

    try:
        return content.decode("utf-8")
    except UnicodeDecodeError:
        return content.decode("latin-1")


# --------------------------------------------------------------------------
# Output file
# --------------------------------------------------------------------------

def _dump_json(obj, level: int = 0) -> str:
    """Like json.dumps(obj, indent=2), except any list found under a "cues"
    key is rendered with each cue object on a single line -- matching the
    compact style used across sample-filters/*.json."""
    pad = "  " * level
    pad_in = "  " * (level + 1)
    if isinstance(obj, dict):
        if not obj:
            return "{}"
        items = []
        for k, v in obj.items():
            if k == "cues" and isinstance(v, list):
                rendered = _dump_cues(v, level + 1)
            else:
                rendered = _dump_json(v, level + 1)
            items.append(f'{pad_in}"{k}": {rendered}')
        return "{\n" + ",\n".join(items) + "\n" + pad + "}"
    if isinstance(obj, list):
        if not obj:
            return "[]"
        items = [pad_in + _dump_json(x, level + 1) for x in obj]
        return "[\n" + ",\n".join(items) + "\n" + pad + "]"
    return json.dumps(obj)


def _dump_cues(cues: list[dict], level: int) -> str:
    pad = "  " * level
    pad_in = "  " * (level + 1)
    if not cues:
        return "[]"
    lines = [
        pad_in + "{ " + ", ".join(f'"{k}": {json.dumps(v)}' for k, v in cue.items()) + " }"
        for cue in cues
    ]
    return "[\n" + ",\n".join(lines) + "\n" + pad + "]"


def _seconds_to_hms(total: float) -> str:
    """Python port of filter.rs's seconds_to_hms: zero-padded
    "HH:MM:SS.ss" -- the same on-disk format `FilterList::save` writes, so a
    cue this script produces lines up with a video's own on-screen timestamp
    and matches whatever the app itself would write for the same cue.
    Rounds to whole centiseconds before decomposing into h/m/s/cs (rather
    than formatting the float directly) so e.g. 59.999999 seconds of float
    imprecision rolls over into the next minute instead of printing "60.00"."""
    total = max(total, 0.0) if total == total and total not in (float("inf"), float("-inf")) else 0.0
    total_centis = int(total * 100.0 + 0.5)
    centis = total_centis % 100
    total_secs = total_centis // 100
    secs = total_secs % 60
    total_mins = total_secs // 60
    mins = total_mins % 60
    hours = total_mins // 60
    return f"{hours:02d}:{mins:02d}:{secs:02d}.{centis:02d}"


def _cue_dict(c: Cue) -> dict:
    """"word" is only ever present when non-empty -- mirrors the Rust side's
    `#[serde(skip_serializing_if = "Option::is_none")]` on filter::Cue::word,
    so a cue nothing matched a word for round-trips exactly like it did
    before this field existed, and --no-words output is indistinguishable
    from the pre-word-field format."""
    d = {"start": _seconds_to_hms(c.start), "end": _seconds_to_hms(c.end), "action": c.action, "category": c.category}
    if c.word:
        d["word"] = c.word
    return d


def write_filter_file(output_path: Path, title: str, service: str, cues: list[Cue]) -> None:
    entry = {
        "title": title,
        "cues": [_cue_dict(c) for c in cues],
    }
    if service:
        entry["service"] = service

    doc = {"media": [entry]}

    output_path.parent.mkdir(parents=True, exist_ok=True)
    with open(output_path, "w", encoding="utf-8") as f:
        f.write(_dump_json(doc))
        f.write("\n")

    print(f"Wrote entry for {title!r} to {output_path} ({len(cues)} cues).")


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("title", help="Movie title to search for / to record in the filter file.")
    parser.add_argument("--year", type=int, default=None, help="Release year, improves SubDL matching.")
    parser.add_argument("--service", default="", help='Streaming service to record on the entry (e.g. "Disney+").')
    parser.add_argument("--language", default="en", help="Subtitle language code to search for. Default: en.")
    parser.add_argument("--output", "-o", required=True, type=Path, help="Filter JSON path to write/merge into.")
    parser.add_argument("--wordlist", type=Path, default=DEFAULT_WORDLIST_PATH, help="Word list JSON to scan with.")
    parser.add_argument(
        "--min-severity", choices=["mild", "moderate", "strong"], default="mild",
        help="Ignore word-list entries below this severity. Default: mild (all entries).",
    )
    parser.add_argument("--action", choices=["mute", "skip"], default="mute", help="Cue action. Default: mute.")
    parser.add_argument("--pad", type=float, default=0.15, help="Seconds padded before/after each hit. Default: 0.15.")
    parser.add_argument(
        "--merge-gap", type=float, default=0.75,
        help="Merge same-category cues within this many seconds of each other. Default: 0.75.",
    )
    parser.add_argument(
        "--no-words", action="store_true",
        help="Don't populate cues' 'word' field at all -- 'word' is always masked (see censor_word) even without "
        "this flag, but leaving it off entirely means the file carries no word info -- not even a first letter "
        "and length -- matching this script's behavior before that field existed.",
    )
    parser.add_argument("--api-key", default=os.environ.get("SUBDL_API_KEY"))
    parser.add_argument(
        "--interactive", action="store_true",
        help="Choose which SubDL search result to use instead of auto-picking the top one. "
        "Not usable together with --sources > 1.",
    )
    parser.add_argument(
        "--sources", type=int, default=1,
        help="Download this many top SubDL results and cross-check them against each other "
        "instead of trusting a single release's timing: hits on the same word within "
        "--cross-check-tolerance seconds of each other across sources are folded into one "
        "cue using the median timestamp. Default: 1 (no cross-check).",
    )
    parser.add_argument(
        "--cross-check-tolerance", type=float, default=1.5,
        help="When --sources > 1, hits on the same word across sources within this many "
        "seconds of each other are treated as the same utterance. Default: 1.5.",
    )
    parser.add_argument(
        "--release-filter", default=DEFAULT_RELEASE_FILTER,
        help="Case-insensitive regex a SubDL result's release/file name must match to be "
        "considered. Defaults to any WEB-tagged release (matches 'WEBRip', 'WEB-DL', 'WEBHD', "
        "etc.) since those are sourced from a streaming service and tend to already line up "
        "with what's playing, unlike a disc-rip release (BluRay/DVD) with a different cut or "
        "runtime. Pass '' to consider all releases, or a stricter pattern like 'web[-. ]?dl' to "
        "require true WEB-DL (uncommon on SubDL -- most streaming-sourced subs there are "
        "tagged WEBRip instead).",
    )
    parser.add_argument("--save-srt", type=Path, default=None, help="Also save the raw downloaded subtitle file(s) here.")
    parser.add_argument(
        "--srt-file", type=Path, default=None,
        help="Skip the SubDL download and scan this local .srt/.vtt file instead.",
    )
    args = parser.parse_args()

    if args.sources < 1:
        parser.error("--sources must be >= 1.")
    if args.interactive and args.sources > 1:
        parser.error("--interactive picks a single result, which conflicts with --sources > 1.")
    if args.srt_file and args.sources > 1:
        parser.error("--srt-file scans one local file, which conflicts with --sources > 1.")

    release_filter: Optional[re.Pattern] = None
    if args.release_filter:
        try:
            release_filter = re.compile(args.release_filter, re.IGNORECASE)
        except re.error as e:
            parser.error(f"--release-filter is not a valid regex: {e}")

    matchers = load_matchers(args.wordlist, args.min_severity)

    if args.srt_file:
        raw = args.srt_file.read_text(encoding="utf-8", errors="replace")
        if args.save_srt:
            args.save_srt.parent.mkdir(parents=True, exist_ok=True)
            args.save_srt.write_text(raw, encoding="utf-8")
            print(f"Saved raw subtitle file to {args.save_srt}")

        entries = parse_subtitles(raw)
        if not entries:
            print("error: no subtitle cues could be parsed from the caption file.", file=sys.stderr)
            return 1
        print(f"Parsed {len(entries)} subtitle lines.")
        hits = scan_entries(entries, matchers)
        print(f"Found {len(hits)} raw hits before merging.")
    else:
        if not args.api_key:
            parser.error("--api-key or SUBDL_API_KEY is required unless --srt-file is given.")
        try:
            if args.sources > 1:
                downloads = subdl_search_sources(
                    args.api_key, args.title, args.year, args.language, args.sources, release_filter
                )
            else:
                downloads = [
                    subdl_search(args.api_key, args.title, args.year, args.language, args.interactive, release_filter)
                ]
        except SubDLError as e:
            print(f"error: {e}", file=sys.stderr)
            return 1

        # Each source is downloaded, parsed, and scanned independently so
        # reconcile_sources can compare their hit timings against each
        # other below -- merging the raw subtitle text together first
        # wouldn't let it tell which release a given hit came from.
        sources_hits: list[list[Cue]] = []
        source_names: list[str] = []
        for i, (download_url, matched_title) in enumerate(downloads):
            print(f"Using subtitle match: {matched_title!r}")
            try:
                raw = subdl_download(download_url)
            except SubDLError as e:
                print(f"error: {e}", file=sys.stderr)
                return 1

            if args.save_srt:
                save_path = args.save_srt
                if len(downloads) > 1:
                    save_path = save_path.with_name(f"{save_path.stem}.{i}{save_path.suffix}")
                save_path.parent.mkdir(parents=True, exist_ok=True)
                save_path.write_text(raw, encoding="utf-8")
                print(f"Saved raw subtitle file to {save_path}")

            entries = parse_subtitles(raw)
            if not entries:
                print(f"warning: no subtitle cues could be parsed from {matched_title!r}, skipping.", file=sys.stderr)
                continue
            print(f"Parsed {len(entries)} subtitle lines from {matched_title!r}.")
            sources_hits.append(scan_entries(entries, matchers))
            source_names.append(matched_title)

        if not sources_hits:
            print("error: no subtitle source yielded any parseable cues.", file=sys.stderr)
            return 1

        total_raw = sum(len(h) for h in sources_hits)
        print(f"Found {total_raw} raw hit(s) across {len(sources_hits)} source(s) before reconciling.")
        hits = reconcile_sources(sources_hits, args.cross_check_tolerance, source_names)
        if len(sources_hits) > 1:
            print(f"{len(hits)} hit(s) remain after cross-checking sources.")

    cues = merge_cues(hits, args.pad, args.merge_gap, args.action)
    if args.no_words:
        for cue in cues:
            cue.word = ""
    write_filter_file(args.output, args.title, args.service, cues)
    return 0


if __name__ == "__main__":
    sys.exit(main())

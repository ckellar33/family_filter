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
            { "start": 123.4, "end": 125.0, "action": "mute", "category": "language-profanity" },
            ...
          ]
        }
      ]
    }

"language-profanity" and "language-blasphemy" are used as the category so the
app's flat `category` string still groups everything under "language" while
keeping the two kinds of hit distinguishable (see scripts/wordlists).

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


def load_matchers(wordlist_path: Path, min_severity: str) -> dict[str, list[re.Pattern]]:
    with open(wordlist_path, "r", encoding="utf-8") as f:
        raw = json.load(f)

    min_rank = SEVERITY_ORDER[min_severity]
    matchers: dict[str, list[re.Pattern]] = {}
    for category in ("profanity", "blasphemy"):
        patterns = []
        for entry in raw.get(category, []):
            if SEVERITY_ORDER.get(entry.get("severity", "mild"), 0) < min_rank:
                continue
            word = normalize(entry_word(entry))
            if not word:
                continue
            patterns.append(re.compile(r"\b" + re.escape(word) + r"\b"))
        matchers[category] = patterns
    return matchers


# --------------------------------------------------------------------------
# Scanning + cue merging
# --------------------------------------------------------------------------

def scan_entries(
    entries: list[SubtitleEntry], matchers: dict[str, list[re.Pattern]]
) -> list[Cue]:
    hits: list[Cue] = []
    for entry in entries:
        normalized = normalize(entry.text)
        if not normalized:
            continue
        for category, patterns in matchers.items():
            if any(p.search(normalized) for p in patterns):
                hits.append(Cue(entry.start, entry.end, "mute", f"language-{category}"))
    return hits


def merge_cues(cues: list[Cue], pad: float, merge_gap: float, action: str) -> list[Cue]:
    """Pads each cue, then merges overlapping/near cues within the same
    category so a burst of profanity in one exchange doesn't chop the audio
    into a dozen tiny mutes."""
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
                current = Cue(start, end, action, category)
                continue
            if start <= current.end + merge_gap:
                current.end = max(current.end, end)
            else:
                merged.append(current)
                current = Cue(start, end, action, category)
        if current is not None:
            merged.append(current)

    merged.sort(key=lambda c: c.start)
    return merged


# --------------------------------------------------------------------------
# SubDL API
# --------------------------------------------------------------------------

class SubDLError(RuntimeError):
    pass


def subdl_search(
    api_key: str,
    title: str,
    year: Optional[int],
    language: str,
    interactive: bool,
) -> tuple[str, str]:
    """Returns (download_url, matched_release_name)."""
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

    chosen = subtitles[index]
    url = chosen.get("url")
    if not url:
        raise SubDLError("Chosen search result has no downloadable file.")
    matched_title = chosen.get("release_name") or chosen.get("name") or title
    return SUBDL_LINK_PREFIX + url, matched_title


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


def write_filter_file(output_path: Path, title: str, service: str, cues: list[Cue]) -> None:
    entry = {
        "title": title,
        "cues": [
            {"start": round(c.start, 2), "end": round(c.end, 2), "action": c.action, "category": c.category}
            for c in cues
        ],
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
    parser.add_argument("--api-key", default=os.environ.get("SUBDL_API_KEY"))
    parser.add_argument(
        "--interactive", action="store_true",
        help="Choose which SubDL search result to use instead of auto-picking the top one.",
    )
    parser.add_argument("--save-srt", type=Path, default=None, help="Also save the raw downloaded subtitle file here.")
    parser.add_argument(
        "--srt-file", type=Path, default=None,
        help="Skip the SubDL download and scan this local .srt/.vtt file instead.",
    )
    args = parser.parse_args()

    if args.srt_file:
        raw = args.srt_file.read_text(encoding="utf-8", errors="replace")
    else:
        if not args.api_key:
            parser.error("--api-key or SUBDL_API_KEY is required unless --srt-file is given.")
        try:
            download_url, matched_title = subdl_search(
                args.api_key, args.title, args.year, args.language, args.interactive
            )
            print(f"Using subtitle match: {matched_title!r}")
            raw = subdl_download(download_url)
        except SubDLError as e:
            print(f"error: {e}", file=sys.stderr)
            return 1

    if args.save_srt:
        args.save_srt.parent.mkdir(parents=True, exist_ok=True)
        args.save_srt.write_text(raw, encoding="utf-8")
        print(f"Saved raw subtitle file to {args.save_srt}")

    entries = parse_subtitles(raw)
    if not entries:
        print("error: no subtitle cues could be parsed from the caption file.", file=sys.stderr)
        return 1
    print(f"Parsed {len(entries)} subtitle lines.")

    matchers = load_matchers(args.wordlist, args.min_severity)
    hits = scan_entries(entries, matchers)
    print(f"Found {len(hits)} raw hits before merging.")

    cues = merge_cues(hits, args.pad, args.merge_gap, args.action)
    write_filter_file(args.output, args.title, args.service, cues)
    return 0


if __name__ == "__main__":
    sys.exit(main())

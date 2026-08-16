#!/usr/bin/env python3
"""Manage scripts/wordlists/language_filter_words.json without displaying
profanity on screen or in shell history.

Words are stored base64-encoded ('word_b64') in the JSON file -- that's
obfuscation so the file doesn't show plaintext profanity when you open it,
not real security; anyone who runs `base64 -d` can read it. This tool just
keeps you from having to look at (or type into a visible terminal) the words
themselves during normal list maintenance.

Usage
-----
    # Add a word/phrase; prompts for it with hidden input (like a password
    # prompt) so it's never echoed or left in shell history.
    python3 scripts/wordlist_tool.py add --category profanity --severity strong

    # List entries without revealing their words.
    python3 scripts/wordlist_tool.py list

    # Remove an entry by the index shown in `list`.
    python3 scripts/wordlist_tool.py remove --category profanity --index 3

    # Change an entry's severity by index, without revealing its word.
    python3 scripts/wordlist_tool.py severity --category profanity --index 3 --severity mild

    # Deliberately reveal one entry's word (e.g. to double check a match) --
    # the only command that ever prints a word.
    python3 scripts/wordlist_tool.py reveal --category profanity --index 3
"""

from __future__ import annotations

import argparse
import base64
import getpass
import json
from pathlib import Path

DEFAULT_WORDLIST_PATH = Path(__file__).parent / "wordlists" / "language_filter_words.json"
CATEGORIES = ("profanity", "blasphemy")
SEVERITIES = ("mild", "moderate", "strong")


def load(path: Path) -> dict:
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def save(path: Path, doc: dict) -> None:
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")


def encode(word: str) -> str:
    return base64.b64encode(word.encode("utf-8")).decode("ascii")


def decode(word_b64: str) -> str:
    return base64.b64decode(word_b64).decode("utf-8")


def cmd_add(args: argparse.Namespace) -> int:
    doc = load(args.wordlist)
    word = getpass.getpass(f"Word/phrase to add to {args.category} ({args.severity}), hidden input: ").strip()
    if not word:
        print("error: empty input, nothing added.")
        return 1
    doc.setdefault(args.category, []).append({"word_b64": encode(word), "severity": args.severity})
    save(args.wordlist, doc)
    print(f"Added 1 entry to {args.category} (index {len(doc[args.category]) - 1}, severity={args.severity}).")
    return 0


def cmd_list(args: argparse.Namespace) -> int:
    doc = load(args.wordlist)
    for category in CATEGORIES:
        entries = doc.get(category, [])
        print(f"\n{category} ({len(entries)} entries):")
        for i, entry in enumerate(entries):
            word = decode(entry["word_b64"]) if "word_b64" in entry else entry.get("word", "")
            redacted = word[0] + "*" * max(len(word) - 1, 0) if word else "?"
            print(f"  [{i}] {redacted:<20} severity={entry.get('severity', '?')}")
    return 0


def cmd_remove(args: argparse.Namespace) -> int:
    doc = load(args.wordlist)
    entries = doc.get(args.category, [])
    if not (0 <= args.index < len(entries)):
        print(f"error: index {args.index} out of range for {args.category} (0-{len(entries) - 1}).")
        return 1
    entries.pop(args.index)
    save(args.wordlist, doc)
    print(f"Removed entry [{args.index}] from {args.category}.")
    return 0


def cmd_severity(args: argparse.Namespace) -> int:
    doc = load(args.wordlist)
    entries = doc.get(args.category, [])
    if not (0 <= args.index < len(entries)):
        print(f"error: index {args.index} out of range for {args.category} (0-{len(entries) - 1}).")
        return 1
    entries[args.index]["severity"] = args.severity
    save(args.wordlist, doc)
    print(f"Set [{args.index}] in {args.category} to severity={args.severity}.")
    return 0


def cmd_reveal(args: argparse.Namespace) -> int:
    doc = load(args.wordlist)
    entries = doc.get(args.category, [])
    if not (0 <= args.index < len(entries)):
        print(f"error: index {args.index} out of range for {args.category} (0-{len(entries) - 1}).")
        return 1
    entry = entries[args.index]
    word = decode(entry["word_b64"]) if "word_b64" in entry else entry.get("word", "")
    print(word)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--wordlist", type=Path, default=DEFAULT_WORDLIST_PATH)
    sub = parser.add_subparsers(dest="command", required=True)

    p_add = sub.add_parser("add", help="Add a word/phrase via hidden prompt.")
    p_add.add_argument("--category", choices=CATEGORIES, required=True)
    p_add.add_argument("--severity", choices=SEVERITIES, default="moderate")
    p_add.set_defaults(func=cmd_add)

    p_list = sub.add_parser("list", help="List entries redacted (first letter only).")
    p_list.set_defaults(func=cmd_list)

    p_remove = sub.add_parser("remove", help="Remove an entry by index (see `list`).")
    p_remove.add_argument("--category", choices=CATEGORIES, required=True)
    p_remove.add_argument("--index", type=int, required=True)
    p_remove.set_defaults(func=cmd_remove)

    p_severity = sub.add_parser("severity", help="Change an entry's severity by index.")
    p_severity.add_argument("--category", choices=CATEGORIES, required=True)
    p_severity.add_argument("--index", type=int, required=True)
    p_severity.add_argument("--severity", choices=SEVERITIES, required=True)
    p_severity.set_defaults(func=cmd_severity)

    p_reveal = sub.add_parser("reveal", help="Print one entry's actual word (only command that does).")
    p_reveal.add_argument("--category", choices=CATEGORIES, required=True)
    p_reveal.add_argument("--index", type=int, required=True)
    p_reveal.set_defaults(func=cmd_reveal)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())

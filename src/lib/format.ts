// m:ss under an hour (movies' cue timestamps are usually well under that);
// h:mm:ss once the position/duration/cue rolls past 60 minutes, so a 2-hour
// movie reads as "2:05:33" rather than a confusing "125:33".
export function fmtTime(seconds: number | null | undefined): string {
  if (seconds == null) return "--:--";
  const total = Math.max(0, Math.round(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  }
  return `${m}:${s.toString().padStart(2, "0")}`;
}

// Turns a now-playing title into a filesystem-friendly default filename for
// "Record a new filter file…" (e.g. "The Princess Bride" -> "the-princess-
// bride.json") -- lowercased, punctuation collapsed to single hyphens, so
// the save dialog opens with a sensible name instead of always "filter.json"
// regardless of what's on screen. Mirrors the spirit of the Rust side's
// `metadata::cache_key` (same normalize-then-collapse-punctuation shape),
// just hyphenated for readability in a filename rather than underscored for
// a cache key, and kept separate since one lands in a Tauri command's
// filesystem path and the other in the poster cache dir.
export function slugifyTitle(title: string): string {
  const slug = title
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "filter";
}

// Pairs with fmtTime -- parses what a cue table's inputs display back into
// seconds, or null for anything unrecognized so the caller can reject the
// edit without touching the backend. Accepts both the m:ss fmtTime shows
// under an hour and the h:mm:ss it shows at/past an hour, so typing over a
// displayed value round-trips either way.
export function parseTime(text: string): number | null {
  const m = /^(\d+):([0-5]?\d)(?::([0-5]?\d))?$/.exec(text.trim());
  if (!m) return null;
  if (m[3] != null) {
    return Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3]);
  }
  return Number(m[1]) * 60 + Number(m[2]);
}

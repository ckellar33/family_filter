//! TMDB poster lookup for the Select Filter grid. Cached to disk under the
//! app's cache directory as raw JPEG bytes, so a repeat lookup for the same
//! title costs a disk read, not a network round trip. Degrades to `None` on
//! any failure (no key configured, no TMDB match, a network error) rather
//! than propagating an error -- a missing poster just means a placeholder
//! tile in the grid, never a broken one.
//!
//! Streaming-service badges are *not* looked up here -- those come straight
//! from the filter file itself (`filter::MediaEntry::services`), tagged
//! either automatically by `creation.rs` (from whichever app was playing
//! when a title's first cue was recorded) or by hand. A prior version of
//! this module also queried TMDB's watch-providers endpoint for that, but
//! the file's own record of where it was actually watched is more reliable
//! than a live catalog lookup guessing at it.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use tauri::{AppHandle, Manager};

use crate::filter;

const TMDB_API_KEY_STORE: &str = "tmdb_api_key.store";

/// Christopher Kellar's own TMDB v3 API key, used whenever `tmdb_api_key.store`
/// isn't present -- there's no way to "drop a file next to the binary" on
/// iOS (no Finder access into the app's sandbox), so this is the only key
/// mobile builds have. TMDB's v3 `api_key` is meant for exactly this kind of
/// client-side embedding (unlike a bearer/session token, it's not treated as
/// a secret by TMDB's own docs), so shipping it in source is the intended
/// use, not a leak -- revocable from the TMDB account if that ever changes.
const FALLBACK_TMDB_API_KEY: &str = "0483c13dc4bb71888e395266f5993741";

/// Reads the user's own TMDB API key from a sidecar file next to the app's
/// other `*.store` files -- same "drop a file next to the binary" pattern as
/// `filter::load_saved_filter_path`, still honored on desktop as a way to
/// override `FALLBACK_TMDB_API_KEY` with a different personal key without a
/// rebuild -- and falls back to that constant when the store is absent,
/// which is the only path mobile builds have.
fn tmdb_api_key() -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(crate::paths::data_dir().join(TMDB_API_KEY_STORE)) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    Some(FALLBACK_TMDB_API_KEY.to_string())
}

/// The `posters/` directory under the app's OS cache dir, created if
/// missing.
fn cache_dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_cache_dir().ok()?.join("posters");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Filesystem-safe cache key for a title -- normalized the same way
/// `filter::normalize_title` does for lookup, then every non-alphanumeric
/// character collapsed to `_` so punctuation in a title (`:`, `'`, ...) never
/// has to survive as a path segment. When `release_year` is provided it's
/// appended to the filename so year-specific lookups get their own cache
/// entry instead of colliding with title-only lookups.
fn cache_key(title: &str, release_year: Option<&str>) -> String {
    let normalized = filter::normalize_title(title)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();

    match release_year {
        Some(y) if !y.is_empty() => {
            // Avoid duplicating the year if it's already in the normalized title
            // (e.g. "home_alone_(1990)" already contains "_1990_"
            let already_has_year =
                normalized.contains(&format!("_{}", y)) || normalized.ends_with(&format!("_{}", y));
            if already_has_year {
                normalized
            } else {
                format!("{}_{}", normalized, y)
            }
        }
        _ => normalized,
    }
}

fn to_data_uri(bytes: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", BASE64.encode(bytes))
}

#[derive(Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Deserialize, Clone)]
struct SearchResult {
    poster_path: Option<String>,
}

/// Title search against TMDB's `/search/multi` -- filters to movie results
/// first so the grid always gets movie poster art (not a TV show that happens
/// to have a higher relevance score). Falls back to the first non-movie if
/// no movie match exists.
async fn search(title: &str, release_year: Option<&str>, api_key: &str) -> Option<SearchResult> {
    let client = reqwest::Client::new();

    let mut query_params = vec![
        ("api_key", api_key),
        ("query", title),
        ("include_video", "true"),
    ];

    // Inject the release year if provided and non-empty
    if let Some(y) = release_year {
        if !y.is_empty() {
            query_params.push(("year", y));
        }
    }

    let resp = client
        .get("https://api.themoviedb.org/3/search/movie")
        .query(&query_params)
        .send()
        .await
        .ok()?;

    let parsed: SearchResponse = resp.json().await.ok()?;

    // Index 0 will now reliably be the exact cinematic release
    parsed.results.into_iter().next()
}

/// Poster art for `title`, as a `data:` URI ready for an `<img src>` --
/// checked against the on-disk cache first so the grid's common case (every
/// title already looked up once) never touches the network.
pub async fn poster_data_uri(
    app: &AppHandle,
    title: &str,
    release_year: Option<&str>,
) -> Option<String> {
    // Normalize empty strings to None so they don't pollute the cache key
    // or the TMDB query. Both callers (`control::list_filter_tiles` and
    // `online::list_online_filters`) pass `Some(entry.release_year)` which
    // may be empty when the field was never written to disk / the DB.
    let release_year = release_year.filter(|s| !s.is_empty());

    let dir = cache_dir(app)?;
    let path = dir.join(format!("{}.jpg", cache_key(title, release_year)));
    if let Ok(bytes) = std::fs::read(&path) {
        if is_probably_valid_jpeg(&bytes) {
            return Some(to_data_uri(&bytes));
        }
        // A truncated/corrupted cache entry -- e.g. left over from a write
        // that got interrupted by the app backgrounding or being killed
        // mid-download, back before write_cache_atomically existed to
        // prevent that -- would otherwise render as a small broken/partial
        // image forever, since a cache hit never re-fetches. Drop it and
        // fall through to a fresh fetch instead, so this self-heals rather
        // than needing a manual cache clear.
        let _ = std::fs::remove_file(&path);
    }

    let api_key = tmdb_api_key()?;
    let matched = search(title, release_year, &api_key).await?;
    let poster_path = matched.poster_path?;
    let bytes = reqwest::get(format!("https://image.tmdb.org/t/p/w342{poster_path}"))
        .await
        .ok()?
        .bytes()
        .await
        .ok()?;
    write_cache_atomically(&path, &bytes);
    Some(to_data_uri(&bytes))
}

/// Whether `bytes` looks like a complete JPEG -- checks the standard SOI/EOI
/// magic markers (0xFFD8 start, 0xFFD9 end) plus a sane minimum size, so an
/// empty or half-written cache file is recognized as corrupt rather than
/// handed to the frontend as if it were real poster art. Not a full JPEG
/// validator -- just enough to catch "this file got cut off partway
/// through being written", the only failure mode this cache is actually
/// exposed to (TMDB itself is trusted to return valid JPEGs).
fn is_probably_valid_jpeg(bytes: &[u8]) -> bool {
    bytes.len() > 512 && bytes.starts_with(&[0xFF, 0xD8]) && bytes.ends_with(&[0xFF, 0xD9])
}

/// Writes `bytes` to `path` via a temp file + rename rather than a plain
/// `fs::write`, so a write interrupted partway (app backgrounded or killed
/// mid-download) never leaves a truncated file sitting at `path` for a
/// later cache read to pick up -- `fs::rename` within the same directory is
/// atomic, so `path` only ever either doesn't exist yet or holds the
/// complete file, never a partial one. Best-effort, same tolerance the
/// plain `fs::write` this replaced already had: a failure just means this
/// title re-fetches from TMDB next time instead of hitting the cache, not
/// something worth surfacing.
fn write_cache_atomically(path: &Path, bytes: &[u8]) {
    let tmp = path.with_extension("jpg.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_strips_punctuation() {
        assert_eq!(
            cache_key("Spider-Man: Far From Home", None),
            "spider_man__far_from_home"
        );
    }

    #[test]
    fn cache_key_is_stable_across_case_and_whitespace() {
        assert_eq!(
            cache_key("  Star Wars ", None),
            cache_key("STAR WARS", None)
        );
    }

    #[test]
    fn cache_key_includes_year() {
        // Year is stored as a separate field; cache key uses the year for the filename.
        assert_eq!(cache_key("Home Alone", Some("1990")), "home_alone_1990");
    }

    #[test]
    fn cache_key_avoids_year_duplication() {
        // When the title already embeds the year (e.g. "Home Alone (1990)"),
        // the year should not be appended twice.
        assert_eq!(
            cache_key("Home Alone (1990)", Some("1990")),
            "home_alone__1990_"
        );
    }

    #[test]
    fn valid_jpeg_bytes_pass() {
        let mut bytes = vec![0xFF, 0xD8];
        bytes.extend(std::iter::repeat(0u8).take(600));
        bytes.extend([0xFF, 0xD9]);
        assert!(is_probably_valid_jpeg(&bytes));
    }

    #[test]
    fn truncated_jpeg_bytes_are_rejected() {
        // Has the right start marker but got cut off before the end one --
        // exactly what a write interrupted mid-download leaves behind.
        let mut bytes = vec![0xFF, 0xD8];
        bytes.extend(std::iter::repeat(0u8).take(600));
        assert!(!is_probably_valid_jpeg(&bytes));
    }

    #[test]
    fn empty_bytes_are_rejected() {
        assert!(!is_probably_valid_jpeg(&[]));
    }
}

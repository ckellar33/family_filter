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

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::Deserialize;
use tauri::{AppHandle, Manager};

use crate::filter;

const TMDB_API_KEY_STORE: &str = "tmdb_api_key.store";

/// Reads the user's own TMDB API key from a sidecar file next to the app's
/// other `*.store` files -- same "drop a file next to the binary" pattern as
/// `filter::load_saved_filter_path`, chosen over a UI field since getting a
/// free TMDB key is a one-time personal setup step, not something meant to
/// be re-entered per install.
fn tmdb_api_key() -> Option<String> {
    let text = std::fs::read_to_string(TMDB_API_KEY_STORE).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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
/// character collapsed to `_` so punctuation in a title (`:`, `'`, ...)
/// never has to survive as a path segment.
fn cache_key(title: &str) -> String {
    filter::normalize_title(title).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

fn to_data_uri(bytes: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", BASE64.encode(bytes))
}

#[derive(Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Deserialize)]
struct SearchResult {
    poster_path: Option<String>,
}

/// Title search against TMDB's `/search/movie` -- just the first result, on
/// the same "close enough" assumption `FilterList::find_entry`'s exact-title
/// matching already relies on elsewhere in this app.
async fn search(title: &str, api_key: &str) -> Option<SearchResult> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.themoviedb.org/3/search/movie")
        .query(&[("api_key", api_key), ("query", title)])
        .send()
        .await
        .ok()?;
    let parsed: SearchResponse = resp.json().await.ok()?;
    parsed.results.into_iter().next()
}

/// Poster art for `title`, as a `data:` URI ready for an `<img src>` --
/// checked against the on-disk cache first so the grid's common case (every
/// title already looked up once) never touches the network.
pub async fn poster_data_uri(app: &AppHandle, title: &str) -> Option<String> {
    let dir = cache_dir(app)?;
    let path = dir.join(format!("{}.jpg", cache_key(title)));
    if let Ok(bytes) = std::fs::read(&path) {
        return Some(to_data_uri(&bytes));
    }

    let api_key = tmdb_api_key()?;
    let matched = search(title, &api_key).await?;
    let poster_path = matched.poster_path?;
    let bytes = reqwest::get(format!("https://image.tmdb.org/t/p/w342{poster_path}")).await.ok()?.bytes().await.ok()?;
    let _ = std::fs::write(&path, &bytes);
    Some(to_data_uri(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_strips_punctuation() {
        assert_eq!(cache_key("Spider-Man: Far From Home"), "spider_man__far_from_home");
    }

    #[test]
    fn cache_key_is_stable_across_case_and_whitespace() {
        assert_eq!(cache_key("  Star Wars "), cache_key("STAR WARS"));
    }
}

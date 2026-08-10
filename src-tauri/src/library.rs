//! Tracks every filter file the app has ever loaded or recorded, so the
//! Select Filter poster grid has something to enumerate -- `filter.rs` only
//! ever remembers the single *most recently active* path
//! (`filter_path.store`), which is enough to auto-reload on launch but not
//! enough to browse "everything I have". This is the same
//! read-a-sidecar-file/write-a-sidecar-file shape as that module and
//! `libs/appletv/src/storage.rs`'s `pairing.store`, just storing a list of
//! paths (JSON, so re-ordering/appending is trivial) instead of one path or
//! one set of credentials.
//!
//! Deliberately just a set of paths, not the parsed `FilterList`s themselves
//! -- callers (`control::list_filter_tiles`) re-load each file fresh, so a
//! file edited or deleted outside the app is reflected (or silently
//! skipped, on the delete side) the next time the grid is built, rather than
//! this module's own view of it ever going stale.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const FILTER_LIBRARY_STORE: &str = "filter_library.store";

/// Adds `path` to the library if it isn't already present (paths are
/// compared as given -- same-file-different-spelling duplicates, e.g. via a
/// symlink, aren't detected, but that's no worse than `filter_path.store`'s
/// own handling), then rewrites the store. Best-effort by convention at the
/// call sites (mirrors `filter::save_filter_path`): a failure to persist
/// just means this path won't show up in the grid until it's loaded again,
/// not that the load/record that triggered this should itself fail.
pub fn register_filter_path(path: &Path) -> Result<()> {
    let mut paths = list_library_paths();
    if !paths.iter().any(|p| p == path) {
        paths.push(path.to_path_buf());
        save(&paths)?;
    }
    Ok(())
}

/// Every path the library currently knows about, in the order they were
/// first registered. Returns an empty list rather than an error when the
/// store is missing or unparseable -- same "nothing to offer yet" tolerance
/// `filter::load_saved_filter_path` has, since there's always a sensible
/// fallback (an empty grid) rather than a hard failure.
pub fn list_library_paths() -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(FILTER_LIBRARY_STORE) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn save(paths: &[PathBuf]) -> Result<()> {
    let strings: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let json = serde_json::to_string_pretty(&strings).context("failed to serialize filter library")?;
    fs::write(FILTER_LIBRARY_STORE, json).context("failed to write filter_library.store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The store is a relative-path file in the process's current directory,
    // same as `filter_path.store` -- these tests share that one file, so
    // they can't run concurrently with each other (a `cargo test` default)
    // without stepping on each other's state. A single process-wide lock
    // serializes them; each test still cleans up after itself.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset() {
        let _ = fs::remove_file(FILTER_LIBRARY_STORE);
    }

    #[test]
    fn register_adds_new_paths_and_lists_them_back() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        register_filter_path(Path::new("a.json")).unwrap();
        register_filter_path(Path::new("b.json")).unwrap();
        assert_eq!(list_library_paths(), vec![PathBuf::from("a.json"), PathBuf::from("b.json")]);
        reset();
    }

    #[test]
    fn register_is_idempotent_for_the_same_path() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        register_filter_path(Path::new("a.json")).unwrap();
        register_filter_path(Path::new("a.json")).unwrap();
        assert_eq!(list_library_paths(), vec![PathBuf::from("a.json")]);
        reset();
    }

    #[test]
    fn list_is_empty_when_nothing_registered_yet() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset();
        assert!(list_library_paths().is_empty());
    }
}

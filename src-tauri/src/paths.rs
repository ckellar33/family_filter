//! Shared app-data directory for every sidecar `*.store` file this crate
//! writes directly (`filter.rs`, `library.rs`, `metadata.rs`'s TMDB key --
//! *not* the poster cache, which is deliberately kept in the OS cache dir
//! instead, see `metadata::cache_dir`). Mirrors `appletv::storage`'s own
//! `set_base_dir`/`base_dir` pair -- both get pointed at the same resolved
//! directory from `lib.rs::run`'s `.setup()`, since credentials and filter
//! metadata have the same durability need (must survive reinstall-adjacent
//! events like an iOS offload, unlike a purely cache-tier download the
//! system's free to evict).
//!
//! Before this existed, these stores were plain CWD-relative paths -- fine
//! by accident on desktop (`tauri dev`/most bundling launches happen to
//! leave CWD writable), but wrong on iOS: an app's process has no
//! "current directory" that's part of its writable sandbox, so a bare
//! relative path resolves nowhere reliable, and can land against the
//! read-only app bundle itself on a real device.

use std::path::PathBuf;
use std::sync::OnceLock;

static BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets the directory `data_dir()` returns from here on. Intended to be
/// called exactly once, early in `lib.rs::run`'s `.setup()`, with the
/// directory already created -- repeat calls are silently ignored
/// (`OnceLock` semantics), and every store write in this crate assumes the
/// directory already exists rather than creating it itself.
pub fn set_base_dir(dir: PathBuf) {
    let _ = BASE_DIR.set(dir);
}

/// Falls back to `.` if called before `set_base_dir` (shouldn't happen in
/// the running app, but keeps `cargo test` -- which never runs `run()` --
/// working the same as it did when these stores were plain CWD-relative
/// paths).
pub fn data_dir() -> PathBuf {
    BASE_DIR.get().cloned().unwrap_or_else(|| PathBuf::from("."))
}

/// Converts a path string as handed to a `#[tauri::command]` by
/// `@tauri-apps/plugin-dialog`'s frontend `open()`/`save()` into a real
/// filesystem `PathBuf`. Desktop's dialog returns a bare path (e.g.
/// `/Users/name/file.json`) -- `PathBuf::from` alone is correct there, and
/// stays correct here (`Url::from_str` fails to parse a bare path at all,
/// since URLs require a scheme, so this falls through to the plain-path
/// branch unchanged).
///
/// iOS's dialog implementation always returns a `file://` URL instead (see
/// `tauri_plugin_fs::FilePath`, which the dialog plugin returns under the
/// hood -- an untagged `Url(url::Url) | Path(PathBuf)` enum, always the
/// `Url` variant on mobile). Passing that string straight to `PathBuf::from`
/// produces a path that's never going to exist (`fs::read`/`fs::copy` fail
/// with "No such file or directory", which is what surfaced as "file not
/// found" in the UI) -- `Url::to_file_path` is what actually turns it back
/// into a real path, percent-decoding included (a `%20` in a picked
/// filename would otherwise end up literally in the path).
pub fn from_picker(raw: &str) -> PathBuf {
    match raw.parse::<url::Url>().ok().and_then(|u| u.to_file_path().ok()) {
        Some(path) => path,
        None => PathBuf::from(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_style_bare_path_passes_through_unchanged() {
        assert_eq!(from_picker("/Users/name/file.json"), PathBuf::from("/Users/name/file.json"));
    }

    #[test]
    fn ios_style_file_url_is_decoded_to_a_real_path() {
        assert_eq!(
            from_picker("file:///private/var/mobile/Containers/Data/Application/ABC/tmp/file.json"),
            PathBuf::from("/private/var/mobile/Containers/Data/Application/ABC/tmp/file.json")
        );
    }

    #[test]
    fn percent_encoded_characters_in_a_file_url_are_decoded() {
        assert_eq!(from_picker("file:///private/var/mobile/the%20princess%20bride.json"), PathBuf::from("/private/var/mobile/the princess bride.json"));
    }
}

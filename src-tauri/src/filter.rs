//! Auto-filter mode: loads a cue file (time ranges per media title *and
//! streaming service* -- see `MediaEntry::service` -- each cue tagged with
//! an action -- mute or skip -- and a free-form content category like
//! "language" or "gore") and decides what to do about it as playback
//! position advances. Deliberately kept out of `libs/appletv` -- that crate
//! is the reusable, protocol-only Apple TV library (its own repo/
//! submodule); a cue list is app-specific business logic, same reasoning
//! `control.rs`'s Tauri commands aren't in `libs` either.
//!
//! The actual mute/unmute/skip calls stay in `control.rs`, next to the live
//! session and Companion session that make them -- this module only decides
//! *what* should happen (`evaluate`), as a pure function of the loaded list,
//! the running state, and one playback snapshot, so the decision logic is
//! testable without a real device.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// Sidecar file remembering the last filter file the user picked, so it
/// reloads automatically on the next launch -- mirrors
/// `libs/appletv/src/storage.rs`'s `pairing.store`, but this one is app-
/// specific (just a path, not credentials) so it lives here instead.
const FILTER_PATH_STORE: &str = "filter_path.store";

/// Sidecar file remembering the master auto-filter on/off toggle across
/// launches -- same idea as `FILTER_PATH_STORE`, just a `"true"`/`"false"`
/// (or anything else, tolerated the same way) instead of a path. Read back
/// by `control::check_saved_filter_file` on startup so the mode comes back
/// exactly how it was left, rather than always defaulting off.
const FILTER_ENABLED_STORE: &str = "filter_enabled.store";

/// Minimum time to wait before re-dispatching a seek for the *same* cue --
/// without this, a poll tick landing inside `[start, end)` again before the
/// device has caught up to the previous seek (or actually applied it --
/// worth retrying if it silently dropped) would re-dispatch every poll
/// interval until it does. Every retry targets the exact same absolute
/// position (`cue.end` -- see `FilterCommand::Seek`), so unlike a relative
/// skip there's no "remaining amount" to recompute between attempts, and
/// no risk of compounding overshoot from retrying.
const SKIP_RETRY_COOLDOWN: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CueAction {
    Mute,
    Skip,
}

/// Also `Serialize` (not just `Deserialize`, needed for parsing the filter
/// file itself) so a `Cue` can be handed straight to the frontend as-is --
/// see `PlaybackStatus::filter_cues` -- rather than needing a separate wire
/// type just to expose the schedule for display.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub action: CueAction,
    /// Free-form content-type label (e.g. "language", "nudity", "gore") --
    /// not an enum, so a filter file can introduce new categories without a
    /// code change; the UI just renders one toggle per distinct value it
    /// finds (see `FilterList::categories`).
    pub category: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MediaEntry {
    pub title: String,
    /// Which streaming service this specific cue list applies to (e.g.
    /// "Netflix", "Disney+") -- required going forward, since different
    /// services commonly cut/pace the same title differently (different
    /// intro length, ad breaks, a scene trimmed on one platform but not
    /// another), so their cue timings can't be shared. A title can have
    /// more than one `MediaEntry` -- one per service -- each with its own
    /// independent cue list.
    ///
    /// The empty string is a deliberate third state, "generic/unspecified"
    /// -- both for filter files written before this field existed
    /// (`#[serde(default)]` parses those as `""`) and for a title recorded
    /// while playing in an app this build's `control::app_display_name`
    /// table doesn't recognize. A generic entry is used as a fallback by
    /// `FilterList::find_entry_for_playback` when no exact-service entry
    /// matches, rather than being treated as "no match".
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct FilterList {
    pub media: Vec<MediaEntry>,
}

/// Identifies one cue for the individual-cue on/off toggle: the entry's
/// normalized title and service, plus its index within that entry's
/// (sorted-by-start) `cues` -- stable for as long as one `FilterList` stays
/// loaded, since cues are sorted once at parse time and never reordered
/// after. Scoped by title+service (not just index) so toggling cue #2 off
/// for one movie-on-one-service can't accidentally affect cue #2 of an
/// unrelated entry -- including a *different service's* entry for the same
/// title, which has its own independent cue list.
pub type CueKey = (String, String, usize);

/// `pub(crate)` rather than private: `control.rs` needs it too, to build the
/// same key when the frontend reports which cue (by title + index) got
/// toggled.
pub(crate) fn normalize_title(t: &str) -> String {
    t.trim().to_lowercase()
}

/// Same normalization as `normalize_title`, kept as its own function (rather
/// than reusing that one under a generic name) so call sites read clearly
/// about which field they're normalizing.
pub(crate) fn normalize_service(s: &str) -> String {
    s.trim().to_lowercase()
}

impl MediaEntry {
    /// Sorts `cues` by start and re-validates every invariant load-time
    /// parsing already enforces (finite/non-negative start, end > start,
    /// non-empty category, no overlap within this entry) -- shared by
    /// `FilterList::parse_and_validate` and the creation-mode mutation
    /// methods below (`FilterList::add_cue`/`update_cue`), so both paths can
    /// never drift apart on what counts as a valid cue list.
    fn sort_and_validate(&mut self) -> Result<()> {
        self.cues.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let mut prev_end: Option<f64> = None;
        for cue in &self.cues {
            if !(cue.start.is_finite() && cue.end.is_finite()) {
                bail!("{:?} ({:?}) has a non-finite cue start/end", self.title, self.service);
            }
            if cue.start < 0.0 || cue.end <= cue.start {
                bail!("{:?} ({:?}) has a cue with start >= end ({} >= {})", self.title, self.service, cue.start, cue.end);
            }
            if cue.category.trim().is_empty() {
                bail!("{:?} ({:?}) has a cue with an empty category", self.title, self.service);
            }
            if let Some(prev_end) = prev_end {
                if cue.start < prev_end {
                    bail!("{:?} ({:?}) has overlapping cues around {}", self.title, self.service, cue.start);
                }
            }
            prev_end = Some(cue.end);
        }
        Ok(())
    }

    /// This entry's contribution to a `CueKey` -- the normalized title and
    /// service, shared by every cue in it.
    fn key_prefix(&self) -> (String, String) {
        (normalize_title(&self.title), normalize_service(&self.service))
    }

    /// Whether cue `idx` is currently eligible to fire at all -- neither its
    /// category nor the cue itself has been individually disabled.
    fn cue_enabled(&self, idx: usize, cue: &Cue, disabled_categories: &HashSet<String>, disabled_cues: &HashSet<CueKey>) -> bool {
        let (title, service) = self.key_prefix();
        !disabled_categories.contains(&cue.category) && !disabled_cues.contains(&(title, service, idx))
    }

    /// The cue containing `pos` (`start <= pos < end`), if any, that's also
    /// currently enabled (see `cue_enabled`).
    fn cue_at(&self, pos: f64, disabled_categories: &HashSet<String>, disabled_cues: &HashSet<CueKey>) -> Option<(usize, &Cue)> {
        self.cues
            .iter()
            .enumerate()
            .find(|(idx, c)| c.start <= pos && pos < c.end && self.cue_enabled(*idx, c, disabled_categories, disabled_cues))
    }

}

impl FilterList {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
        Self::parse_and_validate(&text)
    }

    fn parse_and_validate(json: &str) -> Result<Self> {
        let mut list: FilterList = serde_json::from_str(json).context("invalid filter file JSON")?;

        let mut seen = HashSet::new();
        for entry in &mut list.media {
            if entry.title.trim().is_empty() {
                bail!("a media entry has an empty title");
            }
            if !seen.insert((normalize_title(&entry.title), normalize_service(&entry.service))) {
                bail!("duplicate title+service {:?}/{:?} in filter file", entry.title, entry.service);
            }
            entry.sort_and_validate()?;
        }

        Ok(list)
    }

    /// Exact (title, service) lookup -- `service` is normalized the same
    /// way stored entries are, so callers can pass a raw display name
    /// (e.g. straight from `control::app_display_name`) without
    /// normalizing it themselves first. Pass `""` to look up the generic
    /// (service-unspecified) entry specifically.
    pub fn find_entry(&self, title: &str, service: &str) -> Option<&MediaEntry> {
        let t = normalize_title(title);
        let s = normalize_service(service);
        self.media.iter().find(|e| normalize_title(&e.title) == t && normalize_service(&e.service) == s)
    }

    /// The entry to actually apply for `title` currently playing on
    /// `service` (`None` if the service couldn't be determined -- e.g. an
    /// unrecognized app): an exact match if the file has one, else the
    /// generic (service-unspecified) entry for the title if the file has
    /// *that* instead. Falling back rather than requiring an exact match
    /// keeps filter files written before per-service entries existed (or
    /// deliberately service-agnostic ones) working -- but an exact match
    /// always wins when both exist, since it's the more accurate timing for
    /// what's actually playing.
    pub fn find_entry_for_playback(&self, title: &str, service: Option<&str>) -> Option<&MediaEntry> {
        if let Some(s) = service {
            if let Some(entry) = self.find_entry(title, s) {
                return Some(entry);
            }
        }
        self.find_entry(title, "")
    }

    /// Every service variant registered for `title`, in file order --
    /// what the Select Filter service picker (after tapping a title's
    /// poster) and the "a filter is available" auto-detect check both
    /// enumerate. Includes the generic (`service == ""`) entry, if any.
    pub fn entries_for_title(&self, title: &str) -> Vec<&MediaEntry> {
        let t = normalize_title(title);
        self.media.iter().filter(|e| normalize_title(&e.title) == t).collect()
    }

    /// Distinct categories across every entry, in first-seen order -- what
    /// the frontend renders one on/off toggle per.
    pub fn categories(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for entry in &self.media {
            for cue in &entry.cues {
                if seen.insert(cue.category.clone()) {
                    out.push(cue.category.clone());
                }
            }
        }
        out
    }

    /// Same lookup as `find_entry`, but mutable -- private since only the
    /// creation-mode mutation methods below need write access; everything
    /// else (including the frontend, via Tauri commands) goes through those
    /// instead of poking `media` directly.
    fn find_entry_mut(&mut self, title: &str, service: &str) -> Option<&mut MediaEntry> {
        let t = normalize_title(title);
        let s = normalize_service(service);
        self.media.iter_mut().find(|e| normalize_title(&e.title) == t && normalize_service(&e.service) == s)
    }

    /// Same lookup as `find_entry_mut`, but creates a new (empty-cues) entry
    /// under `(title, service)` first if none exists yet -- lets the
    /// creation-mode mutation methods below record a title's very first cue
    /// without having to special-case "this is a new title/service pair"
    /// themselves.
    fn entry_mut(&mut self, title: &str, service: &str) -> &mut MediaEntry {
        if self.find_entry_mut(title, service).is_none() {
            self.media.push(MediaEntry { title: title.to_string(), service: service.to_string(), cues: Vec::new() });
        }
        self.find_entry_mut(title, service).expect("just inserted if it was absent")
    }

    /// Corrects a mis-detected (or never-detected) service after the fact --
    /// e.g. a title recorded while playing in an app this build's
    /// `control::app_display_name` table doesn't recognize lands as the
    /// generic `""` entry; this renames it once the right name is known.
    /// Rejects the rename if `(title, new_service)` already has its own
    /// entry -- renaming into an existing entry would silently merge two
    /// independent cue lists, which is never what's wanted here.
    pub fn set_entry_service(&mut self, title: &str, old_service: &str, new_service: &str) -> Result<()> {
        if self.find_entry(title, new_service).is_some() {
            bail!("{:?} already has an entry for service {:?}", title, new_service);
        }
        let entry = self
            .find_entry_mut(title, old_service)
            .with_context(|| format!("no entry for {:?} on service {:?}", title, old_service))?;
        entry.service = new_service.to_string();
        Ok(())
    }

    /// Creates an empty (no-cues-yet) entry under `(title, service)` if one
    /// doesn't already exist -- the public counterpart to `entry_mut`, for
    /// callers (creation mode's draft-start commands) that want a title's
    /// entry to exist and be persisted the moment recording starts, rather
    /// than only appearing once its first cue is marked via `add_cue`.
    /// A no-op (not an error) if the entry already exists, so it's safe to
    /// call unconditionally whenever a draft opens with something already
    /// playing.
    pub fn ensure_entry(&mut self, title: &str, service: &str) -> Result<()> {
        if title.trim().is_empty() {
            bail!("a media entry has an empty title");
        }
        self.entry_mut(title, service);
        Ok(())
    }

    /// Adds one cue under `(title, service)` (creating the entry if this is
    /// its first cue), then re-sorts and re-validates that entry -- same
    /// invariants `parse_and_validate` enforces at load time, via
    /// `sort_and_validate`. On failure (e.g. the new cue overlaps an
    /// existing one), the entry's cues are restored to their pre-call state
    /// so a rejected mark never leaves partial data behind. Returns the new
    /// cue's index after sorting -- looked up by `start` alone, which
    /// validation guarantees is unique within an entry (two cues sharing a
    /// start would always overlap, since both include that point and
    /// neither has zero length).
    pub fn add_cue(&mut self, title: &str, service: &str, cue: Cue) -> Result<usize> {
        let start = cue.start;
        let entry = self.entry_mut(title, service);
        let backup = entry.cues.clone();
        entry.cues.push(cue);
        if let Err(e) = entry.sort_and_validate() {
            entry.cues = backup;
            return Err(e);
        }
        Ok(entry.cues.iter().position(|c| c.start == start).expect("just inserted"))
    }

    /// Changes cue `index`'s start/end in place, then re-sorts/re-validates
    /// (its position in the list may shift). On failure, the entry's cues
    /// are restored to their pre-call state, same as `add_cue`.
    pub fn update_cue(&mut self, title: &str, service: &str, index: usize, start: f64, end: f64) -> Result<()> {
        let entry = self
            .find_entry_mut(title, service)
            .with_context(|| format!("no entry for {:?} on service {:?}", title, service))?;
        if index >= entry.cues.len() {
            bail!("cue index {} out of range for {:?} ({} cues)", index, title, entry.cues.len());
        }
        let backup = entry.cues.clone();
        entry.cues[index].start = start;
        entry.cues[index].end = end;
        if let Err(e) = entry.sort_and_validate() {
            entry.cues = backup;
            return Err(e);
        }
        Ok(())
    }

    /// Removes cue `index` outright. No re-validation needed -- removing a
    /// cue can't create an overlap or invalid range among what's left.
    pub fn delete_cue(&mut self, title: &str, service: &str, index: usize) -> Result<()> {
        let entry = self
            .find_entry_mut(title, service)
            .with_context(|| format!("no entry for {:?} on service {:?}", title, service))?;
        if index >= entry.cues.len() {
            bail!("cue index {} out of range for {:?} ({} cues)", index, title, entry.cues.len());
        }
        entry.cues.remove(index);
        Ok(())
    }

    /// Serializes and writes this list to `path`, pretty-printed for a
    /// human-readable filter file (matches the hand-authored style of
    /// `test_filter.json`) -- used to autosave a creation-mode draft after
    /// every mutation.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("failed to serialize filter list")?;
        fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
    }
}

pub fn load_saved_filter_path() -> Option<PathBuf> {
    let text = fs::read_to_string(FILTER_PATH_STORE).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

pub fn save_filter_path(path: &Path) -> Result<()> {
    fs::write(FILTER_PATH_STORE, path.to_string_lossy().as_bytes()).context("failed to write filter_path.store")
}

/// `false` (never on by default) if the store is missing or unparseable --
/// same "nothing to offer yet" tolerance `load_saved_filter_path` has, and
/// the safer of the two defaults regardless: a corrupt/absent store should
/// never be the reason auto-filter mode silently turns itself on.
pub fn load_saved_filter_enabled() -> bool {
    fs::read_to_string(FILTER_ENABLED_STORE).ok().map(|s| s.trim() == "true").unwrap_or(false)
}

pub fn save_filter_enabled(enabled: bool) -> Result<()> {
    fs::write(FILTER_ENABLED_STORE, if enabled { "true" } else { "false" }).context("failed to write filter_enabled.store")
}

/// Per-session bookkeeping the evaluation engine carries across poll ticks --
/// which title+service it last saw, which cue (if any) currently holds an
/// auto-applied mute, when a skip was last dispatched for which cue (to
/// throttle re-dispatch while the device catches up), and the position last
/// seen (to detect a skip cue's window being jumped clean over).
#[derive(Debug, Default)]
pub struct FilterRuntime {
    active_title: Option<String>,
    /// Tracked alongside `active_title` -- switching service for the same
    /// title (e.g. resuming the same movie in a different app) points at a
    /// *different* entry with its own independent cue list, so it needs the
    /// same "this invalidates everything below" treatment as a title change
    /// does, not just a title-alone comparison.
    active_service: Option<String>,
    muted_cue: Option<usize>,
    last_skip: Option<(usize, Instant)>,
    /// The position observed on the *previous* `evaluate` call for the
    /// current `active_title`/`active_service`. `None` right after a
    /// change, which deliberately disables the catch-up pass for that
    /// entry's very first poll: with no prior position to compare against,
    /// there's no way to tell "we were tracking this and missed a cue"
    /// apart from "playback just started already past one" -- only the
    /// former should trigger a catch-up skip.
    last_position: Option<f64>,
}

impl FilterRuntime {
    /// Whether a cue is currently holding an auto-applied mute -- callers
    /// that force a mode/category off outside the normal per-poll
    /// `evaluate` flow use this to decide whether they owe the device an
    /// explicit `unmute()` before discarding this state.
    pub fn is_muted(&self) -> bool {
        self.muted_cue.is_some()
    }

    /// Drops all tracked state (matched title/service, active mute, skip
    /// cooldown). Callers are responsible for issuing an `unmute()` first if
    /// `is_muted()` was true -- this alone doesn't touch the device.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterCommand {
    Mute,
    Unmute,
    /// Absolute seek target, in seconds -- always `cue.end` for whichever
    /// skip cue is in play. Dispatched as `LiveSession::seek` (MRP's
    /// `SendCommandMessage`/`SeekToPlaybackPosition`), *not* Companion's
    /// relative `_mcc` SkipBy: some apps (confirmed against Disney+) only
    /// ever honor a fixed, much shorter interval than a SkipBy actually
    /// requests, so a cue longer than that landed as several under-shot
    /// hops instead of clearing in one go -- an absolute "go to this
    /// position" command doesn't have that problem, since there's no
    /// "requested amount" for the device to substitute its own fixed one
    /// for.
    Seek(f64),
}

/// What `evaluate` decided for one poll tick: which commands (if any) the
/// caller should issue against the live/Companion sessions, plus display
/// info for the frontend.
#[derive(Debug, Clone, Default)]
pub struct FilterOutcome {
    /// The matched media entry's title, or `None` if nothing in the loaded
    /// list matches the current now-playing title+service (including
    /// "nothing is playing").
    pub filter_match: Option<String>,
    pub commands: Vec<FilterCommand>,
    pub filter_action: Option<&'static str>,
    pub filter_category: Option<String>,
}

/// Pure decision function: given the loaded list, the running state, which
/// categories and individual cues are currently disabled, and one playback
/// snapshot (title + service + position + whether it's actually advancing),
/// decides what (if anything) should happen. `service` is whichever app is
/// currently "now playing" -- see `control::app_display_name` -- or `None`
/// if that couldn't be determined, in which case only a generic
/// (service-unspecified) entry can match (see
/// `FilterList::find_entry_for_playback`). `is_playing` should be `false`
/// for anything other than actively playing (paused, stopped, seeking,
/// unknown) -- see the guard below for why, and `continuing_in_flight_skip`
/// just past it for the one narrow exception. No I/O -- the caller
/// (`control.rs`) is responsible for actually issuing the returned commands
/// against the live/Companion sessions, which is what makes this testable
/// without a real device.
pub fn evaluate(
    list: &FilterList,
    runtime: &mut FilterRuntime,
    disabled_categories: &HashSet<String>,
    disabled_cues: &HashSet<CueKey>,
    title: Option<&str>,
    service: Option<&str>,
    position: Option<f64>,
    is_playing: bool,
    now: Instant,
) -> FilterOutcome {
    let mut outcome = FilterOutcome::default();

    let key_changed = (runtime.active_title.as_deref().map(normalize_title), runtime.active_service.as_deref().map(normalize_service))
        != (title.map(normalize_title), service.map(normalize_service));
    if key_changed {
        // Never leave audio stuck muted across a title/service change --
        // whatever cue held the mute no longer applies once the entry we're
        // tracking changes, so release it immediately rather than waiting
        // for the next (now-irrelevant) cue lookup to notice.
        if runtime.muted_cue.take().is_some() {
            outcome.commands.push(FilterCommand::Unmute);
        }
        runtime.last_skip = None;
        runtime.last_position = None;
        runtime.active_title = title.map(str::to_string);
        runtime.active_service = service.map(str::to_string);
    }

    let Some(entry) = title.and_then(|t| list.find_entry_for_playback(t, service)) else {
        return outcome;
    };
    outcome.filter_match = Some(entry.title.clone());

    let Some(pos) = position else {
        return outcome;
    };

    let found = entry.cue_at(pos, disabled_categories, disabled_cues);

    // Paused (or stopped/seeking/unknown) -- don't apply, re-apply, or
    // release any cue action while playback isn't actually advancing. A cue
    // landing exactly where playback is paused shouldn't force a mute the
    // user won't hear anyway, and a seek must never fire while paused --
    // that would yank position out from under a deliberate pause instead of
    // waiting for it to resume. (This used to need a narrow exception here
    // for a skip already in flight, back when skipping was a *relative*
    // Companion `SkipBy` that some apps only ever honored as a fixed,
    // much-shorter-than-requested hop -- clearing a whole cue could take
    // several hops, each triggering the app's own auto-pause-after-seek.
    // `FilterCommand::Seek` is an absolute MRP position command instead, so
    // one dispatch is enough regardless of pause; nothing to continue.)
    // Leaves any mute already engaged untouched rather than releasing it:
    // there's nothing wrong with staying muted through a pause, and the
    // next `evaluate` while actually playing will reapply the correct state
    // for wherever position ends up anyway. Still records `last_position`
    // so the catch-up pass (below, once playing again) compares against an
    // accurate "last seen" position rather than one frozen from before the
    // pause.
    if !is_playing {
        runtime.last_position = Some(pos);
        return outcome;
    }

    // Mute/unmute: compare the cue (if any) that *should* be holding a mute
    // right now against the one that actually is, and transition between
    // them. Covers entering a mute range, leaving one (including because its
    // category or the cue itself just got disabled -- `cue_at` already
    // excludes both, so this sees exactly the same "no cue" case as the
    // range genuinely ending), and switching straight from one mute cue to
    // another.
    let desired_mute_idx = found.and_then(|(idx, cue)| (cue.action == CueAction::Mute).then_some(idx));
    if desired_mute_idx != runtime.muted_cue {
        if runtime.muted_cue.is_some() {
            outcome.commands.push(FilterCommand::Unmute);
        }
        if desired_mute_idx.is_some() {
            let (_, cue) = found.expect("desired_mute_idx only set when found is Some");
            outcome.commands.push(FilterCommand::Mute);
            outcome.filter_action = Some("auto-muted");
            outcome.filter_category = Some(cue.category.clone());
        }
        runtime.muted_cue = desired_mute_idx;
    }

    if let Some((idx, cue)) = found {
        if cue.action == CueAction::Skip {
            let should_dispatch = match runtime.last_skip {
                Some((last_idx, at)) if last_idx == idx => now.duration_since(at) >= SKIP_RETRY_COOLDOWN,
                _ => true,
            };
            if should_dispatch {
                outcome.commands.push(FilterCommand::Seek(cue.end));
                outcome.filter_action = Some("auto-skipped");
                outcome.filter_category = Some(cue.category.clone());
                runtime.last_skip = Some((idx, now));
            }
        }
    }

    // No catch-up pass for a skip cue whose entire window got jumped clean
    // over since the last poll (see `skipped_over_cues`) -- unlike the old
    // relative-skip design (a "best-effort forward nudge" was always safe,
    // since it could only ever move forward), an absolute `Seek` to
    // `cue.end` here would be *backward*: `skipped_over_cues` only reports a
    // cue once `pos` is already at or past its `end`, so there's nothing
    // left ahead to seek to, and seeking back into a cue whose content
    // either already played (missed between two poll ticks) or was
    // deliberately scrubbed past by the user would be actively wrong in
    // both cases. `runtime.last_position` is still tracked below regardless
    // -- other bookkeeping (e.g. a future entry's very first poll) still
    // depends on it.
    runtime.last_position = Some(pos);

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: f64, end: f64, action: CueAction, category: &str) -> Cue {
        Cue { start, end, action, category: category.to_string() }
    }

    fn entry(title: &str, service: &str, cues: Vec<Cue>) -> MediaEntry {
        MediaEntry { title: title.to_string(), service: service.to_string(), cues }
    }

    /// A single generic (service-unspecified) entry -- what most of the
    /// existing tests exercise, unrelated to the per-service matching
    /// behavior covered separately below.
    fn sample_list() -> FilterList {
        FilterList {
            media: vec![entry(
                "Some Movie",
                "",
                vec![cue(10.0, 20.0, CueAction::Mute, "language"), cue(30.0, 40.0, CueAction::Skip, "gore")],
            )],
        }
    }

    fn empty_disabled() -> HashSet<String> {
        HashSet::new()
    }

    fn empty_disabled_cues() -> HashSet<CueKey> {
        HashSet::new()
    }

    #[test]
    fn parses_valid_file() {
        let json = r#"{
            "media": [
                { "title": "Some Movie", "cues": [
                    { "start": 10.0, "end": 20.0, "action": "mute", "category": "language" },
                    { "start": 30.0, "end": 40.0, "action": "skip", "category": "gore" }
                ] }
            ]
        }"#;
        let list = FilterList::parse_and_validate(json).unwrap();
        assert_eq!(list.media.len(), 1);
        assert_eq!(list.media[0].cues.len(), 2);
        assert_eq!(list.media[0].service, ""); // omitted field parses as the generic entry
    }

    #[test]
    fn parses_a_service_field() {
        let json = r#"{
            "media": [
                { "title": "Some Movie", "service": "Netflix", "cues": [] }
            ]
        }"#;
        let list = FilterList::parse_and_validate(json).unwrap();
        assert_eq!(list.media[0].service, "Netflix");
    }

    #[test]
    fn rejects_overlapping_cues() {
        let json = r#"{
            "media": [
                { "title": "X", "cues": [
                    { "start": 10.0, "end": 20.0, "action": "mute", "category": "language" },
                    { "start": 15.0, "end": 25.0, "action": "skip", "category": "gore" }
                ] }
            ]
        }"#;
        assert!(FilterList::parse_and_validate(json).is_err());
    }

    #[test]
    fn rejects_start_after_end() {
        let json = r#"{
            "media": [
                { "title": "X", "cues": [
                    { "start": 20.0, "end": 20.0, "action": "mute", "category": "language" }
                ] }
            ]
        }"#;
        assert!(FilterList::parse_and_validate(json).is_err());
    }

    #[test]
    fn rejects_unknown_action() {
        let json = r#"{
            "media": [
                { "title": "X", "cues": [
                    { "start": 1.0, "end": 2.0, "action": "bleep", "category": "language" }
                ] }
            ]
        }"#;
        assert!(FilterList::parse_and_validate(json).is_err());
    }

    #[test]
    fn rejects_empty_category() {
        let json = r#"{
            "media": [
                { "title": "X", "cues": [
                    { "start": 1.0, "end": 2.0, "action": "mute", "category": "" }
                ] }
            ]
        }"#;
        assert!(FilterList::parse_and_validate(json).is_err());
    }

    #[test]
    fn rejects_duplicate_title_and_service() {
        let json = r#"{
            "media": [
                { "title": "Same", "service": "Netflix", "cues": [] },
                { "title": "  same ", "service": " NETFLIX ", "cues": [] }
            ]
        }"#;
        assert!(FilterList::parse_and_validate(json).is_err());
    }

    #[test]
    fn allows_same_title_on_different_services() {
        let json = r#"{
            "media": [
                { "title": "Same", "service": "Netflix", "cues": [] },
                { "title": "Same", "service": "Disney+", "cues": [] }
            ]
        }"#;
        let list = FilterList::parse_and_validate(json).unwrap();
        assert_eq!(list.media.len(), 2);
    }

    #[test]
    fn find_entry_is_case_insensitive_and_trims_title_and_service() {
        let list = FilterList { media: vec![entry("Some Movie", "Netflix", vec![])] };
        assert!(list.find_entry("  some movie ", " NETFLIX ").is_some());
        assert!(list.find_entry("SOME MOVIE", "netflix").is_some());
        assert!(list.find_entry("Some Other Movie", "Netflix").is_none());
        assert!(list.find_entry("Some Movie", "Disney+").is_none());
    }

    #[test]
    fn find_entry_for_playback_prefers_exact_service_match() {
        let list = FilterList {
            media: vec![
                entry("Some Movie", "", vec![cue(0.0, 1.0, CueAction::Mute, "language")]),
                entry("Some Movie", "Netflix", vec![cue(5.0, 6.0, CueAction::Mute, "language")]),
            ],
        };
        let found = list.find_entry_for_playback("Some Movie", Some("Netflix")).unwrap();
        assert_eq!(found.service, "Netflix");
        assert_eq!(found.cues[0].start, 5.0);
    }

    #[test]
    fn find_entry_for_playback_falls_back_to_generic_when_no_exact_match() {
        let list = sample_list(); // generic-only
        let found = list.find_entry_for_playback("Some Movie", Some("Netflix")).unwrap();
        assert_eq!(found.service, "");
    }

    #[test]
    fn find_entry_for_playback_falls_back_to_generic_when_service_unknown() {
        let list = sample_list();
        let found = list.find_entry_for_playback("Some Movie", None).unwrap();
        assert_eq!(found.service, "");
    }

    #[test]
    fn find_entry_for_playback_none_when_neither_exists() {
        let list = FilterList { media: vec![entry("Some Movie", "Netflix", vec![])] };
        assert!(list.find_entry_for_playback("Some Movie", Some("Disney+")).is_none());
    }

    #[test]
    fn entries_for_title_returns_every_service_variant() {
        let list = FilterList {
            media: vec![
                entry("Some Movie", "Netflix", vec![]),
                entry("Some Movie", "Disney+", vec![]),
                entry("A Different Movie", "Netflix", vec![]),
            ],
        };
        let variants = list.entries_for_title("Some Movie");
        assert_eq!(variants.len(), 2);
        assert!(variants.iter().any(|e| e.service == "Netflix"));
        assert!(variants.iter().any(|e| e.service == "Disney+"));
    }

    #[test]
    fn categories_dedupes_in_first_seen_order() {
        let list = FilterList {
            media: vec![
                entry("A", "", vec![cue(0.0, 1.0, CueAction::Mute, "language"), cue(2.0, 3.0, CueAction::Skip, "gore")]),
                entry("B", "", vec![cue(0.0, 1.0, CueAction::Mute, "language")]),
            ],
        };
        assert_eq!(list.categories(), vec!["language".to_string(), "gore".to_string()]);
    }

    #[test]
    fn no_title_produces_no_match_or_commands() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let outcome = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), None, None, None, true, Instant::now());
        assert!(outcome.filter_match.is_none());
        assert!(outcome.commands.is_empty());
    }

    #[test]
    fn entering_and_leaving_a_mute_range() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Before the cue: no commands.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(5.0), true, now);
        assert_eq!(o.filter_match.as_deref(), Some("Some Movie"));
        assert!(o.commands.is_empty());

        // Enters the mute range.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
        assert_eq!(o.filter_category.as_deref(), Some("language"));

        // Still inside: idempotent, no repeated Mute.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(15.0), true, now);
        assert!(o.commands.is_empty());

        // Leaves the range: unmute.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn skip_dispatches_once_then_cools_down() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let t0 = Instant::now();

        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), true, t0);
        assert_eq!(o.commands, vec![FilterCommand::Seek(40.0)]); // cue.end, regardless of pos

        // Immediately again (device hasn't caught up yet): no re-dispatch.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), true, t0);
        assert!(o.commands.is_empty());

        // After the cooldown, still stuck in range: dispatch again.
        let later = t0 + Duration::from_secs(4);
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), true, later);
        assert_eq!(o.commands, vec![FilterCommand::Seek(40.0)]);
    }

    #[test]
    fn paused_skip_cue_does_not_dispatch_until_playing_resumes() {
        let list = sample_list(); // skip cue at [30.0, 40.0)
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Paused inside the skip window: no dispatch, even though the
        // position alone would normally trigger one.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), false, now);
        assert_eq!(o.filter_match.as_deref(), Some("Some Movie"));
        assert!(o.commands.is_empty());

        // Still paused, same position: still nothing.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), false, now);
        assert!(o.commands.is_empty());

        // Resumes at the same position: now it fires.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Seek(40.0)]);
    }

    #[test]
    fn paused_mute_cue_does_not_engage_until_playing_resumes() {
        let list = sample_list(); // mute cue at [10.0, 20.0)
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Paused inside the mute window: no Mute command, and nothing ends
        // up tracked as muted either -- there's nothing to release later.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), false, now);
        assert!(o.commands.is_empty());
        assert!(!runtime.is_muted());

        // Resumes at the same position: mute engages now.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
    }

    #[test]
    fn pausing_leaves_an_already_engaged_mute_untouched() {
        let list = sample_list(); // mute cue at [10.0, 20.0)
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Engages the mute while playing.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
        assert!(runtime.is_muted());

        // Paused, now past the mute range -- if this weren't paused, the
        // range ending would normally release the mute. It shouldn't while
        // paused.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), false, now);
        assert!(o.commands.is_empty());
        assert!(runtime.is_muted());

        // Resumes: now it releases.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn jumping_over_a_skip_window_does_not_seek_backward() {
        let list = sample_list(); // skip cue at [30.0, 40.0)
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Establishes a "previous position" before the skip cue's start.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), true, now);
        assert!(o.commands.is_empty());

        // Next poll lands past the *entire* window (e.g. a background gap,
        // or the user scrubbing past it themselves) -- `cue_at` alone never
        // catches this, since `pos` is already past `end`. Unlike the old
        // relative-skip design (a "best-effort forward nudge" was always
        // safe here), there's nothing to dispatch now: seeking to `cue.end`
        // would move backward, either replaying content already shown or
        // undoing the user's own scrub -- both wrong. See `evaluate`'s doc
        // just above the (removed) catch-up pass.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(45.0), true, now);
        assert!(o.commands.is_empty());
    }

    #[test]
    fn disabling_a_category_mid_mute_forces_unmute() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), true, now);

        let mut disabled = HashSet::new();
        disabled.insert("language".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), None, Some(13.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn disabled_category_never_triggers_a_skip() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let mut disabled = HashSet::new();
        disabled.insert("gore".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), true, Instant::now());
        assert!(o.commands.is_empty());
    }

    #[test]
    fn title_change_while_muted_forces_unmute_and_resets() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), true, now);

        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("A Different Movie"), None, Some(1.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
        assert!(o.filter_match.is_none());
    }

    #[test]
    fn service_change_for_the_same_title_forces_unmute_and_resets() {
        // Two services' entries for the same title, each with a mute cue
        // covering position 12 -- switching from one to the other mid-mute
        // must unmute (the old entry's cue index is meaningless against the
        // new entry) rather than silently carrying the mute over.
        let list = FilterList {
            media: vec![
                entry("Some Movie", "Netflix", vec![cue(10.0, 20.0, CueAction::Mute, "language")]),
                entry("Some Movie", "Disney+", vec![cue(10.0, 20.0, CueAction::Mute, "language")]),
            ],
        };
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some("Netflix"), Some(12.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Mute]);

        // Switches service, same title, same position -- still inside the
        // new entry's mute range too, but the runtime must not assume the
        // mute is already accounted for.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some("Disney+"), Some(12.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute, FilterCommand::Mute]);
    }

    #[test]
    fn exact_service_entry_is_preferred_over_generic_when_both_exist() {
        let list = FilterList {
            media: vec![
                entry("Some Movie", "", vec![cue(10.0, 20.0, CueAction::Mute, "language")]),
                entry("Some Movie", "Netflix", vec![cue(30.0, 40.0, CueAction::Skip, "gore")]),
            ],
        };
        let mut runtime = FilterRuntime::default();
        // At position 12 the generic entry would mute, but the Netflix
        // entry (which should win) has no cue there -- no command either
        // way confirms the generic entry's cue isn't the one being used.
        let o =
            evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some("Netflix"), Some(12.0), true, Instant::now());
        assert!(o.commands.is_empty());
    }

    #[test]
    fn disabling_one_cue_leaves_other_cues_in_the_same_category_alone() {
        // Two mute/"language" cues, back to back, so disabling just the
        // first one's index shouldn't touch the second.
        let list = FilterList {
            media: vec![entry(
                "Some Movie",
                "",
                vec![cue(10.0, 20.0, CueAction::Mute, "language"), cue(20.0, 30.0, CueAction::Mute, "language")],
            )],
        };
        let mut runtime = FilterRuntime::default();
        let mut disabled_cues = HashSet::new();
        disabled_cues.insert(("some movie".to_string(), String::new(), 0));

        // The disabled cue (index 0) never fires.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), None, Some(12.0), true, Instant::now());
        assert!(o.commands.is_empty());

        // The still-enabled cue (index 1) fires normally.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), None, Some(22.0), true, Instant::now());
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
    }

    #[test]
    fn disabling_a_cue_mid_mute_forces_unmute() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), true, now);

        let mut disabled_cues = HashSet::new();
        disabled_cues.insert(("some movie".to_string(), String::new(), 0)); // the mute cue is index 0
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), None, Some(13.0), true, now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn add_cue_creates_entry_when_title_unseen() {
        let mut list = FilterList::default();
        let index = list.add_cue("New Movie", "", cue(10.0, 20.0, CueAction::Mute, "language")).unwrap();
        assert_eq!(index, 0);
        assert_eq!(list.media.len(), 1);
        assert_eq!(list.find_entry("New Movie", "").unwrap().cues.len(), 1);
    }

    #[test]
    fn add_cue_creates_a_separate_entry_per_service() {
        let mut list = FilterList::default();
        list.add_cue("New Movie", "Netflix", cue(10.0, 20.0, CueAction::Mute, "language")).unwrap();
        list.add_cue("New Movie", "Disney+", cue(5.0, 6.0, CueAction::Skip, "gore")).unwrap();
        assert_eq!(list.media.len(), 2);
        assert_eq!(list.find_entry("New Movie", "Netflix").unwrap().cues.len(), 1);
        assert_eq!(list.find_entry("New Movie", "Disney+").unwrap().cues.len(), 1);
    }

    #[test]
    fn add_cue_appends_and_resorts_existing_entry() {
        let mut list = sample_list(); // has cues at [10,20) and [30,40)
        let index = list.add_cue("Some Movie", "", cue(0.0, 5.0, CueAction::Mute, "language")).unwrap();
        assert_eq!(index, 0); // sorts before the existing two
        assert_eq!(list.find_entry("Some Movie", "").unwrap().cues.len(), 3);
    }

    #[test]
    fn add_cue_rejects_overlap_and_leaves_entry_unchanged() {
        let mut list = sample_list();
        let before = list.find_entry("Some Movie", "").unwrap().cues.clone();
        let err = list.add_cue("Some Movie", "", cue(15.0, 22.0, CueAction::Mute, "language"));
        assert!(err.is_err());
        assert_eq!(list.find_entry("Some Movie", "").unwrap().cues, before);
    }

    #[test]
    fn add_cue_rejects_end_before_start() {
        let mut list = FilterList::default();
        assert!(list.add_cue("X", "", cue(10.0, 10.0, CueAction::Mute, "language")).is_err());
    }

    #[test]
    fn add_cue_rejects_empty_category() {
        let mut list = FilterList::default();
        assert!(list.add_cue("X", "", cue(1.0, 2.0, CueAction::Mute, "")).is_err());
    }

    #[test]
    fn update_cue_changes_times_and_resorts() {
        let mut list = sample_list(); // [10,20) mute, [30,40) skip
        list.update_cue("Some Movie", "", 1, 0.0, 5.0).unwrap(); // move the skip cue (index 1) earliest
        let cues = &list.find_entry("Some Movie", "").unwrap().cues;
        assert_eq!(cues[0].start, 0.0);
        assert_eq!(cues[0].action, CueAction::Skip);
    }

    #[test]
    fn update_cue_rejects_new_overlap_and_leaves_original_times_unchanged() {
        let mut list = sample_list();
        let before = list.find_entry("Some Movie", "").unwrap().cues.clone();
        let err = list.update_cue("Some Movie", "", 1, 15.0, 22.0); // would overlap the mute cue
        assert!(err.is_err());
        assert_eq!(list.find_entry("Some Movie", "").unwrap().cues, before);
    }

    #[test]
    fn update_cue_rejects_out_of_range_index() {
        let mut list = sample_list();
        assert!(list.update_cue("Some Movie", "", 5, 0.0, 1.0).is_err());
    }

    #[test]
    fn delete_cue_removes_cue() {
        let mut list = sample_list();
        list.delete_cue("Some Movie", "", 0).unwrap();
        let cues = &list.find_entry("Some Movie", "").unwrap().cues;
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].action, CueAction::Skip);
    }

    #[test]
    fn delete_cue_rejects_out_of_range_index() {
        let mut list = sample_list();
        assert!(list.delete_cue("Some Movie", "", 5).is_err());
    }

    #[test]
    fn delete_cue_rejects_unknown_title() {
        let mut list = sample_list();
        assert!(list.delete_cue("Nonexistent", "", 0).is_err());
    }

    #[test]
    fn set_entry_service_renames() {
        let mut list = sample_list(); // generic entry
        list.set_entry_service("Some Movie", "", "Netflix").unwrap();
        assert!(list.find_entry("Some Movie", "").is_none());
        assert_eq!(list.find_entry("Some Movie", "Netflix").unwrap().cues.len(), 2);
    }

    #[test]
    fn set_entry_service_rejects_collision_with_an_existing_entry() {
        let mut list = FilterList {
            media: vec![entry("Some Movie", "", vec![]), entry("Some Movie", "Netflix", vec![])],
        };
        assert!(list.set_entry_service("Some Movie", "", "Netflix").is_err());
    }

    #[test]
    fn set_entry_service_rejects_unknown_source_entry() {
        let mut list = sample_list();
        assert!(list.set_entry_service("Some Movie", "Netflix", "Disney+").is_err());
    }

    #[test]
    fn save_round_trips_through_parse_and_validate() {
        let list = sample_list();
        let json = serde_json::to_string(&list).unwrap();
        let reloaded = FilterList::parse_and_validate(&json).unwrap();
        assert_eq!(reloaded.media.len(), list.media.len());
        assert_eq!(reloaded.find_entry("Some Movie", "").unwrap().cues.len(), 2);
    }
}

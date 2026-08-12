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

/// Minimum time to wait before re-dispatching a skip for the *same* cue --
/// without this, a poll tick landing inside `[start, end)` again before the
/// device has caught up to the previous skip would re-dispatch every poll
/// interval until it does.
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

    /// Skip cues whose *entire* window was jumped clean over between the
    /// previous known position and this one -- e.g. a background gap or a
    /// large position-drift correction that landed past `end` without this
    /// engine ever observing a position inside `[start, end)`, so `cue_at`
    /// above never got a chance to catch (and dispatch a skip for) them.
    /// Mute cues aren't included: muting audio after it's already played
    /// achieves nothing, so there's nothing meaningful to catch up on for
    /// those the way there is for skip.
    fn skipped_over_cues<'a>(
        &'a self,
        prev_pos: f64,
        pos: f64,
        disabled_categories: &HashSet<String>,
        disabled_cues: &HashSet<CueKey>,
    ) -> Vec<(usize, &'a Cue)> {
        self.cues
            .iter()
            .enumerate()
            .filter(|(idx, c)| {
                c.action == CueAction::Skip
                    && prev_pos < c.start
                    && pos >= c.end
                    && self.cue_enabled(*idx, c, disabled_categories, disabled_cues)
            })
            .collect()
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
    /// Relative skip, in seconds -- always positive; matches
    /// `CompanionSession::skip`'s (also relative) signature.
    Skip(f64),
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
/// snapshot (title + service + position), decides what (if anything) should
/// happen. `service` is whichever app is currently "now playing" -- see
/// `control::app_display_name` -- or `None` if that couldn't be determined,
/// in which case only a generic (service-unspecified) entry can match (see
/// `FilterList::find_entry_for_playback`). No I/O -- the caller
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
                outcome.commands.push(FilterCommand::Skip((cue.end - pos).max(0.1)));
                outcome.filter_action = Some("auto-skipped");
                outcome.filter_category = Some(cue.category.clone());
                runtime.last_skip = Some((idx, now));
            }
        }
    }

    // Catch-up pass: any skip cue whose entire window got jumped clean over
    // since the last poll (see `skipped_over_cues`) never shows up as
    // `found` above, since `pos` is already past its `end` by the time we
    // notice -- dispatch a best-effort skip for each anyway, just in case
    // there's still something left to skip past. Same "end minus current
    // position" amount `found`'s skip uses above (i.e. still subtracting
    // however far over `pos` already is), just evaluated past the window
    // instead of inside it, so it naturally clamps to the token 0.1s
    // minimum once there's truly nothing left. Self-limiting without extra
    // bookkeeping: `runtime.last_position` becomes `pos` (>= this cue's
    // `end`) right below, so the `prev_pos < cue.start` condition can never
    // match this same cue again for the rest of this entry.
    if let Some(prev_pos) = runtime.last_position {
        for (idx, cue) in entry.skipped_over_cues(prev_pos, pos, disabled_categories, disabled_cues) {
            outcome.commands.push(FilterCommand::Skip((cue.end - pos).max(0.1)));
            outcome.filter_action = Some("auto-skipped (caught up)");
            outcome.filter_category = Some(cue.category.clone());
            runtime.last_skip = Some((idx, now));
        }
    }
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
        let outcome = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), None, None, None, Instant::now());
        assert!(outcome.filter_match.is_none());
        assert!(outcome.commands.is_empty());
    }

    #[test]
    fn entering_and_leaving_a_mute_range() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Before the cue: no commands.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(5.0), now);
        assert_eq!(o.filter_match.as_deref(), Some("Some Movie"));
        assert!(o.commands.is_empty());

        // Enters the mute range.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
        assert_eq!(o.filter_category.as_deref(), Some("language"));

        // Still inside: idempotent, no repeated Mute.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(15.0), now);
        assert!(o.commands.is_empty());

        // Leaves the range: unmute.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn skip_dispatches_once_then_cools_down() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let t0 = Instant::now();

        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), t0);
        assert_eq!(o.commands, vec![FilterCommand::Skip(8.0)]); // end(40) - pos(32)

        // Immediately again (device hasn't caught up yet): no re-dispatch.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), t0);
        assert!(o.commands.is_empty());

        // After the cooldown, still stuck in range: dispatch again.
        let later = t0 + Duration::from_secs(4);
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), later);
        assert_eq!(o.commands, vec![FilterCommand::Skip(8.0)]);
    }

    #[test]
    fn catch_up_skip_fires_when_a_skip_window_is_jumped_clean_over() {
        let list = sample_list(); // skip cue at [30.0, 40.0)
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Establishes a "previous position" before the skip cue's start.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), now);
        assert!(o.commands.is_empty());

        // Next poll lands past the *entire* window (e.g. a background gap)
        // -- cue_at alone would never catch this, since pos is already past
        // `end`, but the catch-up pass should still fire a best-effort skip.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(45.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Skip(0.1)]); // end(40) - pos(45), clamped to the minimum
    }

    #[test]
    fn catch_up_skip_does_not_fire_on_a_titles_very_first_poll() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        // First-ever poll for this title lands already past the skip
        // window -- no prior position to compare against, so this must not
        // be treated as "missed while running" (vs. "just started here").
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(45.0), Instant::now());
        assert!(o.commands.is_empty());
    }

    #[test]
    fn catch_up_skip_fires_only_once_per_miss() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), now);
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(45.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Skip(0.1)]);

        // Polled again at the same (or any later) position: no repeat --
        // `last_position` is now past this cue's start, so it can't look
        // "freshly jumped over" again.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(45.0), now);
        assert!(o.commands.is_empty());
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(60.0), now);
        assert!(o.commands.is_empty());
    }

    #[test]
    fn catch_up_skip_respects_a_disabled_category() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(25.0), now);

        let mut disabled = HashSet::new();
        disabled.insert("gore".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), None, Some(45.0), now);
        assert!(o.commands.is_empty());
    }

    #[test]
    fn disabling_a_category_mid_mute_forces_unmute() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), now);

        let mut disabled = HashSet::new();
        disabled.insert("language".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), None, Some(13.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn disabled_category_never_triggers_a_skip() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let mut disabled = HashSet::new();
        disabled.insert("gore".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), None, Some(32.0), Instant::now());
        assert!(o.commands.is_empty());
    }

    #[test]
    fn title_change_while_muted_forces_unmute_and_resets() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), now);

        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("A Different Movie"), None, Some(1.0), now);
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
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some("Netflix"), Some(12.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Mute]);

        // Switches service, same title, same position -- still inside the
        // new entry's mute range too, but the runtime must not assume the
        // mute is already accounted for.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some("Disney+"), Some(12.0), now);
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
            evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some("Netflix"), Some(12.0), Instant::now());
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
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), None, Some(12.0), Instant::now());
        assert!(o.commands.is_empty());

        // The still-enabled cue (index 1) fires normally.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), None, Some(22.0), Instant::now());
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
    }

    #[test]
    fn disabling_a_cue_mid_mute_forces_unmute() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), None, Some(12.0), now);

        let mut disabled_cues = HashSet::new();
        disabled_cues.insert(("some movie".to_string(), String::new(), 0)); // the mute cue is index 0
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), None, Some(13.0), now);
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

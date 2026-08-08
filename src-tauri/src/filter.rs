//! Auto-filter mode: loads a cue file (time ranges per media title, each
//! tagged with an action -- mute or skip -- and a free-form content
//! category like "language" or "gore") and decides what to do about it as
//! playback position advances. Deliberately kept out of `libs/appletv` --
//! that crate is the reusable, protocol-only Apple TV library (its own repo/
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
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MediaEntry {
    pub title: String,
    #[serde(default)]
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FilterList {
    pub media: Vec<MediaEntry>,
}

/// Identifies one cue for the individual-cue on/off toggle: the entry's
/// normalized title plus its index within that entry's (sorted-by-start)
/// `cues` -- stable for as long as one `FilterList` stays loaded, since
/// cues are sorted once at parse time and never reordered after. Scoped by
/// title (not just index) so toggling cue #2 off for one movie can't
/// accidentally affect cue #2 of an unrelated one.
pub type CueKey = (String, usize);

/// `pub(crate)` rather than private: `control.rs` needs it too, to build the
/// same key when the frontend reports which cue (by title + index) got
/// toggled.
pub(crate) fn normalize_title(t: &str) -> String {
    t.trim().to_lowercase()
}

impl MediaEntry {
    /// Whether cue `idx` is currently eligible to fire at all -- neither its
    /// category nor the cue itself has been individually disabled.
    fn cue_enabled(&self, idx: usize, cue: &Cue, disabled_categories: &HashSet<String>, disabled_cues: &HashSet<CueKey>) -> bool {
        !disabled_categories.contains(&cue.category) && !disabled_cues.contains(&(normalize_title(&self.title), idx))
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

        let mut seen_titles = HashSet::new();
        for entry in &mut list.media {
            if entry.title.trim().is_empty() {
                bail!("a media entry has an empty title");
            }
            if !seen_titles.insert(normalize_title(&entry.title)) {
                bail!("duplicate title {:?} in filter file", entry.title);
            }

            entry.cues.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
            let mut prev_end: Option<f64> = None;
            for cue in &entry.cues {
                if !(cue.start.is_finite() && cue.end.is_finite()) {
                    bail!("{:?} has a non-finite cue start/end", entry.title);
                }
                if cue.start < 0.0 || cue.end <= cue.start {
                    bail!("{:?} has a cue with start >= end ({} >= {})", entry.title, cue.start, cue.end);
                }
                if cue.category.trim().is_empty() {
                    bail!("{:?} has a cue with an empty category", entry.title);
                }
                if let Some(prev_end) = prev_end {
                    if cue.start < prev_end {
                        bail!("{:?} has overlapping cues around {}", entry.title, cue.start);
                    }
                }
                prev_end = Some(cue.end);
            }
        }

        Ok(list)
    }

    pub fn find_entry(&self, title: &str) -> Option<&MediaEntry> {
        let norm = normalize_title(title);
        self.media.iter().find(|e| normalize_title(&e.title) == norm)
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
/// which title it last saw, which cue (if any) currently holds an
/// auto-applied mute, when a skip was last dispatched for which cue (to
/// throttle re-dispatch while the device catches up), and the position last
/// seen (to detect a skip cue's window being jumped clean over).
#[derive(Debug, Default)]
pub struct FilterRuntime {
    active_title: Option<String>,
    muted_cue: Option<usize>,
    last_skip: Option<(usize, Instant)>,
    /// The position observed on the *previous* `evaluate` call for the
    /// current `active_title`. `None` right after a title change, which
    /// deliberately disables the catch-up pass for that title's very first
    /// poll: with no prior position to compare against, there's no way to
    /// tell "we were tracking this title and missed a cue" apart from
    /// "playback just started already past one" -- only the former should
    /// trigger a catch-up skip.
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

    /// Drops all tracked state (matched title, active mute, skip cooldown).
    /// Callers are responsible for issuing an `unmute()` first if
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
    /// list matches the current now-playing title (including "nothing is
    /// playing").
    pub filter_match: Option<String>,
    pub commands: Vec<FilterCommand>,
    pub filter_action: Option<&'static str>,
    pub filter_category: Option<String>,
}

/// Pure decision function: given the loaded list, the running state, which
/// categories and individual cues are currently disabled, and one playback
/// snapshot (title + position), decides what (if anything) should happen.
/// No I/O -- the caller (`control.rs`) is responsible for actually issuing
/// the returned commands against the live/Companion sessions, which is what
/// makes this testable without a real device.
pub fn evaluate(
    list: &FilterList,
    runtime: &mut FilterRuntime,
    disabled_categories: &HashSet<String>,
    disabled_cues: &HashSet<CueKey>,
    title: Option<&str>,
    position: Option<f64>,
    now: Instant,
) -> FilterOutcome {
    let mut outcome = FilterOutcome::default();

    let title_changed = runtime.active_title.as_deref().map(normalize_title) != title.map(normalize_title);
    if title_changed {
        // Never leave audio stuck muted across a title/episode change --
        // whatever cue held the mute no longer applies once the title moves
        // on, so release it immediately rather than waiting for the next
        // (now-irrelevant) cue lookup to notice.
        if runtime.muted_cue.take().is_some() {
            outcome.commands.push(FilterCommand::Unmute);
        }
        runtime.last_skip = None;
        runtime.last_position = None;
        runtime.active_title = title.map(str::to_string);
    }

    let Some(entry) = title.and_then(|t| list.find_entry(t)) else {
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
    // match this same cue again for the rest of this title.
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

    fn sample_list() -> FilterList {
        FilterList {
            media: vec![MediaEntry {
                title: "Some Movie".to_string(),
                cues: vec![
                    cue(10.0, 20.0, CueAction::Mute, "language"),
                    cue(30.0, 40.0, CueAction::Skip, "gore"),
                ],
            }],
        }
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
    fn rejects_duplicate_titles() {
        let json = r#"{
            "media": [
                { "title": "Same", "cues": [] },
                { "title": "  same ", "cues": [] }
            ]
        }"#;
        assert!(FilterList::parse_and_validate(json).is_err());
    }

    #[test]
    fn find_entry_is_case_insensitive_and_trims() {
        let list = sample_list();
        assert!(list.find_entry("  some movie ").is_some());
        assert!(list.find_entry("SOME MOVIE").is_some());
        assert!(list.find_entry("Some Other Movie").is_none());
    }

    #[test]
    fn categories_dedupes_in_first_seen_order() {
        let list = FilterList {
            media: vec![
                MediaEntry {
                    title: "A".to_string(),
                    cues: vec![cue(0.0, 1.0, CueAction::Mute, "language"), cue(2.0, 3.0, CueAction::Skip, "gore")],
                },
                MediaEntry { title: "B".to_string(), cues: vec![cue(0.0, 1.0, CueAction::Mute, "language")] },
            ],
        };
        assert_eq!(list.categories(), vec!["language".to_string(), "gore".to_string()]);
    }

    fn empty_disabled() -> HashSet<String> {
        HashSet::new()
    }

    fn empty_disabled_cues() -> HashSet<CueKey> {
        HashSet::new()
    }

    #[test]
    fn no_title_produces_no_match_or_commands() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let outcome = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), None, None, Instant::now());
        assert!(outcome.filter_match.is_none());
        assert!(outcome.commands.is_empty());
    }

    #[test]
    fn entering_and_leaving_a_mute_range() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Before the cue: no commands.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(5.0), now);
        assert_eq!(o.filter_match.as_deref(), Some("Some Movie"));
        assert!(o.commands.is_empty());

        // Enters the mute range.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(12.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
        assert_eq!(o.filter_category.as_deref(), Some("language"));

        // Still inside: idempotent, no repeated Mute.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(15.0), now);
        assert!(o.commands.is_empty());

        // Leaves the range: unmute.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(25.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn skip_dispatches_once_then_cools_down() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let t0 = Instant::now();

        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(32.0), t0);
        assert_eq!(o.commands, vec![FilterCommand::Skip(8.0)]); // end(40) - pos(32)

        // Immediately again (device hasn't caught up yet): no re-dispatch.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(32.0), t0);
        assert!(o.commands.is_empty());

        // After the cooldown, still stuck in range: dispatch again.
        let later = t0 + Duration::from_secs(4);
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(32.0), later);
        assert_eq!(o.commands, vec![FilterCommand::Skip(8.0)]);
    }

    #[test]
    fn catch_up_skip_fires_when_a_skip_window_is_jumped_clean_over() {
        let list = sample_list(); // skip cue at [30.0, 40.0)
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();

        // Establishes a "previous position" before the skip cue's start.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(25.0), now);
        assert!(o.commands.is_empty());

        // Next poll lands past the *entire* window (e.g. a background gap)
        // -- cue_at alone would never catch this, since pos is already past
        // `end`, but the catch-up pass should still fire a best-effort skip.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(45.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Skip(0.1)]); // end(40) - pos(45), clamped to the minimum
    }

    #[test]
    fn catch_up_skip_does_not_fire_on_a_titles_very_first_poll() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        // First-ever poll for this title lands already past the skip
        // window -- no prior position to compare against, so this must not
        // be treated as "missed while running" (vs. "just started here").
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(45.0), Instant::now());
        assert!(o.commands.is_empty());
    }

    #[test]
    fn catch_up_skip_fires_only_once_per_miss() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(25.0), now);
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(45.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Skip(0.1)]);

        // Polled again at the same (or any later) position: no repeat --
        // `last_position` is now past this cue's start, so it can't look
        // "freshly jumped over" again.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(45.0), now);
        assert!(o.commands.is_empty());
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(60.0), now);
        assert!(o.commands.is_empty());
    }

    #[test]
    fn catch_up_skip_respects_a_disabled_category() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(25.0), now);

        let mut disabled = HashSet::new();
        disabled.insert("gore".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), Some(45.0), now);
        assert!(o.commands.is_empty());
    }

    #[test]
    fn disabling_a_category_mid_mute_forces_unmute() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(12.0), now);

        let mut disabled = HashSet::new();
        disabled.insert("language".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), Some(13.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }

    #[test]
    fn disabled_category_never_triggers_a_skip() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let mut disabled = HashSet::new();
        disabled.insert("gore".to_string());
        let o = evaluate(&list, &mut runtime, &disabled, &empty_disabled_cues(), Some("Some Movie"), Some(32.0), Instant::now());
        assert!(o.commands.is_empty());
    }

    #[test]
    fn title_change_while_muted_forces_unmute_and_resets() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(12.0), now);

        let o = evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("A Different Movie"), Some(1.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
        assert!(o.filter_match.is_none());
    }

    #[test]
    fn disabling_one_cue_leaves_other_cues_in_the_same_category_alone() {
        // Two mute/"language" cues, back to back, so disabling just the
        // first one's index shouldn't touch the second.
        let list = FilterList {
            media: vec![MediaEntry {
                title: "Some Movie".to_string(),
                cues: vec![
                    cue(10.0, 20.0, CueAction::Mute, "language"),
                    cue(20.0, 30.0, CueAction::Mute, "language"),
                ],
            }],
        };
        let mut runtime = FilterRuntime::default();
        let mut disabled_cues = HashSet::new();
        disabled_cues.insert(("some movie".to_string(), 0));

        // The disabled cue (index 0) never fires.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), Some(12.0), Instant::now());
        assert!(o.commands.is_empty());

        // The still-enabled cue (index 1) fires normally.
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), Some(22.0), Instant::now());
        assert_eq!(o.commands, vec![FilterCommand::Mute]);
    }

    #[test]
    fn disabling_a_cue_mid_mute_forces_unmute() {
        let list = sample_list();
        let mut runtime = FilterRuntime::default();
        let now = Instant::now();
        evaluate(&list, &mut runtime, &empty_disabled(), &empty_disabled_cues(), Some("Some Movie"), Some(12.0), now);

        let mut disabled_cues = HashSet::new();
        disabled_cues.insert(("some movie".to_string(), 0)); // the mute cue is index 0
        let o = evaluate(&list, &mut runtime, &empty_disabled(), &disabled_cues, Some("Some Movie"), Some(13.0), now);
        assert_eq!(o.commands, vec![FilterCommand::Unmute]);
    }
}

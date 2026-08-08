//! Tauri commands for driving a live control session against a saved
//! pairing -- mute/unmute, skip forward/back, and now-playing title/
//! position. Mirrors `libs/appletv-cli`'s `control_flow` and
//! `show_live_position`, but restructured for request/response commands
//! plus frontend-side polling instead of one long-lived interactive loop:
//! the CLI owns its session for the duration of a blocking menu loop,
//! whereas this app's mute/skip/status calls arrive as independent,
//! possibly concurrent Tauri commands -- so the session lives in
//! `Mutex`-guarded Tauri state instead, and each command takes the lock
//! just long enough for its own round trip.
//!
//! Skip goes through Companion (`CompanionSession::skip`, required to even
//! reach this screen). Mute/unmute and playback title/position all go
//! through the *live* session (MRP or AirPlay-tunneled MRP) instead --
//! that's optional, so those controls are unavailable when neither was
//! paired (see `ControlInfo::has_live`).
//!
//! Also hosts the auto-filter mode's Tauri commands (`load_filter_file` and
//! friends) -- the mute/skip calls it issues go through this same
//! `ControlState`, so it's simplest to keep them next to the sessions that
//! make those calls rather than behind a second lock. The actual decision
//! logic (what a cue file means, when a cue should fire) lives in
//! `crate::filter`; this module only wires that decision to the live/
//! Companion sessions and exposes it to the frontend.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tauri::State;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use appletv::companion::CompanionSession;
use appletv::{storage, LiveSession};

use crate::creation::CreationState;
use crate::filter::{self, CueKey, FilterList, FilterRuntime};
use crate::DISPLAY_NAME;

#[derive(Default)]
pub struct ControlState {
    session: Option<CompanionSession>,
    live: Option<LiveSession>,
    filter_list: Option<FilterList>,
    /// Master auto-filter on/off -- a cue never fires unless this is also
    /// true, regardless of category/cue state.
    filter_enabled: bool,
    /// Categories the user has turned off this session; empty means every
    /// category found in the loaded file is active. Not persisted across
    /// launches, same as `filter_enabled`.
    disabled_categories: HashSet<String>,
    /// Individual cues the user has turned off this session, on top of the
    /// category-level toggle -- lets "language: on" still exclude one
    /// specific instance. Keyed by `filter::CueKey` (normalized title +
    /// index within that title's cues). Not persisted, same as the others.
    disabled_cues: HashSet<CueKey>,
    filter_runtime: FilterRuntime,
    /// Filter-*creation*-mode state (`crate::creation`) -- a separate draft
    /// `FilterList` being authored live from playback marks, deliberately
    /// distinct from `filter_list` above (which is what's actively muting/
    /// skipping right now). Reset along with everything else whenever
    /// `start_control_session` rebuilds this struct.
    pub(crate) creation: CreationState,
}

impl ControlState {
    /// Mutable access to the live session for `creation.rs`'s Tauri commands
    /// (they need to refresh + read title/position, same as
    /// `control_playback_status` below) without exposing `live`'s field
    /// itself, or the Companion `session` field, outside this module.
    pub(crate) fn live_mut(&mut self) -> Option<&mut LiveSession> {
        self.live.as_mut()
    }
}

/// Runs one filter-engine evaluation against the live session's current
/// title/position and applies whatever mute/unmute/skip commands it
/// produces. Shared by the periodic poll (`control_playback_status`) and
/// anything that changes filter state mid-playback (toggling the mode or a
/// category) so that effect lands right away instead of waiting for the
/// next poll tick.
async fn apply_filter(guard: &mut ControlState) -> (Option<String>, Option<String>, Option<String>) {
    if !guard.filter_enabled || guard.filter_list.is_none() {
        return (None, None, None);
    }
    let Some(live) = guard.live.as_ref() else {
        return (None, None, None);
    };
    let playback = live.playback();
    let title = playback.title().map(str::to_string);
    let position = playback.position_now();

    let outcome = filter::evaluate(
        guard.filter_list.as_ref().expect("checked above"),
        &mut guard.filter_runtime,
        &guard.disabled_categories,
        &guard.disabled_cues,
        title.as_deref(),
        position,
        Instant::now(),
    );

    for cmd in &outcome.commands {
        match cmd {
            filter::FilterCommand::Mute => {
                if let Some(live) = guard.live.as_mut() {
                    let _ = live.mute().await;
                }
            }
            filter::FilterCommand::Unmute => {
                if let Some(live) = guard.live.as_mut() {
                    let _ = live.unmute().await;
                }
            }
            filter::FilterCommand::Skip(seconds) => {
                if let Some(session) = guard.session.as_mut() {
                    let _ = session.skip(*seconds).await;
                }
            }
        }
    }

    (outcome.filter_match, outcome.filter_action.map(str::to_string), outcome.filter_category)
}

#[derive(serde::Serialize)]
pub struct FilterSummary {
    pub path: String,
    pub media_count: usize,
    pub categories: Vec<String>,
}

/// Opens the file at `path` (chosen by the frontend via
/// `@tauri-apps/plugin-dialog`'s native picker), parses + validates it as a
/// filter list, persists the path so it reloads automatically on the next
/// launch, and replaces whatever list was previously loaded. Does *not*
/// enable auto-filter mode by itself -- see `set_filter_enabled`.
#[tauri::command]
pub async fn load_filter_file(state: State<'_, ControlStateHandle>, path: String) -> Result<FilterSummary, String> {
    let path_buf = PathBuf::from(&path);
    let list = FilterList::load(&path_buf).map_err(|e| describe(&e))?;
    let categories = list.categories();
    let media_count = list.media.len();

    // Best-effort: a failure to persist the path just means the user has to
    // re-pick the file next launch, not that this load itself should fail.
    if let Err(e) = filter::save_filter_path(&path_buf) {
        eprintln!("[filter] failed to persist filter_path.store: {e}");
    }

    let mut guard = state.lock().await;
    // Swapping in a new list invalidates any cue index the runtime was
    // tracking against the *old* one -- if a mute is active, release it for
    // real before resetting, rather than just forgetting about it.
    if guard.filter_runtime.is_muted() {
        if let Some(live) = guard.live.as_mut() {
            let _ = live.unmute().await;
        }
    }
    guard.filter_runtime.reset();
    guard.disabled_categories.clear();
    guard.disabled_cues.clear();
    guard.filter_list = Some(list);

    Ok(FilterSummary { path, media_count, categories })
}

/// On app start, tries to reload whatever filter file was last picked (see
/// `filter::load_saved_filter_path`). Populates state but leaves the mode
/// off -- same "auto-load, don't auto-arm" rule as `load_filter_file`.
/// Returns `None` for both "nothing was ever picked" and "the saved path no
/// longer parses" -- either way there's nothing to offer, so the frontend
/// just shows its normal "load a filter file" prompt.
#[tauri::command]
pub async fn check_saved_filter_file(state: State<'_, ControlStateHandle>) -> Result<Option<FilterSummary>, String> {
    let Some(path) = filter::load_saved_filter_path() else {
        return Ok(None);
    };
    let Ok(list) = FilterList::load(&path) else {
        return Ok(None);
    };
    let categories = list.categories();
    let media_count = list.media.len();

    let mut guard = state.lock().await;
    guard.filter_runtime.reset();
    guard.disabled_categories.clear();
    guard.disabled_cues.clear();
    guard.filter_list = Some(list);

    Ok(Some(FilterSummary { path: path.to_string_lossy().into_owned(), media_count, categories }))
}

/// Flips the master auto-filter toggle. Turning it off while a filter-
/// applied mute is active immediately unmutes -- disabling the mode must
/// never leave the family stuck with muted audio.
#[tauri::command]
pub async fn set_filter_enabled(state: State<'_, ControlStateHandle>, enabled: bool) -> Result<(), String> {
    let mut guard = state.lock().await;
    guard.filter_enabled = enabled;
    if enabled {
        // Land any cue that's already due right away rather than waiting up
        // to a second for the next poll tick.
        let _ = apply_filter(&mut guard).await;
    } else if guard.filter_runtime.is_muted() {
        if let Some(live) = guard.live.as_mut() {
            live.unmute().await.map_err(|e| describe(&e))?;
        }
        guard.filter_runtime.reset();
    }
    Ok(())
}

/// Enables/disables one category by name (must match a value returned in
/// `FilterSummary::categories`). Applies immediately -- e.g. disabling
/// "language" while a language-category mute is active unmutes right away,
/// via the same `apply_filter` re-evaluation `set_filter_enabled` uses.
#[tauri::command]
pub async fn set_filter_category_enabled(
    state: State<'_, ControlStateHandle>,
    category: String,
    enabled: bool,
) -> Result<(), String> {
    let mut guard = state.lock().await;
    if enabled {
        guard.disabled_categories.remove(&category);
    } else {
        guard.disabled_categories.insert(category);
    }
    let _ = apply_filter(&mut guard).await;
    Ok(())
}

/// Enables/disables one individual cue, identified by its matched title plus
/// its index within `PlaybackStatus::filter_cues` (which is exactly
/// `MediaEntry::cues`'s order, since that's cloned as-is -- see
/// `control_playback_status`). Layers on top of the category toggle: a cue
/// only fires if both its category *and* this are enabled. Applies
/// immediately, same as `set_filter_category_enabled`.
#[tauri::command]
pub async fn set_filter_cue_enabled(
    state: State<'_, ControlStateHandle>,
    title: String,
    index: usize,
    enabled: bool,
) -> Result<(), String> {
    let mut guard = state.lock().await;
    let key: CueKey = (filter::normalize_title(&title), index);
    if enabled {
        guard.disabled_cues.remove(&key);
    } else {
        guard.disabled_cues.insert(key);
    }
    let _ = apply_filter(&mut guard).await;
    Ok(())
}

pub type ControlStateHandle = Arc<Mutex<ControlState>>;

pub(crate) fn describe(e: &anyhow::Error) -> String {
    appletv::error_chain(e).join(": ")
}

#[derive(serde::Serialize)]
pub struct ControlInfo {
    /// Whether MRP or AirPlay is also paired, unlocking mute/unmute and
    /// playback title/position. Skip works either way (Companion only).
    pub has_live: bool,
}

/// Loads `pairing.store`, runs Pair-Verify + bootstraps a Companion control
/// session, and connects a live (MRP/AirPlay) session if one was paired.
/// Replaces whatever control session (if any) was already active.
#[tauri::command]
pub async fn start_control_session(state: State<'_, ControlStateHandle>) -> Result<ControlInfo, String> {
    let saved = storage::load_pairing()
        .map_err(|e| describe(&e))?
        .ok_or_else(|| "No saved pairing found".to_string())?;

    let mut stream = TcpStream::connect(format!("{}:{}", saved.companion.host, saved.companion.port))
        .await
        .map_err(|e| format!("failed to connect: {e}"))?;
    let keys = appletv::hap_pair::pair_verify(&mut stream, &saved.companion.creds)
        .await
        .map_err(|e| describe(&e))?;

    let mut session = CompanionSession::new(stream, keys);
    session
        .bootstrap(&saved.companion.creds.pairing_id)
        .await
        .map_err(|e| describe(&e))?;

    let live = appletv::connect_live_session(&saved, DISPLAY_NAME).await;
    let has_live = live.is_some();

    // Resets any previously loaded filter list too (`..Default::default()`)
    // -- the frontend re-checks for a saved one right after this via
    // `check_saved_filter_file`, same as it re-checks pairing on mount.
    *state.lock().await = ControlState { session: Some(session), live, ..Default::default() };
    Ok(ControlInfo { has_live })
}

#[tauri::command]
pub async fn control_mute(state: State<'_, ControlStateHandle>) -> Result<(), String> {
    let mut guard = state.lock().await;
    let live = guard
        .live
        .as_mut()
        .ok_or_else(|| "No live transport paired (pair MRP or AirPlay to enable mute/unmute)".to_string())?;
    live.mute().await.map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn control_unmute(state: State<'_, ControlStateHandle>) -> Result<(), String> {
    let mut guard = state.lock().await;
    let live = guard
        .live
        .as_mut()
        .ok_or_else(|| "No live transport paired (pair MRP or AirPlay to enable mute/unmute)".to_string())?;
    live.unmute().await.map_err(|e| describe(&e))
}

/// Skip forward (`seconds > 0`) or back (`seconds < 0`) via Companion.
#[tauri::command]
pub async fn control_skip(state: State<'_, ControlStateHandle>, seconds: f64) -> Result<(), String> {
    let mut guard = state.lock().await;
    let session = guard
        .session
        .as_mut()
        .ok_or_else(|| "No control session active -- open controls again".to_string())?;
    session.skip(seconds).await.map_err(|e| describe(&e))
}

/// One cue as shown to the frontend: the raw cue fields plus its position in
/// the matched entry's list (what `set_filter_cue_enabled` expects back) and
/// whether it's currently eligible to fire at all (category *and*
/// individual-cue toggles both on). Distinct from `filter::Cue` -- that's
/// just what's in the file; this is "that, plus this session's overrides".
#[derive(serde::Serialize)]
pub struct CueStatus {
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub action: filter::CueAction,
    pub category: String,
    pub enabled: bool,
}

#[derive(serde::Serialize)]
pub struct PlaybackStatus {
    pub title: Option<String>,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub playback_state: String,
    /// The loaded filter list's matching title, or `None` if nothing's
    /// loaded or nothing in the list matches the current now-playing title.
    /// Populated whenever a list is loaded regardless of the master
    /// enabled toggle, so the schedule below is visible as a preview even
    /// before auto-filter mode is turned on.
    pub filter_match: Option<String>,
    /// Every cue for the matched title, in order -- lets the frontend show
    /// the full schedule (with times) rather than just the one currently
    /// firing.
    pub filter_cues: Vec<CueStatus>,
    /// Set for one poll when this tick's evaluation just fired a mute or
    /// skip -- e.g. "auto-muted" / "auto-skipped" -- for a one-shot UI hint.
    /// Unlike `filter_match`/`filter_cues`, only ever set while the master
    /// toggle is on (nothing is actually applied while it's off).
    pub filter_action: Option<String>,
    /// The category of the cue behind `filter_action`, when set.
    pub filter_category: Option<String>,
}

/// One snapshot of the current now-playing state, for the frontend to poll
/// on an interval (e.g. every second). Returns `Ok(None)` rather than an
/// error when there's no live transport -- that's an expected, steady
/// state (MRP/AirPlay weren't paired), not a failure.
///
/// Actively re-requests the current item's metadata on every call rather
/// than extrapolating locally between occasional refreshes, so a skip, or
/// a pause/seek from the physical remote, shows up on the caller's *next*
/// poll automatically -- no manual "refresh now" action needed to stay in
/// sync, at the cost of one extra request per poll interval against the
/// device.
#[tauri::command]
pub async fn control_playback_status(state: State<'_, ControlStateHandle>) -> Result<Option<PlaybackStatus>, String> {
    let mut guard = state.lock().await;
    let Some(live) = guard.live.as_mut() else {
        return Ok(None);
    };

    // Best-effort: an occasional refresh failure shouldn't hide the
    // still-good extrapolated position below.
    let _ = live.refresh_position().await;

    let playback = live.playback();
    let title = playback.title().map(str::to_string);
    let position = playback.position_now();
    let duration = playback.duration();
    let playback_state = format!("{:?}", playback.playback_state());

    // Looked up independently of the master enabled toggle -- unlike
    // `apply_filter` below, this is a preview ("what would happen"), not an
    // application of it, so it stays populated even while auto-filter mode
    // is off.
    let matched_entry = guard
        .filter_list
        .as_ref()
        .and_then(|list| title.as_deref().and_then(|t| list.find_entry(t)));
    let filter_match = matched_entry.map(|e| e.title.clone());
    let filter_cues: Vec<CueStatus> = matched_entry
        .map(|entry| {
            let title_key = filter::normalize_title(&entry.title);
            entry
                .cues
                .iter()
                .enumerate()
                .map(|(index, cue)| CueStatus {
                    index,
                    start: cue.start,
                    end: cue.end,
                    action: cue.action,
                    category: cue.category.clone(),
                    enabled: !guard.disabled_categories.contains(&cue.category)
                        && !guard.disabled_cues.contains(&(title_key.clone(), index)),
                })
                .collect()
        })
        .unwrap_or_default();

    // Runs after the position refresh above so it's evaluating against the
    // freshest possible position, and before returning so a mute/skip it
    // decides on shows up in this same response rather than a poll late.
    // Its own `filter_match` is redundant with the one above (same lookup,
    // just gated on `filter_enabled` too) so it's discarded here.
    let (_, filter_action, filter_category) = apply_filter(&mut guard).await;

    Ok(Some(PlaybackStatus {
        title,
        position,
        duration,
        playback_state,
        filter_cues,
        filter_match,
        filter_action,
        filter_category,
    }))
}

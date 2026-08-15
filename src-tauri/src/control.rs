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
use std::time::{Duration, Instant};

use tauri::{AppHandle, State};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

use appletv::companion::{CompanionSession, HidButton};
use appletv::{storage, LiveSession};

use crate::creation::CreationState;
use crate::filter::{self, CueKey, FilterList, FilterRuntime};
use crate::{library, metadata, DISPLAY_NAME};

#[derive(Default)]
pub struct ControlState {
    session: Option<CompanionSession>,
    live: Option<LiveSession>,
    filter_list: Option<FilterList>,
    /// Where `filter_list` was loaded from, if anywhere -- set alongside it
    /// by `load_filter_file_inner`/`check_saved_filter_file`, and read back
    /// by `update_filter_cue`/`delete_filter_cue` so an edit made from the
    /// Filters tab can be written straight back to the same file, the same
    /// way a creation-mode draft autosaves to `creation.draft_path`.
    filter_list_path: Option<PathBuf>,
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
    let service = playback.app_bundle_id().and_then(app_display_name);
    let position = playback.position_now();
    // Cues only take effect while content is actually advancing -- see
    // `filter::evaluate`'s `is_playing` doc. Paused/stopped/seeking/unknown
    // all withhold new mute/skip commands (and leave any existing mute
    // alone) until playback resumes. Uses `is_advancing` rather than a bare
    // `playback_state() == Playing` check -- some clients leave
    // `playbackState` stale on pause and only zero out the rate, so relying
    // on state alone let a skip cue still dispatch a real seek to the
    // device even while the locally displayed position had already frozen
    // (see `PlayerSnapshot::is_advancing`'s doc for why the two can
    // disagree).
    let is_playing = playback.is_advancing();

    let outcome = filter::evaluate(
        guard.filter_list.as_ref().expect("checked above"),
        &mut guard.filter_runtime,
        &guard.disabled_categories,
        &guard.disabled_cues,
        title.as_deref(),
        service,
        position,
        is_playing,
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
            filter::FilterCommand::Seek(position) => {
                // MRP's absolute SeekToPlaybackPosition (LiveSession::seek),
                // not Companion's relative `_mcc` SkipBy -- see
                // `filter::FilterCommand::Seek`'s doc for why.
                if let Some(live) = guard.live.as_mut() {
                    let _ = live.seek(*position).await;
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
    /// Master auto-filter toggle's state at the moment this summary was
    /// built -- lets the frontend mirror the backend's persisted value
    /// (see `filter::load_saved_filter_enabled`) instead of assuming it's
    /// always off.
    pub enabled: bool,
}

/// Shared by `load_filter_file` and `select_filter_tile` below -- both need
/// to load a file at a given path into `ControlState.filter_list` and hand
/// back its summary, so that logic lives once here, taking the state handle
/// itself (rather than a `State` extractor, which only a `#[tauri::command]`
/// can receive) so either caller can invoke it directly.
async fn load_filter_file_inner(handle: &ControlStateHandle, path: String) -> Result<FilterSummary, String> {
    let path_buf = PathBuf::from(&path);
    let list = FilterList::load(&path_buf).map_err(|e| describe(&e))?;
    let categories = list.categories();
    let media_count = list.media.len();

    // Best-effort: a failure to persist either just means the user has to
    // re-pick the file next launch, or it won't show up in the Select Filter
    // grid until it's loaded again -- neither should fail this load itself.
    if let Err(e) = filter::save_filter_path(&path_buf) {
        eprintln!("[filter] failed to persist filter_path.store: {e}");
    }
    if let Err(e) = library::register_filter_path(&path_buf) {
        eprintln!("[library] failed to persist filter_library.store: {e}");
    }

    let mut guard = handle.lock().await;
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
    guard.filter_list_path = Some(path_buf);
    // Loading a (possibly different) list must never silently start auto-
    // muting/skipping -- previously this was a given since `filter_enabled`
    // only ever started `false` each launch, but now that it's persisted
    // (see `set_filter_enabled`) a restored session can easily still be
    // `true` here, so this has to force it off explicitly rather than just
    // leaving it alone.
    guard.filter_enabled = false;
    if let Err(e) = filter::save_filter_enabled(false) {
        eprintln!("[filter] failed to persist filter_enabled.store: {e}");
    }

    Ok(FilterSummary { path, media_count, categories, enabled: false })
}

/// Opens the file at `path` (chosen by the frontend via
/// `@tauri-apps/plugin-dialog`'s native picker), parses + validates it as a
/// filter list, persists the path so it reloads automatically on the next
/// launch, and replaces whatever list was previously loaded. Does *not*
/// enable auto-filter mode by itself -- see `set_filter_enabled`.
#[tauri::command]
pub async fn load_filter_file(state: State<'_, ControlStateHandle>, path: String) -> Result<FilterSummary, String> {
    load_filter_file_inner(state.inner(), path).await
}

/// On app start, tries to reload whatever filter file was last picked (see
/// `filter::load_saved_filter_path`) *and* restores the master auto-filter
/// toggle to however it was last left (see `filter::load_saved_filter_
/// enabled`) -- unlike `load_filter_file`, this genuinely can come back
/// armed: it's re-opening the exact same list the toggle was already
/// validated against when the user turned it on, not swapping in a new one
/// out from under it. Returns `None` for both "nothing was ever picked" and
/// "the saved path no longer parses" -- either way there's nothing to
/// offer, so the frontend just shows its normal "load a filter file"
/// prompt (and the enabled flag is left untouched on disk either way, so a
/// transient load failure doesn't erase it for next launch).
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
    let enabled = filter::load_saved_filter_enabled();

    let mut guard = state.lock().await;
    guard.filter_runtime.reset();
    guard.disabled_categories.clear();
    guard.disabled_cues.clear();
    guard.filter_list = Some(list);
    guard.filter_list_path = Some(path.clone());
    guard.filter_enabled = enabled;
    if enabled {
        // Land any cue that's already due right away, same as
        // `set_filter_enabled` does for an explicit toggle -- otherwise a
        // restored session sits un-applied for up to a second until the
        // next poll tick.
        let _ = apply_filter(&mut guard).await;
    }

    Ok(Some(FilterSummary { path: path.to_string_lossy().into_owned(), media_count, categories, enabled }))
}

/// Flips the master auto-filter toggle. Turning it off while a filter-
/// applied mute is active immediately unmutes -- disabling the mode must
/// never leave the family stuck with muted audio.
#[tauri::command]
pub async fn set_filter_enabled(state: State<'_, ControlStateHandle>, enabled: bool) -> Result<(), String> {
    let mut guard = state.lock().await;
    guard.filter_enabled = enabled;
    // Best-effort, same tolerance as everywhere else this pattern shows up:
    // a failure to persist just means the toggle reverts to off on the next
    // launch instead of coming back as left, not something worth failing
    // this call over.
    if let Err(e) = filter::save_filter_enabled(enabled) {
        eprintln!("[filter] failed to persist filter_enabled.store: {e}");
    }
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

/// Enables/disables one individual cue, identified by its matched
/// title+service plus its index within `PlaybackStatus::filter_cues` (which
/// is exactly `MediaEntry::cues`'s order, since that's cloned as-is -- see
/// `control_playback_status`). Layers on top of the category toggle: a cue
/// only fires if both its category *and* this are enabled. Applies
/// immediately, same as `set_filter_category_enabled`.
#[tauri::command]
pub async fn set_filter_cue_enabled(
    state: State<'_, ControlStateHandle>,
    title: String,
    service: String,
    index: usize,
    enabled: bool,
) -> Result<(), String> {
    let mut guard = state.lock().await;
    let key: CueKey = (filter::normalize_title(&title), filter::normalize_service(&service), index);
    if enabled {
        guard.disabled_cues.remove(&key);
    } else {
        guard.disabled_cues.insert(key);
    }
    let _ = apply_filter(&mut guard).await;
    Ok(())
}

/// Best-effort save of the active auto-filter list back to `filter_list_path`
/// -- called after `update_filter_cue`/`delete_filter_cue` actually rewrite a
/// cue, unlike `set_filter_category_enabled`/`set_filter_cue_enabled` above,
/// which only ever touch this *session's* enabled/disabled overrides. Same
/// "log, don't fail the caller" tolerance `load_filter_file_inner`'s own
/// persistence calls use -- a failed write here would otherwise turn a
/// perfectly good in-memory edit into a hard error the user can't do
/// anything about from the Filters tab.
fn persist_active_filter_list(guard: &ControlState) {
    let (Some(list), Some(path)) = (guard.filter_list.as_ref(), guard.filter_list_path.as_ref()) else {
        return;
    };
    if let Err(e) = list.save(path) {
        eprintln!("[filter] failed to persist edit to {}: {e}", path.display());
    }
}

/// Drops every per-cue enabled/disabled override recorded for one (title,
/// service) entry -- called after `update_filter_cue`/`delete_filter_cue`
/// change that entry's cue order (both re-sort; delete also shifts every
/// later index down by one), since `disabled_cues` is keyed by index and a
/// stale entry would otherwise silently start applying to a different cue
/// than the one the user actually turned off. The user re-toggling whichever
/// cues they'd disabled is a fair trade against that happening quietly.
fn clear_disabled_cues_for(guard: &mut ControlState, title_key: &str, service_key: &str) {
    guard.disabled_cues.retain(|(t, s, _)| t != title_key || s != service_key);
}

/// Corrects an existing cue's timing in the *active* auto-filter list -- the
/// one loaded into Select Filter's detail view, as opposed to
/// `creation::creation_update_cue`, which only ever edits a recording draft
/// and needs something currently playing to know which entry to target. This
/// acts on whatever (title, service) the frontend already has open, so it
/// works from the Filters tab regardless of what's on screen right now, and
/// persists straight back to `filter_list_path` rather than staying
/// session-only like the enabled/disabled toggles above.
#[tauri::command]
pub async fn update_filter_cue(
    state: State<'_, ControlStateHandle>,
    title: String,
    service: String,
    index: usize,
    start: f64,
    end: f64,
) -> Result<(), String> {
    let mut guard = state.lock().await;
    let list = guard.filter_list.as_mut().ok_or_else(|| "no filter list loaded".to_string())?;
    list.update_cue(&title, &service, index, start, end).map_err(|e| describe(&e))?;
    persist_active_filter_list(&guard);
    clear_disabled_cues_for(&mut guard, &filter::normalize_title(&title), &filter::normalize_service(&service));
    let _ = apply_filter(&mut guard).await;
    Ok(())
}

/// Removes a cue outright from the *active* auto-filter list -- the
/// Filters-tab counterpart to `creation::creation_delete_cue`, same
/// distinction as `update_filter_cue` above.
#[tauri::command]
pub async fn delete_filter_cue(state: State<'_, ControlStateHandle>, title: String, service: String, index: usize) -> Result<(), String> {
    let mut guard = state.lock().await;
    let list = guard.filter_list.as_mut().ok_or_else(|| "no filter list loaded".to_string())?;
    list.delete_cue(&title, &service, index).map_err(|e| describe(&e))?;
    persist_active_filter_list(&guard);
    clear_disabled_cues_for(&mut guard, &filter::normalize_title(&title), &filter::normalize_service(&service));
    let _ = apply_filter(&mut guard).await;
    Ok(())
}

/// Registers one or more filter files (chosen via
/// `@tauri-apps/plugin-dialog`'s native multi-file picker) into the library,
/// for the Select Filter grid to pick up -- unlike `load_filter_file`, this
/// doesn't make any of them the *active* list; adding a file to the shelf
/// and choosing to play it are separate actions (the latter is what tapping
/// its tile does). Validates each path parses as a filter list before
/// registering it, so a bad file is rejected with an error naming it rather
/// than silently added and then failing to appear in the grid.
#[tauri::command]
pub fn add_filter_files(paths: Vec<String>) -> Result<usize, String> {
    let mut added = 0;
    for path in paths {
        let path_buf = PathBuf::from(&path);
        FilterList::load(&path_buf).map_err(|e| format!("{path}: {}", describe(&e)))?;
        library::register_filter_path(&path_buf).map_err(|e| describe(&e))?;
        added += 1;
    }
    Ok(added)
}

/// Registers every `.json` file directly inside `path` (chosen via the
/// dialog plugin's directory picker) that parses as a valid filter list --
/// not recursive, and silently skips both non-`.json` files and `.json`
/// files that don't parse (e.g. some other JSON file that happens to live
/// in the same folder), rather than failing the whole scan over one bad
/// file. Returns how many were actually added.
#[tauri::command]
pub fn add_filter_directory(path: String) -> Result<usize, String> {
    let dir = PathBuf::from(&path);
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("failed to read {}: {e}", dir.display()))?;
    let mut added = 0;
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if FilterList::load(&entry_path).is_err() {
            continue;
        }
        if library::register_filter_path(&entry_path).is_ok() {
            added += 1;
        }
    }
    Ok(added)
}

/// One poster tile for the Select Filter grid -- one per movie *title*
/// (`crate::library` tracks the *files*; this command flattens every known
/// file's `media` entries into one deduped list of titles for the frontend
/// to render). `path` is which file to hand `select_filter_tile` when this
/// tile is tapped.
#[derive(serde::Serialize)]
pub struct FilterTile {
    pub title: String,
    pub path: String,
    /// `data:` URI, or `None` if TMDB has no key configured, no match, or
    /// the lookup otherwise failed -- the frontend shows a placeholder tile
    /// rather than treating this as an error.
    pub poster: Option<String>,
    /// How many cues this tile's own entry carries, for the grid's badge.
    /// A title with more than one service entry can differ per service --
    /// this is the count for the entry `select_filter_tile` opens by
    /// default, i.e. the one the dedupe below kept.
    pub cue_count: usize,
}

/// Every title across every filter file the library knows about, for the
/// Select Filter grid. Titles are deduped by `filter::normalize_title` --
/// first file registered wins -- since a poster/tap target only makes sense
/// once per distinct title, even if (unusually) more than one file mentions
/// the same movie.
#[tauri::command]
pub async fn list_filter_tiles(app: AppHandle) -> Result<Vec<FilterTile>, String> {
    let mut seen = HashSet::new();
    let mut tiles = Vec::new();
    for path in library::list_library_paths() {
        let Ok(list) = FilterList::load(&path) else {
            // A file that's been moved/deleted/corrupted since it was
            // registered just drops out of the grid silently -- the library
            // itself is left alone so it's not lost if the file reappears.
            continue;
        };
        let path_str = path.to_string_lossy().into_owned();
        for entry in &list.media {
            if !seen.insert(filter::normalize_title(&entry.title)) {
                continue;
            }
            let poster = metadata::poster_data_uri(&app, &entry.title).await;
            tiles.push(FilterTile {
                title: entry.title.clone(),
                path: path_str.clone(),
                poster,
                cue_count: entry.cues.len(),
            });
        }
    }
    Ok(tiles)
}

/// The detail payload for one tapped (title, service) tile: the same
/// per-cue status shape `control_playback_status` builds for whatever's
/// currently playing, just keyed by the tapped entry instead.
#[derive(serde::Serialize)]
pub struct FilterEntryDetail {
    pub title: String,
    pub service: String,
    pub categories: Vec<String>,
    pub cues: Vec<CueStatus>,
}

/// Loads `path` as the active auto-filter list (exactly like
/// `load_filter_file`) and returns the `(title, service)` entry's
/// categories/cues. Tapping a tile (after resolving which service via
/// `list_services_for_title`, see that command's doc comment) selects that
/// whole *file* as active -- a file's titles always shared one active list
/// even before this command existed (see `FilterList`'s doc comment), so
/// this doesn't change that, just gives the frontend an entry-scoped view
/// of the result.
#[tauri::command]
pub async fn select_filter_tile(
    state: State<'_, ControlStateHandle>,
    path: String,
    title: String,
    service: String,
) -> Result<FilterEntryDetail, String> {
    load_filter_file_inner(state.inner(), path).await?;

    let guard = state.lock().await;
    let entry = guard
        .filter_list
        .as_ref()
        .and_then(|list| list.find_entry(&title, &service))
        .ok_or_else(|| format!("{title:?} on {service:?} not found in the selected filter file"))?;

    let mut categories = Vec::new();
    let mut seen = HashSet::new();
    for cue in &entry.cues {
        if seen.insert(cue.category.clone()) {
            categories.push(cue.category.clone());
        }
    }

    let title_key = filter::normalize_title(&entry.title);
    let service_key = filter::normalize_service(&entry.service);
    let cues: Vec<CueStatus> = entry
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
                && !guard.disabled_cues.contains(&(title_key.clone(), service_key.clone(), index)),
        })
        .collect();
    let resolved_title = entry.title.clone();
    let resolved_service = entry.service.clone();

    Ok(FilterEntryDetail { title: resolved_title, service: resolved_service, categories, cues })
}

/// One service variant of a title, as known anywhere in the library (not
/// just the currently active file) -- `path` is which file it lives in, for
/// handing straight to `select_filter_tile`.
#[derive(serde::Serialize)]
pub struct ServiceOption {
    pub service: String,
    pub path: String,
}

/// Every distinct service variant registered for `title`, across every
/// filter file the library knows about -- not just the one currently
/// active. Powers two things on the frontend: the Select Filter service
/// picker shown after tapping a title whose services can't be confidently
/// auto-picked (see `FilterList::find_entry_for_playback`'s doc comment for
/// what "confidently" means), and the Open Controls "a filter is available"
/// auto-detect banner, which looks for an entry matching whatever's
/// actually playing right now among these.
#[tauri::command]
pub fn list_services_for_title(title: String) -> Vec<ServiceOption> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in library::list_library_paths() {
        let Ok(list) = FilterList::load(&path) else { continue };
        let path_str = path.to_string_lossy().into_owned();
        for entry in list.entries_for_title(&title) {
            if seen.insert(filter::normalize_service(&entry.service)) {
                out.push(ServiceOption { service: entry.service.clone(), path: path_str.clone() });
            }
        }
    }
    out
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

/// How long to wait for the initial TCP connect to the saved Companion host
/// before giving up -- a plain `TcpStream::connect` against an unreachable
/// host (asleep, moved to a different IP, off the network) can otherwise
/// hang far longer than this, since nothing sends back a RST to fail it
/// quickly. Without this, the auto-connect attempt on launch (see
/// +page.svelte) would sit indefinitely with no error and no visible
/// "connecting" state -- indistinguishable from having never tried at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

/// Loads `pairing.store`, runs Pair-Verify + bootstraps a Companion control
/// session, and connects a live (MRP/AirPlay) session if one was paired.
/// Replaces whatever control session (if any) was already active.
#[tauri::command]
pub async fn start_control_session(state: State<'_, ControlStateHandle>) -> Result<ControlInfo, String> {
    let saved = storage::load_pairing()
        .map_err(|e| describe(&e))?
        .ok_or_else(|| "No saved pairing found".to_string())?;

    let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(format!("{}:{}", saved.companion.host, saved.companion.port)))
        .await
        .map_err(|_| format!("timed out connecting to {}:{}", saved.companion.host, saved.companion.port))?
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

/// The remote buttons the frontend can send via `control_button` -- the
/// Siri Remote's button ring (arrows, Select, Menu, Home) plus Play/Pause.
/// Deliberately excludes the touchpad's swipe/tap gestures (see
/// `appletv::companion::HidButton`'s doc comment); `#[serde(rename_all =
/// "snake_case")]` so the frontend just passes e.g. `"play_pause"`.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteButton {
    Up,
    Down,
    Left,
    Right,
    Select,
    Menu,
    Home,
    PlayPause,
}

impl From<RemoteButton> for HidButton {
    fn from(button: RemoteButton) -> Self {
        match button {
            RemoteButton::Up => HidButton::Up,
            RemoteButton::Down => HidButton::Down,
            RemoteButton::Left => HidButton::Left,
            RemoteButton::Right => HidButton::Right,
            RemoteButton::Select => HidButton::Select,
            RemoteButton::Menu => HidButton::Menu,
            RemoteButton::Home => HidButton::Home,
            RemoteButton::PlayPause => HidButton::PlayPause,
        }
    }
}

/// Presses (and releases) one Siri Remote button via Companion -- same
/// transport `control_skip` uses, so it needs the Companion session, not
/// the optional live (MRP/AirPlay) one.
#[tauri::command]
pub async fn control_button(state: State<'_, ControlStateHandle>, button: RemoteButton) -> Result<(), String> {
    let mut guard = state.lock().await;
    let session = guard
        .session
        .as_mut()
        .ok_or_else(|| "No control session active -- open controls again".to_string())?;
    session.hid_press(button.into()).await.map_err(|e| describe(&e))
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
    /// The show's name, for a TV episode -- `title` above is just the
    /// episode's own title (e.g. "Chapter 1") in that case. `None` for a
    /// movie or anything else the device doesn't report a series for.
    pub series_name: Option<String>,
    /// Freeform secondary line some apps populate instead of `series_name`
    /// (e.g. PureFlix, which doesn't send the structured field at all) --
    /// not guaranteed to be show-related, just whatever the app put there.
    /// The frontend falls back to this only when `series_name` is absent.
    pub subtitle: Option<String>,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub playback_state: String,
    /// Bundle id of whatever app is currently "now playing" (e.g.
    /// `com.netflix.Netflix`), straight from MRP's
    /// `SetNowPlayingClientMessage`. `None` until the device has announced
    /// one, same as `title`.
    pub app_bundle_id: Option<String>,
    /// Friendly name for `app_bundle_id`, looked up in `app_display_name`
    /// below -- `None` for a bundle id not in that (necessarily incomplete)
    /// table, so the frontend can fall back to showing the raw bundle id
    /// rather than hiding the app entirely.
    pub app_name: Option<String>,
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

/// Best-effort bundle-id -> friendly-name table for the apps most likely to
/// show up as the "now playing" client -- Apple's own (confirmed via
/// https://support.apple.com/guide/deployment/depcdd66fe58) plus the
/// handful of third-party streaming apps whose tvOS bundle id could be
/// confirmed independently. Deliberately incomplete rather than guessed:
/// an unrecognized bundle id just falls back to being shown as-is (see
/// `app_bundle_id` in `PlaybackStatus`) instead of risking a wrong label --
/// if yours shows up as a raw bundle id, that string is exactly what to add
/// a case for here.
///
/// `pub(crate)` rather than private: `creation.rs` reuses it too, to
/// auto-tag a title's `services` (see `filter::MediaEntry::services`) with
/// whichever app was playing when its first cue was recorded.
pub(crate) fn app_display_name(bundle_id: &str) -> Option<&'static str> {
    Some(match bundle_id {
        "com.apple.TVWatchList" => "Apple TV",
        "com.apple.TVMovies" => "Movies",
        "com.apple.TVShows" => "TV Shows",
        "com.apple.TVMusic" => "Music",
        "com.apple.podcasts" => "Podcasts",
        "com.netflix.Netflix" => "Netflix",
        "com.disney.disneyplus" => "Disney+",
        "com.hulu.plus" => "Hulu",
        "com.amazon.aiv.AIVApp" => "Prime Video",
        "com.peacocktv.peacock" => "Peacock",
        "com.google.ios.youtube" => "YouTube",
        "com.plexapp.plex" => "Plex",
        "com.rollingstorm.PureFlix" => "Pure Flix",
        _ => return None,
    })
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
    let series_name = playback.series_name().map(str::to_string);
    let subtitle = playback.subtitle().map(str::to_string);
    let position = playback.position_now();
    let duration = playback.duration();
    let playback_state = format!("{:?}", playback.playback_state());
    let app_bundle_id = playback.app_bundle_id().map(str::to_string);
    let app_name = app_bundle_id.as_deref().and_then(app_display_name).map(str::to_string);

    // Looked up independently of the master enabled toggle -- unlike
    // `apply_filter` below, this is a preview ("what would happen"), not an
    // application of it, so it stays populated even while auto-filter mode
    // is off.
    let matched_entry = guard
        .filter_list
        .as_ref()
        .and_then(|list| title.as_deref().and_then(|t| list.find_entry_for_playback(t, app_name.as_deref())));
    let filter_match = matched_entry.map(|e| e.title.clone());
    let filter_cues: Vec<CueStatus> = matched_entry
        .map(|entry| {
            let title_key = filter::normalize_title(&entry.title);
            let service_key = filter::normalize_service(&entry.service);
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
                        && !guard.disabled_cues.contains(&(title_key.clone(), service_key.clone(), index)),
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
        series_name,
        subtitle,
        position,
        duration,
        playback_state,
        app_bundle_id,
        app_name,
        filter_cues,
        filter_match,
        filter_action,
        filter_category,
    }))
}

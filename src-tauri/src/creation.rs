//! Filter-creation mode: while a title plays live on the paired Apple TV,
//! record cue timestamps straight from the live playback position instead
//! of hand-authoring them -- a language-category press records a brief
//! mute, other categories record a start/end skip pair across two presses.
//!
//! Shares `ControlStateHandle` with `control.rs` (one lock, reuses the
//! already-open `live` session for position/title reads) but the draft
//! `FilterList` it edits is deliberately a separate piece of state from
//! `ControlState::filter_list` -- that one is what's actively muting/
//! skipping right now, and editing it mid-movie would be surprising. Kept
//! out of `control.rs` itself so that file's auto-filter-*playback*
//! narrative doesn't grow this unrelated authoring feature; see that
//! module's doc comment for why it hosts the auto-filter Tauri commands
//! next to the sessions that need them -- the same "avoid a second lock"
//! argument applies here (this also needs `live`), just for a different
//! feature, so this module gets its own file instead.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tauri::State;

use crate::control::{describe, ControlState, ControlStateHandle};
use crate::filter::{self, Cue, CueAction, FilterList};

/// How long a language-category mute mark lasts, in seconds.
/// `creation_mark_mute` accepts an optional override of this so a future
/// "adjustable duration" UI needs no signature change, but nothing in the
/// frontend exposes that yet.
const MUTE_MARK_SECS: f64 = 3.0;

/// One skip-category mark waiting for its closing press -- the category and
/// start position/title recorded at `creation_start_skip_mark`, completed
/// into a `Cue` once `creation_end_skip_mark` supplies the end position.
struct PendingSkip {
    category: String,
    /// Normalized (see `filter::normalize_title`) title in effect when the
    /// mark started -- `creation_end_skip_mark` checks the current title
    /// still matches this before completing the cue, so a mark can't
    /// silently span a title change.
    title: String,
    start: f64,
}

#[derive(Default)]
pub(crate) struct CreationState {
    draft: Option<FilterList>,
    draft_path: Option<PathBuf>,
    pending_skip: Option<PendingSkip>,
}

#[derive(serde::Serialize)]
pub struct DraftSummary {
    pub path: String,
    pub media_count: usize,
}

/// One freshly recorded cue, handed back to the frontend so it can update
/// its cue table without a separate `creation_list_cues` round trip.
#[derive(serde::Serialize)]
pub struct CueMarkResult {
    pub title: String,
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub category: String,
    pub action: CueAction,
}

/// One cue as shown in the creation-mode editing table -- unlike
/// `control::CueStatus`, there's no `enabled`: auto-filter's per-cue on/off
/// toggle doesn't apply to a draft that isn't wired up for playback.
#[derive(serde::Serialize)]
pub struct CreationCue {
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub action: CueAction,
    pub category: String,
}

fn cue_result(entry_title: &str, index: usize, cue: &Cue) -> CueMarkResult {
    CueMarkResult {
        title: entry_title.to_string(),
        index,
        start: cue.start,
        end: cue.end,
        category: cue.category.clone(),
        action: cue.action,
    }
}

/// Best-effort autosave to `draft_path` after every mutation -- a movie-long
/// recording session shouldn't lose marks to a crash between them. Mirrors
/// `control::load_filter_file`'s handling of `filter::save_filter_path`: a
/// failure to persist doesn't undo the in-memory mutation, just gets logged.
fn autosave(creation: &CreationState) {
    let (Some(draft), Some(path)) = (creation.draft.as_ref(), creation.draft_path.as_ref()) else {
        return;
    };
    if let Err(e) = draft.save(path) {
        eprintln!("[creation] failed to autosave {}: {e}", path.display());
    }
}

/// Reads the live session's current title/position, refreshing first so a
/// mark lands on the true current position rather than one extrapolated
/// since the last periodic poll -- same reasoning as
/// `control::control_playback_status`'s own refresh-then-read. Never trusts
/// a client-supplied title/position, matching `control::apply_filter`'s
/// fresh read.
async fn live_title_and_position(guard: &mut ControlState) -> Result<(String, f64)> {
    let live = guard.live_mut().context("no live transport paired (pair MRP or AirPlay to record marks)")?;
    // Best-effort: an occasional refresh failure shouldn't block a mark --
    // fall back to the still-good extrapolated position, same tolerance
    // control_playback_status already has.
    let _ = live.refresh_position().await;
    let playback = live.playback();
    let title = playback.title().context("nothing is currently playing")?.to_string();
    let position = playback.position_now().context("no current playback position")?;
    Ok((title, position))
}

/// Starts a brand-new, empty draft at `path` (chosen via
/// `@tauri-apps/plugin-dialog`'s `save()` picker on the frontend) and writes
/// it immediately so the file exists at the chosen destination right away --
/// the save dialog only picks a path, it doesn't create anything.
#[tauri::command]
pub async fn creation_new_draft(state: State<'_, ControlStateHandle>, path: String) -> Result<DraftSummary, String> {
    let path_buf = PathBuf::from(&path);
    let draft = FilterList::default();
    draft.save(&path_buf).map_err(|e| describe(&e))?;

    let mut guard = state.lock().await;
    guard.creation.pending_skip = None;
    guard.creation.draft_path = Some(path_buf);
    guard.creation.draft = Some(draft);

    Ok(DraftSummary { path, media_count: 0 })
}

/// Opens an existing filter file (chosen via the same `open()` picker
/// `load_filter_file` uses) to keep recording marks into it.
#[tauri::command]
pub async fn creation_open_draft(state: State<'_, ControlStateHandle>, path: String) -> Result<DraftSummary, String> {
    let path_buf = PathBuf::from(&path);
    let draft = FilterList::load(&path_buf).map_err(|e| describe(&e))?;
    let media_count = draft.media.len();

    let mut guard = state.lock().await;
    guard.creation.pending_skip = None;
    guard.creation.draft_path = Some(path_buf);
    guard.creation.draft = Some(draft);

    Ok(DraftSummary { path, media_count })
}

/// Drops the in-memory draft/pending-mark state only -- the file itself is
/// already up to date on disk (every mutation autosaves), so nothing is
/// lost; the user re-opens it via `creation_open_draft` to keep going.
#[tauri::command]
pub async fn creation_close_draft(state: State<'_, ControlStateHandle>) -> Result<(), String> {
    state.lock().await.creation = CreationState::default();
    Ok(())
}

/// Records a brief mute mark ("language" by default, but any category name
/// works) at the current live playback position.
#[tauri::command]
pub async fn creation_mark_mute(
    state: State<'_, ControlStateHandle>,
    category: String,
    duration_secs: Option<f64>,
) -> Result<CueMarkResult, String> {
    let mut guard = state.lock().await;
    let (title, position) = live_title_and_position(&mut guard).await.map_err(|e| describe(&e))?;
    let duration = duration_secs.unwrap_or(MUTE_MARK_SECS);

    let draft = guard.creation.draft.as_mut().ok_or_else(|| "no draft filter file open -- start or open one first".to_string())?;
    let cue = Cue { start: position, end: position + duration, action: CueAction::Mute, category };
    let index = draft.add_cue(&title, cue.clone()).map_err(|e| describe(&e))?;
    autosave(&guard.creation);

    Ok(cue_result(&title, index, &cue))
}

/// Marks the start of a skip-scene cue. Only one skip mark can be pending at
/// a time (app-wide, not per-category) -- two overlapping in-progress marks
/// could never both resolve into non-overlapping cues for the same video
/// anyway, so this is rejected up front rather than at completion.
#[tauri::command]
pub async fn creation_start_skip_mark(state: State<'_, ControlStateHandle>, category: String) -> Result<(), String> {
    let mut guard = state.lock().await;
    if let Some(pending) = &guard.creation.pending_skip {
        return Err(format!("a skip mark is already pending for {:?} -- end or cancel it first", pending.category));
    }
    let (title, position) = live_title_and_position(&mut guard).await.map_err(|e| describe(&e))?;
    guard.creation.pending_skip = Some(PendingSkip { category, title: filter::normalize_title(&title), start: position });
    Ok(())
}

/// Completes the pending skip mark at the current position.
#[tauri::command]
pub async fn creation_end_skip_mark(state: State<'_, ControlStateHandle>) -> Result<CueMarkResult, String> {
    let mut guard = state.lock().await;
    let Some(pending) = guard.creation.pending_skip.take() else {
        return Err("no skip mark is pending".to_string());
    };

    let (title, position) = match live_title_and_position(&mut guard).await {
        Ok(t) => t,
        // Can't even read the current position -- nothing to retry against,
        // so the pending mark stays cleared.
        Err(e) => return Err(describe(&e)),
    };
    if filter::normalize_title(&title) != pending.title {
        return Err("title changed since the mark started -- cancelled, mark again".to_string());
    }

    let draft = guard.creation.draft.as_mut().ok_or_else(|| "no draft filter file open".to_string())?;
    let cue = Cue { start: pending.start, end: position, action: CueAction::Skip, category: pending.category.clone() };
    match draft.add_cue(&title, cue.clone()) {
        Ok(index) => {
            autosave(&guard.creation);
            Ok(cue_result(&title, index, &cue))
        }
        // Restore the pending mark so the user can retry ending it a moment
        // later instead of losing the start.
        Err(e) => {
            guard.creation.pending_skip = Some(pending);
            Err(describe(&e))
        }
    }
}

/// Discards the pending skip mark without creating a cue.
#[tauri::command]
pub async fn creation_cancel_skip_mark(state: State<'_, ControlStateHandle>) -> Result<(), String> {
    state.lock().await.creation.pending_skip = None;
    Ok(())
}

/// Corrects a previously recorded cue's timing -- the timing-adjustment
/// mode's edit action.
#[tauri::command]
pub async fn creation_update_cue(
    state: State<'_, ControlStateHandle>,
    title: String,
    index: usize,
    start: f64,
    end: f64,
) -> Result<(), String> {
    let mut guard = state.lock().await;
    let draft = guard.creation.draft.as_mut().ok_or_else(|| "no draft filter file open".to_string())?;
    draft.update_cue(&title, index, start, end).map_err(|e| describe(&e))?;
    autosave(&guard.creation);
    Ok(())
}

/// Removes a mis-marked cue outright.
#[tauri::command]
pub async fn creation_delete_cue(state: State<'_, ControlStateHandle>, title: String, index: usize) -> Result<(), String> {
    let mut guard = state.lock().await;
    let draft = guard.creation.draft.as_mut().ok_or_else(|| "no draft filter file open".to_string())?;
    draft.delete_cue(&title, index).map_err(|e| describe(&e))?;
    autosave(&guard.creation);
    Ok(())
}

/// Lists the current draft's cues for `title`, for the timing-adjustment
/// table. Pure draft lookup -- doesn't touch `live`, since the frontend
/// already has a fresh title from its existing 1s `control_playback_status`
/// poll, so no separate poller is needed here.
#[tauri::command]
pub async fn creation_list_cues(state: State<'_, ControlStateHandle>, title: String) -> Result<Vec<CreationCue>, String> {
    let guard = state.lock().await;
    let Some(draft) = guard.creation.draft.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(entry) = draft.find_entry(&title) else {
        return Ok(Vec::new());
    };
    Ok(entry
        .cues
        .iter()
        .enumerate()
        .map(|(index, cue)| CreationCue { index, start: cue.start, end: cue.end, action: cue.action, category: cue.category.clone() })
        .collect())
}

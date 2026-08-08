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

use std::sync::Arc;

use tauri::State;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use appletv::companion::CompanionSession;
use appletv::{storage, LiveSession};

use crate::DISPLAY_NAME;

/// How many `control_playback_status` polls between active
/// `refresh_position()` calls. Matches the CLI's
/// `POSITION_REFRESH_EVERY_TICKS`: most polls just extrapolate locally from
/// the last known position (see `PlaybackState::position_now`), with an
/// occasional active re-request to close the gap apps don't always push a
/// fresh value for, without hammering the device every single poll.
const REFRESH_EVERY_POLLS: u32 = 3;

#[derive(Default)]
pub struct ControlState {
    session: Option<CompanionSession>,
    live: Option<LiveSession>,
    poll_count: u32,
}

pub type ControlStateHandle = Arc<Mutex<ControlState>>;

fn describe(e: &anyhow::Error) -> String {
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

    *state.lock().await = ControlState { session: Some(session), live, poll_count: 0 };
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

#[derive(serde::Serialize)]
pub struct PlaybackStatus {
    pub title: Option<String>,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub playback_state: String,
}

/// One snapshot of the current now-playing state, for the frontend to poll
/// on an interval (e.g. every second). Returns `Ok(None)` rather than an
/// error when there's no live transport -- that's an expected, steady
/// state (MRP/AirPlay weren't paired), not a failure.
///
/// Between active refreshes, `position`/`playback_state` are whatever the
/// last known-good snapshot extrapolates to locally -- accurate only if
/// nothing has changed the *real* position since then. A skip through this
/// app, or a pause/seek from the physical remote, invalidates that
/// immediately but won't be reflected here until the next active refresh
/// (throttled to every `REFRESH_EVERY_POLLS`th call) or a push from the
/// device. `force: true` bypasses the throttle for an on-demand, always-
/// fresh read -- used right after `control_skip`, and available to the
/// frontend as a manual "refresh now" action.
#[tauri::command]
pub async fn control_playback_status(state: State<'_, ControlStateHandle>, force: bool) -> Result<Option<PlaybackStatus>, String> {
    let mut guard = state.lock().await;
    let ControlState { live, poll_count, .. } = &mut *guard;
    let Some(live) = live.as_mut() else {
        return Ok(None);
    };

    *poll_count += 1;
    if force || *poll_count % REFRESH_EVERY_POLLS == 0 {
        // Best-effort: an occasional refresh failure shouldn't hide the
        // still-good extrapolated position below.
        let _ = live.refresh_position().await;
    }

    let playback = live.playback();
    Ok(Some(PlaybackStatus {
        title: playback.title().map(str::to_string),
        position: playback.position_now(),
        duration: playback.duration(),
        playback_state: format!("{:?}", playback.playback_state()),
    }))
}

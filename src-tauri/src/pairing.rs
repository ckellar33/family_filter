//! Tauri commands for the discover-and-pair flow, wiring `appletv`'s
//! session-level pairing functions (`libs/appletv/src/session.rs`) into the
//! GUI. Mirrors what `libs/appletv-cli`'s `pair_flow()` does over stdin/
//! stdout, but PIN entry can't block a thread on stdin here -- the on-screen
//! PIN triggers a `pin-requested` event to the frontend, and the frontend
//! answers it later via the `submit_pin` command instead.
//!
//! Companion pairing is required (it's what `control_flow` needs for mute/
//! skip); MRP and AirPlay are optional, each their own pairing ceremony
//! against their own discovered device, needed only for live playback
//! position (see `appletv::mrp::tunnel`). The frontend decides whether to
//! attempt them; this module just exposes each step.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, State};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

use appletv::{mdns, storage};

use crate::DISPLAY_NAME;

/// Owned, `Serialize`-able mirror of `appletv::mdns::Discovered` -- Tauri
/// command return types need to derive `Serialize`.
#[derive(serde::Serialize, Clone)]
pub struct DeviceDto {
    pub host: String,
    pub port: u16,
}

impl From<mdns::Discovered> for DeviceDto {
    fn from(d: mdns::Discovered) -> Self {
        DeviceDto { host: d.host, port: d.port }
    }
}

/// Holds each protocol's pairing result as it completes, plus any PIN
/// requests currently awaiting an answer from the frontend. Lives for the
/// duration of one pairing session; `finish_pairing` drains and resets it.
#[derive(Default)]
pub struct PairingState {
    companion: Option<storage::Pairing>,
    mrp: Option<storage::Pairing>,
    airplay: Option<storage::Pairing>,
    pending_pins: HashMap<String, oneshot::Sender<String>>,
}

pub type PairingStateHandle = Arc<Mutex<PairingState>>;

/// Flattens an `anyhow::Error`'s full causal chain into one string, since
/// Tauri command errors only carry a `String` to the frontend -- otherwise
/// only the outermost `.context()` message would reach the UI, hiding the
/// actual cause (a timeout, a wrong HTTP code, a decode failure).
fn describe(e: &anyhow::Error) -> String {
    appletv::error_chain(e).join(": ")
}

/// The `get_pin` callback passed to `appletv::pair_{companion,mrp,airplay}`.
/// Those only call it *after* triggering the on-screen code, so emitting
/// `pin-requested` here is exactly when the frontend should show the input.
/// Registers a one-shot slot under `protocol` and waits for `submit_pin` to
/// fill it; if the frontend never answers (e.g. the window closes), the
/// sender is simply dropped and this resolves to an empty string, which the
/// pairing ceremony will then reject as a wrong PIN.
async fn wait_for_pin(app: AppHandle, state: PairingStateHandle, protocol: &'static str) -> String {
    let (tx, rx) = oneshot::channel();
    {
        let mut guard = state.lock().unwrap();
        guard.pending_pins.insert(protocol.to_string(), tx);
    }
    let _ = app.emit("pin-requested", protocol);
    rx.await.unwrap_or_default()
}

/// Discovers devices for one protocol's mDNS service
/// (`_companion-link._tcp`, `_mediaremotetv._tcp`, or `_airplay._tcp`).
/// Same 7s timeout the CLI uses. Each protocol enumerates hosts
/// independently -- even for the same physical device, the advertised host
/// string usually differs per service -- so the frontend re-discovers and
/// re-prompts device selection for each pairing step, same as the CLI does.
///
/// mDNS commonly re-announces the same service across multiple interfaces
/// or records, so the same host:port can legitimately show up more than
/// once in one scan -- the CLI just prints every entry positionally and
/// doesn't care, but the frontend keys its device list by `host:port`, and
/// a duplicate key crashes Svelte's keyed `{#each}` render (see the
/// `each_key_duplicate` bug this was fixing). Dedupe here so callers never
/// see redundant entries in the first place.
#[tauri::command]
pub async fn discover_devices(protocol: String) -> Result<Vec<DeviceDto>, String> {
    let timeout = Duration::from_secs(7);
    let devices = match protocol.as_str() {
        "companion" => mdns::find_companion(timeout).await,
        "mrp" => mdns::find_mrp(timeout).await,
        "airplay" => mdns::find_airplay(timeout).await,
        other => return Err(format!("unknown protocol: {other}")),
    };
    devices
        .map(|devices| {
            let mut seen = HashSet::new();
            devices
                .into_iter()
                .map(DeviceDto::from)
                .filter(|d| seen.insert((d.host.clone(), d.port)))
                .collect::<Vec<_>>()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pair_companion(
    app: AppHandle,
    state: State<'_, PairingStateHandle>,
    host: String,
    port: u16,
) -> Result<(), String> {
    let state = state.inner().clone();
    let mut stream = TcpStream::connect(format!("{host}:{port}"))
        .await
        .map_err(|e| format!("failed to connect: {e}"))?;

    let pairing_id = appletv::random_pairing_id();
    let creds = appletv::pair_companion(&mut stream, &pairing_id, DISPLAY_NAME, {
        let app = app.clone();
        let state = state.clone();
        move || wait_for_pin(app, state, "companion")
    })
    .await
    .map_err(|e| describe(&e))?;

    state.lock().unwrap().companion = Some(storage::Pairing { host, port, creds });
    Ok(())
}

#[tauri::command]
pub async fn pair_mrp(
    app: AppHandle,
    state: State<'_, PairingStateHandle>,
    host: String,
    port: u16,
) -> Result<(), String> {
    let state = state.inner().clone();
    let mrp_stream = TcpStream::connect(format!("{host}:{port}"))
        .await
        .map_err(|e| format!("failed to connect: {e}"))?;
    let mut conn = appletv::mrp::connection::MrpConnection::new(mrp_stream);

    let pairing_id = appletv::random_pairing_id();
    let creds = appletv::pair_mrp(&mut conn, &pairing_id, DISPLAY_NAME, {
        let app = app.clone();
        let state = state.clone();
        move || wait_for_pin(app, state, "mrp")
    })
    .await
    .map_err(|e| describe(&e))?;

    state.lock().unwrap().mrp = Some(storage::Pairing { host, port, creds });
    Ok(())
}

#[tauri::command]
pub async fn pair_airplay(
    app: AppHandle,
    state: State<'_, PairingStateHandle>,
    host: String,
    port: u16,
) -> Result<(), String> {
    let state = state.inner().clone();
    let mut conn = appletv::airplay::rtsp::RtspConnection::connect(&host, port)
        .await
        .map_err(|e| describe(&e))?;

    let pairing_id = appletv::random_pairing_id();
    let creds = appletv::pair_airplay(&mut conn, &pairing_id, DISPLAY_NAME, {
        let app = app.clone();
        let state = state.clone();
        move || wait_for_pin(app, state, "airplay")
    })
    .await
    .map_err(|e| describe(&e))?;

    state.lock().unwrap().airplay = Some(storage::Pairing { host, port, creds });
    Ok(())
}

/// Answers a pending `pin-requested` event for `protocol` (one of
/// "companion" / "mrp" / "airplay"). Errors if there's no pairing currently
/// waiting on a PIN for that protocol -- e.g. it already completed, or was
/// never started.
#[tauri::command]
pub fn submit_pin(state: State<'_, PairingStateHandle>, protocol: String, pin: String) -> Result<(), String> {
    let sender = state.lock().unwrap().pending_pins.remove(&protocol);
    match sender {
        Some(tx) => tx.send(pin).map_err(|_| "pairing was cancelled before the PIN was submitted".to_string()),
        None => Err(format!("no pairing is currently waiting on a {protocol} PIN")),
    }
}

/// Writes whatever pairing results have accumulated so far to
/// `pairing.store` and resets the session. Companion is required (matches
/// `storage::save_pairing`'s signature); MRP/AirPlay are included only if
/// that step was attempted and succeeded.
#[tauri::command]
pub fn finish_pairing(state: State<'_, PairingStateHandle>) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    let companion = guard
        .companion
        .take()
        .ok_or_else(|| "Companion pairing must succeed before saving".to_string())?;
    let mrp = guard.mrp.take();
    let airplay = guard.airplay.take();
    drop(guard);

    storage::save_pairing(&companion, mrp.as_ref(), airplay.as_ref()).map_err(|e| describe(&e))
}

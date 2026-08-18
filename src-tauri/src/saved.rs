//! Listing, verifying, renaming, and deleting saved devices (the
//! `pairings/` multi-device store), mirroring `libs/appletv-cli`'s "Verify
//! saved device" menu option but for however many devices the GUI has
//! paired. Deliberately scoped to detect + verify/manage only for now --
//! driving an actual control session (mute/skip/live position) off a saved
//! pairing is `control::start_control_session`.

use std::time::Duration;

use appletv::storage;

/// Same reasoning as `control::CONNECT_TIMEOUT` -- an unreachable host would
/// otherwise hang `TcpStream::connect` far longer than this, leaving
/// "Verify" looking stuck rather than failing with a clear error.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

/// Lightweight, `Serialize`-able summary of one saved device -- the frontend
/// gets id/name/host/port/which-protocols-are-present, never the raw
/// credentials (private keys, accessory identifiers).
#[derive(serde::Serialize)]
pub struct SavedDeviceInfo {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub has_mrp: bool,
    pub has_airplay: bool,
}

fn describe(e: &anyhow::Error) -> String {
    appletv::error_chain(e).join(": ")
}

/// Every saved device (see `storage::list_devices`), for the Devices
/// screen's chooser. Empty (rather than an error) both when nothing's ever
/// been paired and when the store exists but happens to be unreadable --
/// either way there's nothing to offer, so the frontend just falls through
/// to the pairing wizard.
#[tauri::command]
pub fn list_saved_devices() -> Vec<SavedDeviceInfo> {
    storage::list_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| SavedDeviceInfo {
            id: entry.id,
            name: entry.device.name,
            host: entry.device.companion.host,
            port: entry.device.companion.port,
            has_mrp: entry.device.mrp.is_some(),
            has_airplay: entry.device.airplay.is_some(),
        })
        .collect()
}

/// The device id `control::start_control_session` last connected to, if
/// any -- lets the Devices screen auto-reconnect to it on launch while
/// still offering every other saved device as a "switch device" option, per
/// `storage::LAST_DEVICE_STORE`'s doc.
#[tauri::command]
pub fn last_saved_device_id() -> Option<String> {
    storage::load_last_device_id()
}

/// Runs Pair-Verify against a saved device's Companion pairing -- same check
/// `libs/appletv-cli`'s "Verify saved device" option performs -- to confirm
/// the stored credentials are still accepted without redoing the full
/// pairing ceremony or disturbing whatever control session (if any) is
/// currently active against a different device.
#[tauri::command]
pub async fn verify_saved_pairing(id: String) -> Result<(), String> {
    let mut saved = storage::load_device(&id).map_err(|e| describe(&e))?;

    let (mut stream, port) = appletv::connect_companion(&saved.companion.host, saved.companion.port, CONNECT_TIMEOUT)
        .await
        .map_err(|e| describe(&e))?;
    if port != saved.companion.port {
        // See the matching comment in control.rs::start_control_session --
        // same stale-port recovery, persisted here too so this button
        // actually fixes future auto-connects, not just this one check.
        saved.companion.port = port;
        if let Err(e) = storage::update_device(&id, &saved.name, &saved.companion, saved.mrp.as_ref(), saved.airplay.as_ref()) {
            eprintln!("[pairing] failed to persist refreshed Companion port: {}", describe(&e));
        }
    }

    appletv::hap_pair::pair_verify(&mut stream, &saved.companion.creds)
        .await
        .map(|_keys| ())
        .map_err(|e| describe(&e))
}

/// Renames a saved device in place -- credentials and id are untouched.
#[tauri::command]
pub fn rename_saved_device(id: String, name: String) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name can't be empty".to_string());
    }
    storage::rename_device(&id, name).map_err(|e| describe(&e))
}

/// Deletes a saved device outright. Doesn't touch a control session already
/// running against it -- that session simply won't be reconnectable next
/// launch, same as deleting the file out from under any other saved-state
/// screen in this app.
#[tauri::command]
pub fn delete_saved_device(id: String) -> Result<(), String> {
    storage::delete_device(&id).map_err(|e| describe(&e))
}

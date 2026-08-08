//! Detecting and verifying a previously saved pairing (`pairing.store`),
//! mirroring `libs/appletv-cli`'s "Verify saved device" menu option.
//! Deliberately scoped to detect + verify only for now -- driving an actual
//! control session (mute/skip/live position) off a saved pairing is a
//! separate follow-up.

use appletv::storage;
use tokio::net::TcpStream;

/// Lightweight, `Serialize`-able summary of a saved pairing -- the frontend
/// gets host/port/which-protocols-are-present, never the raw credentials
/// (private keys, accessory identifiers).
#[derive(serde::Serialize)]
pub struct SavedPairingInfo {
    pub host: String,
    pub port: u16,
    pub has_mrp: bool,
    pub has_airplay: bool,
}

fn describe(e: &anyhow::Error) -> String {
    appletv::error_chain(e).join(": ")
}

/// Checks for a `pairing.store` and returns a summary if one exists and
/// parses successfully. `Ok(None)` covers both "no file" and "file exists
/// but isn't a valid saved pairing" -- either way there's nothing to offer
/// verifying, so the frontend should fall through to the normal pairing
/// wizard.
#[tauri::command]
pub fn check_saved_pairing() -> Option<SavedPairingInfo> {
    let saved = storage::load_pairing().ok().flatten()?;
    Some(SavedPairingInfo {
        host: saved.companion.host,
        port: saved.companion.port,
        has_mrp: saved.mrp.is_some(),
        has_airplay: saved.airplay.is_some(),
    })
}

/// Runs Pair-Verify against the saved Companion pairing -- same check
/// `libs/appletv-cli`'s "Verify saved device" option performs -- to confirm
/// the stored credentials are still accepted without redoing the full
/// pairing ceremony.
#[tauri::command]
pub async fn verify_saved_pairing() -> Result<(), String> {
    let saved = storage::load_pairing()
        .map_err(|e| describe(&e))?
        .ok_or_else(|| "No saved pairing found".to_string())?;

    let mut stream = TcpStream::connect(format!("{}:{}", saved.companion.host, saved.companion.port))
        .await
        .map_err(|e| format!("failed to connect: {e}"))?;

    appletv::hap_pair::pair_verify(&mut stream, &saved.companion.creds)
        .await
        .map(|_keys| ())
        .map_err(|e| describe(&e))
}

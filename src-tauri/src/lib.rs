mod control;
mod pairing;
mod saved;

use control::ControlStateHandle;
use pairing::{PairingState, PairingStateHandle};

/// Display name advertised to the Apple TV during pairing and control
/// sessions -- same identity the CLI (`libs/appletv-cli`) uses.
pub(crate) const DISPLAY_NAME: &str = "family-filter";

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage::<PairingStateHandle>(std::sync::Arc::new(std::sync::Mutex::new(PairingState::default())))
        .manage::<ControlStateHandle>(Default::default())
        .invoke_handler(tauri::generate_handler![
            greet,
            pairing::discover_devices,
            pairing::pair_companion,
            pairing::pair_mrp,
            pairing::pair_airplay,
            pairing::submit_pin,
            pairing::finish_pairing,
            saved::check_saved_pairing,
            saved::verify_saved_pairing,
            control::start_control_session,
            control::control_mute,
            control::control_unmute,
            control::control_skip,
            control::control_playback_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

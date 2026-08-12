mod control;
mod creation;
mod filter;
mod library;
mod metadata;
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
        .plugin(tauri_plugin_dialog::init())
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
            control::control_button,
            control::control_playback_status,
            control::load_filter_file,
            control::check_saved_filter_file,
            control::set_filter_enabled,
            control::set_filter_category_enabled,
            control::set_filter_cue_enabled,
            control::add_filter_files,
            control::add_filter_directory,
            control::list_filter_tiles,
            control::list_services_for_title,
            control::select_filter_tile,
            creation::creation_new_draft,
            creation::creation_open_draft,
            creation::creation_close_draft,
            creation::creation_mark_mute,
            creation::creation_start_skip_mark,
            creation::creation_end_skip_mark,
            creation::creation_cancel_skip_mark,
            creation::creation_update_cue,
            creation::creation_delete_cue,
            creation::creation_list_cues,
            creation::creation_set_service,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

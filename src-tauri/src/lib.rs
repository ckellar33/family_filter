mod pairing;

use pairing::{PairingState, PairingStateHandle};

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
        .invoke_handler(tauri::generate_handler![
            greet,
            pairing::discover_devices,
            pairing::pair_companion,
            pairing::pair_mrp,
            pairing::pair_airplay,
            pairing::submit_pin,
            pairing::finish_pairing,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

mod clock_sync;
mod control;
mod creation;
mod filter;
mod library;
mod metadata;
mod online;
mod pairing;
mod paths;
mod saved;

use tauri::{Emitter, Manager};

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
    // Installs `ring` as the process-wide default rustls crypto provider --
    // needed before `online.rs`'s direct Postgres connection can build a
    // `rustls::ClientConfig` (its `builder()` panics without one already
    // installed). Best-effort: if reqwest's own rustls-tls setup already
    // installed a (possibly different) default first, this just fails
    // harmlessly and that one is used instead -- either backend is fine for
    // a plain TLS connection to Neon, so there's nothing to actually handle
    // in the `Err` case.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage::<PairingStateHandle>(std::sync::Arc::new(std::sync::Mutex::new(
            PairingState::default(),
        )))
        .manage::<ControlStateHandle>(Default::default())
        .setup(|app| {
            // Point every sidecar `*.store` file (pairing/device credentials,
            // the saved filter path + toggle, the filter library, the TMDB
            // key) at the OS/platform-appropriate app data directory instead
            // of leaving them CWD-relative -- see `paths.rs`'s doc for why
            // that matters on iOS specifically. Must happen before any
            // command that reads/writes one of those stores runs, so this
            // is the very first thing `setup` does.
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("no app data directory available");
            std::fs::create_dir_all(&data_dir).expect("failed to create app data directory");
            paths::set_base_dir(data_dir.clone());
            appletv::storage::set_base_dir(data_dir);

            // Backgrounded, not awaited -- see `clock_sync::sync_clock_offset`'s
            // doc for why app startup shouldn't block on it. Every playback
            // position calculation self-corrects the moment this resolves,
            // with no restart or reconnect needed.
            tauri::async_runtime::spawn(clock_sync::sync_clock_offset());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            pairing::discover_devices,
            pairing::pair_companion,
            pairing::pair_mrp,
            pairing::pair_airplay,
            pairing::submit_pin,
            pairing::finish_pairing,
            saved::list_saved_devices,
            saved::last_saved_device_id,
            saved::verify_saved_pairing,
            saved::rename_saved_device,
            saved::delete_saved_device,
            control::start_control_session,
            control::control_mute,
            control::control_unmute,
            control::control_skip,
            control::control_seek,
            control::control_button,
            control::control_playback_status,
            control::load_filter_file,
            control::check_saved_filter_file,
            control::set_filter_enabled,
            control::set_filter_category_enabled,
            control::set_filter_cue_enabled,
            control::update_filter_cue,
            control::add_filter_cue,
            control::delete_filter_cue,
            control::publish_filter_entry_online,
            control::add_filter_files,
            control::supports_folder_import,
            control::add_filter_directory,
            control::supports_save_location_picker,
            control::list_filter_tiles,
            control::list_services_for_title,
            control::select_filter_tile,
            control::delete_filter_file,
            online::list_online_filters,
            online::preview_online_entry,
            online::download_online_filter,
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
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app_handle, event| {
            // iOS suspends/reclaims the raw TCP sockets `control::ControlState`
            // holds (Companion, and MRP/AirPlay if paired) while the app is
            // backgrounded -- swiping up to the home screen doesn't kill the
            // process, but the sockets don't survive it, and nothing else
            // would ever notice they'd gone dead. `RunEvent::Resumed` maps to
            // `applicationWillEnterForeground` on iOS (and Android's onResume),
            // so this is the one reliable moment to tell the frontend to
            // reconnect -- it already owns that flow (`openControls`, the same
            // call a manual device switch makes) via the `app-resumed` event.
            // Gated to mobile: on desktop `Resumed` is an event-loop startup
            // signal, not a background/foreground transition, and re-running
            // the reconnect flow on every window focus would be both wrong and
            // wasteful there.
            if matches!(event, tauri::RunEvent::Resumed)
                && cfg!(any(target_os = "ios", target_os = "android"))
            {
                let _ = app_handle.emit("app-resumed", ());
            }
        });
}

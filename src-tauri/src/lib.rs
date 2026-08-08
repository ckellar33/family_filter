// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// One serializable device entry for the frontend. Mirrors
/// `appletv::mdns::Discovered` but as an owned, `serde`-friendly shape --
/// Tauri commands need their return types to derive `Serialize`.
#[derive(serde::Serialize)]
struct Device {
    host: String,
    port: u16,
}

/// Sanity-check command wiring the `appletv` library (from the
/// `libs/appletv` submodule) into the Tauri backend: discovers
/// `_companion-link._tcp` Apple TVs on the LAN via mDNS, same call the CLI
/// (`libs/appletv-cli`) makes for its "Discover and pair a device" flow.
#[tauri::command]
async fn discover_apple_tvs() -> Result<Vec<Device>, String> {
    appletv::mdns::find_companion(std::time::Duration::from_secs(7))
        .await
        .map(|devices| {
            devices
                .into_iter()
                .map(|d| Device { host: d.host, port: d.port })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet, discover_apple_tvs])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

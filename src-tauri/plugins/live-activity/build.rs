// No frontend-callable commands: the app's Rust side drives this plugin
// directly (see `LiveActivity` in src/lib.rs), so there's nothing to grant
// in a capability file.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}

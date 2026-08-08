//! Apple MRP (Media Remote Protocol) client — the protocol that actually
//! carries now-playing metadata (title, duration, elapsed time, playback
//! rate), unlike Companion (`companion.rs`), which only ever reports which
//! transport buttons are enabled. See the plan notes for how this was
//! reverse-verified against pyatv's source.

pub mod connection;
pub mod messages;
pub mod pairing;
pub mod playback;
pub mod session;
pub mod tunnel;

pub use session::MrpSession;
pub use tunnel::TunneledMrpSession;

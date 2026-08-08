//! Companion protocol client — the encrypted media-control session
//! established after Pair-Verify (`hap_pair::pair_verify`). Only reports
//! which transport buttons are enabled, unlike MRP (`mrp/`), which carries
//! actual now-playing metadata (title, duration, elapsed time).

pub mod connection;
pub mod messages;
pub mod session;

pub use session::CompanionSession;

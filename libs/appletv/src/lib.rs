//! AppleTV Companion / MRP / AirPlay pairing and control protocol library.
//!
//! `mdns`, `crypto`, `hap_pair`, `srp`, `storage`, `companion`, `mrp`, and
//! `airplay` are the low-level protocol pieces (no I/O beyond the sockets
//! they're handed). `session` is the higher-level orchestration layer that
//! any front end (CLI, GUI, ...) can drive: it sequences pairing ceremonies
//! and live-playback sessions correctly (e.g. "trigger the on-screen PIN
//! before asking for it") without doing any of its own terminal I/O — PINs
//! come in through a caller-supplied async callback, and progress/fallback
//! notices go through the `log` crate rather than `println!`.

pub mod airplay;
pub mod companion;
pub mod crypto;
pub mod hap_pair;
pub mod mdns;
pub mod mrp;
pub mod session;
pub mod srp;
pub mod storage;

pub use session::{connect_live_session, error_chain, pair_airplay, pair_companion, pair_mrp, random_pairing_id, LiveSession};

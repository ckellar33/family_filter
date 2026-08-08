//! AirPlay 2 client — required as a transport for MRP on tvOS 15+, which no
//! longer advertises MRP as its own discoverable service (see
//! `mrp/tunnel.rs` and the plan notes). Only the "remote control" tunnel
//! subset is implemented: enough to Pair-Setup/Pair-Verify, open the
//! event/data channels, and carry MRP `ProtocolMessage` traffic — no actual
//! audio/video streaming.

pub mod channels;
pub mod hap_channel;
pub mod pairing;
pub mod rtsp;
pub mod session;

pub use session::Ap2Session;

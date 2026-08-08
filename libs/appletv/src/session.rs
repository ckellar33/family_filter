//! High-level orchestration: pairing ceremonies and live-playback session
//! selection, shared by any front end. Interactive concerns (discovering
//! devices and letting a human pick one, printing a menu) stay with the
//! caller; a PIN is obtained through an async callback so the "trigger the
//! on-screen code, *then* ask for it" ordering is enforced here instead of
//! depending on every caller getting it right. Non-blocking progress /
//! fallback notices go through the `log` crate rather than `println!`.

use std::future::Future;

use anyhow::Result;
use log::{info, warn};
use tokio::net::TcpStream;

use crate::airplay::{self, pairing as airplay_pairing, rtsp::RtspConnection};
use crate::hap_pair::{self, PairingResult};
use crate::mrp::{self, connection::MrpConnection};
use crate::storage::SavedDevice;

/// pyatv uses a random UUID as the controller pairing identifier; a fresh
/// one is generated per protocol (Companion, MRP, and AirPlay are each
/// independent pairings).
pub fn random_pairing_id() -> String {
    let mut b = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
    // RFC 4122 version 4 / variant 1
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// Full causal chain of an anyhow error, outermost `.context()` message
/// first — the underlying cause (a timeout, a wrong HTTP code, a decode
/// failure) is usually where the actual bug is, not just the outermost
/// wrapping. Callers decide how to present it (multi-line for a CLI,
/// single-line for a log record, ...).
pub fn error_chain(e: &anyhow::Error) -> Vec<String> {
    e.chain().map(|cause| cause.to_string()).collect()
}

fn chain_summary(e: &anyhow::Error) -> String {
    error_chain(e).join("; caused by: ")
}

/// Companion Pair-Setup (M1/M3/M5). `get_pin` is only invoked after M1 has
/// triggered the on-screen code on the Apple TV.
pub async fn pair_companion<F, Fut>(
    stream: &mut TcpStream,
    pairing_id: &str,
    display_name: &str,
    get_pin: F,
) -> Result<PairingResult>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = String>,
{
    let (salt, public_key) = hap_pair::initial_pair_m1(stream)
        .await
        .map_err(|_| anyhow::Error::msg("failed to send initial pair request"))?;
    let pin = get_pin().await;
    let session_key = hap_pair::pair_m3(stream, &pin, &salt, &public_key).await?;
    hap_pair::pair_m5(stream, pairing_id, &session_key, display_name).await
}

/// MRP Pair-Setup, including the DeviceInfo handshake that triggers the
/// on-screen code. Standalone MRP only works on older tvOS / the local fake
/// device (tvOS 15+ stopped advertising it) — see [`pair_airplay`] for the
/// modern path.
pub async fn pair_mrp<F, Fut>(
    conn: &mut MrpConnection,
    pairing_id: &str,
    display_name: &str,
    get_pin: F,
) -> Result<PairingResult>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = String>,
{
    mrp::pairing::device_info_handshake(conn, pairing_id, display_name).await?;
    let (salt, public_key) = mrp::pairing::pair_setup_m1(conn).await?;
    let pin = get_pin().await;
    let session_key = mrp::pairing::pair_setup_m3(conn, &pin, &salt, &public_key).await?;
    mrp::pairing::pair_setup_m5(conn, pairing_id, &session_key, display_name).await
}

/// AirPlay Pair-Setup — what tvOS 15+ real hardware needs to tunnel MRP
/// through (see `mrp::tunnel`).
pub async fn pair_airplay<F, Fut>(
    conn: &mut RtspConnection,
    pairing_id: &str,
    display_name: &str,
    get_pin: F,
) -> Result<PairingResult>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = String>,
{
    let (salt, public_key) = airplay_pairing::pair_setup_start(conn).await?;
    let pin = get_pin().await;
    airplay_pairing::pair_setup_finish(conn, pairing_id, display_name, &pin, &salt, &public_key).await
}

/// Either transport that can supply live playback position: standalone MRP
/// (older tvOS / the local fake device) or MRP tunneled inside AirPlay 2
/// (tvOS 15+ real hardware — see `mrp::tunnel`). Both expose the same
/// `PlaybackState`, so callers don't care which one is active.
pub enum LiveSession {
    Standalone(mrp::MrpSession),
    Tunneled(mrp::TunneledMrpSession),
}

impl LiveSession {
    pub fn playback(&self) -> &mrp::playback::PlaybackState {
        match self {
            LiveSession::Standalone(s) => &s.playback,
            LiveSession::Tunneled(s) => &s.playback,
        }
    }

    pub async fn recv_update(&mut self) -> Result<()> {
        match self {
            LiveSession::Standalone(s) => s.recv_update().await,
            LiveSession::Tunneled(s) => s.recv_update().await,
        }
    }

    /// Mute via AirPlay/MRP's `SendHIDEventMessage` (volume-down burst)
    /// instead of Companion's `_hidC` — see `mrp::session::MrpSession::mute`
    /// / `mrp::tunnel::TunneledMrpSession::mute`.
    pub async fn mute(&mut self) -> Result<()> {
        match self {
            LiveSession::Standalone(s) => s.mute().await,
            LiveSession::Tunneled(s) => s.mute().await,
        }
    }

    pub async fn unmute(&mut self) -> Result<()> {
        match self {
            LiveSession::Standalone(s) => s.unmute().await,
            LiveSession::Tunneled(s) => s.unmute().await,
        }
    }

    /// Actively re-request the current item's metadata rather than only
    /// waiting on the app's own pushes — see `MrpSession::refresh_position`.
    pub async fn refresh_position(&mut self) -> Result<()> {
        match self {
            LiveSession::Standalone(s) => s.refresh_position().await,
            LiveSession::Tunneled(s) => s.refresh_position().await,
        }
    }
}

/// Prefer standalone MRP when its credentials/service are available; fall
/// back to the AirPlay-tunneled path otherwise (the normal case on tvOS
/// 15+, where MRP no longer advertises its own service at all). Progress
/// and fallback notices go through `log::info!`/`log::warn!` — install a
/// logger to see them (the CLI installs one that reproduces the previous
/// println output).
pub async fn connect_live_session(saved: &SavedDevice, display_name: &str) -> Option<LiveSession> {
    if let Some(mrp_pairing) = &saved.mrp {
        match TcpStream::connect(format!("{}:{}", mrp_pairing.host, mrp_pairing.port)).await {
            Ok(mrp_stream) => match mrp::MrpSession::connect(mrp_stream, &mrp_pairing.creds, display_name).await {
                Ok(session) => {
                    info!("✅ MRP session ready (live playback position available)");
                    return Some(LiveSession::Standalone(session));
                }
                Err(e) => warn!(
                    "⚠️  MRP session setup failed: {} (trying AirPlay tunnel instead)",
                    chain_summary(&e)
                ),
            },
            Err(e) => warn!("⚠️  Could not connect to MRP service: {e} (trying AirPlay tunnel instead)"),
        }
    }

    if let Some(airplay_pairing) = &saved.airplay {
        match airplay::Ap2Session::connect(&airplay_pairing.host, airplay_pairing.port, &airplay_pairing.creds, display_name).await {
            Ok(ap2) => match mrp::TunneledMrpSession::start(ap2.data_channel, &airplay_pairing.creds, display_name).await {
                Ok(session) => {
                    info!("✅ MRP-over-AirPlay session ready (live playback position available)");
                    return Some(LiveSession::Tunneled(session));
                }
                Err(e) => warn!("⚠️  Tunneled MRP handshake failed: {} (live position unavailable)", chain_summary(&e)),
            },
            Err(e) => warn!("⚠️  AirPlay session setup failed: {} (live position unavailable)", chain_summary(&e)),
        }
    }

    None
}

//! Measures how far this machine's wall clock is from true time and feeds
//! the result into `appletv::set_wall_clock_offset_secs` -- see that
//! function's doc, and `libs/appletv/src/clock.rs`'s module doc, for why:
//! a clock even a couple seconds off from real time silently biases every
//! extrapolated playback position by exactly that amount, which looked
//! indistinguishable from a real sync bug until traced back to the OS clock
//! itself.
//!
//! A minimal SNTP client (RFC 4330) implemented directly over a UDP socket
//! rather than shelling out to a system `sntp`/`w32tm` binary -- this needs
//! to run unmodified on every Tauri target (macOS, Windows, Linux, iOS,
//! Android), and none of those consistently ship a callable NTP CLI (iOS in
//! particular has no shell to shell out to at all). `tokio::net::UdpSocket`
//! and `lookup_host` are both already exercised elsewhere in this app's
//! dependency tree and work the same way on every one of those targets.
//!
//! Deliberately doesn't touch the OS clock itself (which `sntp -s` does) --
//! that needs elevated privileges on desktop and isn't available to an app
//! sandbox at all on iOS/Android, so it wouldn't be cross-platform even if
//! this app were allowed to ask for it. Correcting the one place this crate
//! *uses* wall-clock time, instead of the wall clock itself, sidesteps that
//! entirely.

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::{lookup_host, UdpSocket};
use tokio::time::timeout;

/// Queried in order, first success wins -- three independent operators so
/// one being blocked (e.g. a network that firewalls off Apple's or
/// Cloudflare's NTP service) doesn't leave the correction unset.
const NTP_SERVERS: &[&str] = &["time.apple.com:123", "time.cloudflare.com:123", "pool.ntp.org:123"];

/// Per-server request timeout -- generous enough for a slow network, short
/// enough that a firewalled/unreachable server (the common failure mode,
/// e.g. a UDP-blocking captive portal or corporate network) doesn't stall
/// app startup for long before falling through to the next one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// NTP (1900-epoch) to Unix (1970-epoch) timestamp offset, in seconds --
/// same constant as `libs/appletv`'s `COCOA_EPOCH_OFFSET_SECS`, just a
/// different reference epoch (SNTP's wire format uses 1900, not Cocoa's
/// 2001).
const NTP_EPOCH_OFFSET_SECS: f64 = 2_208_988_800.0;

/// Runs one SNTP exchange against `server` (`"host:123"`) and returns the
/// clock offset in seconds -- `true_time - this_machine's_time` -- using
/// the standard four-timestamp formula (`(T2-T1 + T3-T4) / 2`), which
/// cancels out network round-trip delay rather than assuming a
/// symmetric-latency estimate off just the response alone.
async fn query_offset(server: &str) -> anyhow::Result<f64> {
    let addr: SocketAddr = lookup_host(server)
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("no address for {server}"))?;

    // Bind an ephemeral local socket per attempt rather than reusing one
    // across servers -- these run once at startup, not on any hot path, so
    // the extra syscalls don't matter, and it keeps each attempt fully
    // independent (no leftover state from a prior server's failed/late
    // response landing on this one).
    let local_addr: SocketAddr = if addr.is_ipv6() { "[::]:0".parse()? } else { "0.0.0.0:0".parse()? };
    let socket = UdpSocket::bind(local_addr).await?;
    socket.connect(addr).await?;

    let mut request = [0u8; 48];
    // LI = 0 (no warning), VN = 4 (NTPv4), Mode = 3 (client).
    request[0] = 0b0010_0011;

    let t1 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    socket.send(&request).await?;

    let mut response = [0u8; 48];
    let n = timeout(REQUEST_TIMEOUT, socket.recv(&mut response)).await??;
    let t4 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    if n < 48 {
        anyhow::bail!("short SNTP response from {server} ({n} bytes)");
    }

    let stratum = response[1];
    if stratum == 0 {
        // "Kiss of death" -- the server is refusing to answer (typically
        // rate-limiting), not reporting a real time. Reject rather than
        // treat this as a valid (and wildly wrong) sample.
        anyhow::bail!("{server} sent a kiss-of-death reply");
    }

    let t2 = read_ntp_timestamp(&response[32..40]);
    let t3 = read_ntp_timestamp(&response[40..48]);

    Ok(((t2 - t1) + (t3 - t4)) / 2.0)
}

/// Decodes an NTP wire timestamp: 32 bits of whole seconds since 1900,
/// followed by 32 bits of binary fraction, big-endian -- into Unix seconds.
fn read_ntp_timestamp(bytes: &[u8]) -> f64 {
    let seconds = u32::from_be_bytes(bytes[0..4].try_into().expect("4-byte slice"));
    let fraction = u32::from_be_bytes(bytes[4..8].try_into().expect("4-byte slice"));
    seconds as f64 + (fraction as f64 / u32::MAX as f64) - NTP_EPOCH_OFFSET_SECS
}

/// Tries each of `NTP_SERVERS` in turn and applies the first successful
/// offset via `appletv::set_wall_clock_offset_secs`. Meant to be spawned
/// once, in the background, at app startup (see `lib.rs`'s `setup` hook) --
/// never awaited on the startup path itself, since a slow/unreachable
/// network shouldn't delay the app opening; playback position just runs
/// uncorrected (the same behavior this app always had) until this resolves,
/// which in practice is well within the first few seconds of any session.
pub async fn sync_clock_offset() {
    for server in NTP_SERVERS {
        match query_offset(server).await {
            Ok(offset_secs) => {
                appletv::set_wall_clock_offset_secs(offset_secs);
                return;
            }
            Err(e) => {
                eprintln!("[clock] NTP check against {server} failed: {e}");
            }
        }
    }
    eprintln!("[clock] all NTP servers unreachable -- leaving wall-clock offset uncorrected.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_zero_fraction_ntp_timestamp() {
        // 2024-01-01T00:00:00Z: 1704067200 Unix seconds, so
        // 1704067200 + NTP_EPOCH_OFFSET_SECS seconds since the NTP epoch,
        // with a zero fractional part -- exactly representable, so this
        // should round-trip with no float error at all.
        let ntp_seconds: u32 = (1_704_067_200.0 + NTP_EPOCH_OFFSET_SECS) as u32;
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&ntp_seconds.to_be_bytes());
        assert_eq!(read_ntp_timestamp(&bytes), 1_704_067_200.0);
    }

    #[test]
    fn decodes_a_nonzero_fraction_within_one_microsecond() {
        // Half a second past the same reference instant -- fraction 0x8000_0000
        // is exactly 0.5 in NTP's 32-bit binary-fraction encoding.
        let ntp_seconds: u32 = (1_704_067_200.0 + NTP_EPOCH_OFFSET_SECS) as u32;
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&ntp_seconds.to_be_bytes());
        bytes[4..8].copy_from_slice(&0x8000_0000u32.to_be_bytes());
        let decoded = read_ntp_timestamp(&bytes);
        assert!((decoded - 1_704_067_200.5).abs() < 1e-6, "decoded = {decoded}");
    }
}

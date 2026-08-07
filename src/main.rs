mod mdns;
mod crypto;
mod hap_pair;
mod srp;
mod storage;
mod companion;
mod mrp;
mod airplay;

use tokio::net::TcpStream;
use std::io::{self, Write};

#[derive(Clone)]
pub struct Device { pub host: String, pub port: u16 }

#[tokio::main]
async fn main() -> Result<(), &'static str> {
    loop {
        println!("=== Apple TV Companion CLI ===");
        println!("1) Discover and pair a device");
        println!("2) Verify saved device");
        println!("3) Control device");
        println!("4) Quit");
        print!("Select: "); io::stdout().flush().unwrap();
        let mut input = String::new(); io::stdin().read_line(&mut input).unwrap();
        match input.trim() {
            "1" => pair_flow().await?,
            "2" => verify_flow().await?,
            "3" => control_flow().await?,
            "4" => break,
            _ => println!("Invalid option"),
        }
    }
    Ok(())
}

/// pyatv uses a random UUID as the controller pairing identifier; a fresh one
/// is generated per protocol (Companion and MRP are independent pairings).
pub(crate) fn random_pairing_id() -> String {
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

async fn pair_flow() -> Result<(), &'static str> {
    println!("🔎 Discovering Apple TV (_companion-link._tcp) …");
    let devices = match mdns::find_companion(std::time::Duration::from_secs(7)).await {
        Ok(devices) => {
            for (i, d) in devices.iter().enumerate() {
                println!("{}) {}:{}", i, d.host, d.port);
            }
            devices
        },
        Err(e) => {
            println!("Error discovering device: {}", e);
            return Err(e);
        }
    };

    println!("Select the device you desire to pair with:");
    print!("Select: "); io::stdout().flush().unwrap();
    let mut device = String::new(); io::stdin().read_line(&mut device).unwrap();
    let device = device.trim().parse::<usize>().unwrap_or(0);
    if device >= devices.len() {
        println!("Invalid selection");
        return Ok(());
    }

    let host = devices[device].host.clone();
    let port = devices[device].port;
    let mut stream = TcpStream::connect(format!("{host}:{port}")).await.unwrap();

    let (salt, public_key) = hap_pair::initial_pair_m1(&mut stream)
        .await
        .map_err(|_| "Failed to send initial pair request")?;

    print!("Enter Companion PIN shown on Apple TV: "); io::stdout().flush().unwrap();
    let mut pin = String::new(); io::stdin().read_line(&mut pin).unwrap();
    let pin = pin.trim();

    let pairing_id = random_pairing_id();
    let display_name = "family-filter";
    let session_key = hap_pair::pair_m3(&mut stream, pin, &salt, &public_key)
        .await
        .unwrap();
    let companion_result = match hap_pair::pair_m5(&mut stream, &pairing_id, &session_key, display_name).await {
        Ok(result) => result,
        Err(e) => {
            println!("❌ Companion pairing failed: {e}");
            return Ok(());
        }
    };
    println!("✅ Companion paired");

    // MRP and AirPlay are each their own pairing ceremony (own on-screen
    // PIN) from their own services. Standalone MRP only works on older tvOS
    // / the local fake device (tvOS 15+ stopped advertising it); AirPlay is
    // what real modern hardware needs to tunnel MRP through instead (see
    // `mrp/tunnel.rs`). Pair both whenever reachable.
    let mrp_pairing = pair_mrp().await;
    let airplay_pairing = pair_airplay().await;

    let companion_pairing = storage::Pairing { host, port, creds: companion_result };
    storage::save_pairing(&companion_pairing, mrp_pairing.as_ref(), airplay_pairing.as_ref()).unwrap();
    println!("✅ Credentials saved to pairing.store");
    Ok(())
}

/// mDNS instance host strings differ per protocol even for the same
/// physical device (and on a real home network there are usually *several*
/// distinct devices advertising the same service, e.g. multiple Apple TVs /
/// HomePods) — so there is no safe way to auto-match "the same device" the
/// Companion pairing picked. Always ask explicitly, same as Companion does.
fn prompt_device_selection(devices: &[mdns::Discovered]) -> Option<mdns::Discovered> {
    for (i, d) in devices.iter().enumerate() {
        println!("{}) {}:{}", i, d.host, d.port);
    }
    print!("Select: ");
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let idx: usize = input.trim().parse().ok()?;
    devices.get(idx).cloned()
}

async fn pair_airplay() -> Option<storage::Pairing> {
    println!("🔎 Discovering AirPlay devices (_airplay._tcp) …");
    let devices = match mdns::find_airplay(std::time::Duration::from_secs(7)).await {
        Ok(devices) => devices,
        Err(e) => {
            println!("⚠️  AirPlay discovery failed: {e} (skipping; live playback position will be unavailable)");
            return None;
        }
    };
    println!("Select the same device you just paired (for live playback position):");
    let device = prompt_device_selection(&devices)?;

    let mut conn = match airplay::rtsp::RtspConnection::connect(&device.host, device.port).await {
        Ok(c) => c,
        Err(e) => {
            println!("⚠️  Could not connect to AirPlay service: {e} (skipping)");
            return None;
        }
    };

    // Must trigger the on-screen PIN (POST /pair-pin-start + M1) *before*
    // asking the user for it — nothing shows up otherwise on real hardware.
    let (salt, public_key) = match airplay::pairing::pair_setup_start(&mut conn).await {
        Ok(v) => v,
        Err(e) => {
            print_error_chain("❌ AirPlay pairing failed to start: ", &e, "(live playback position will be unavailable)");
            return None;
        }
    };

    print!("Enter AirPlay PIN shown on Apple TV: "); io::stdout().flush().unwrap();
    let mut pin = String::new(); io::stdin().read_line(&mut pin).unwrap();
    let pin = pin.trim();

    let pairing_id = random_pairing_id();
    match airplay::pairing::pair_setup_finish(&mut conn, &pairing_id, "family-filter", pin, &salt, &public_key).await {
        Ok(creds) => {
            println!("✅ AirPlay paired");
            Some(storage::Pairing { host: device.host, port: device.port, creds })
        }
        Err(e) => {
            print_error_chain("❌ AirPlay pairing failed: ", &e, "(live playback position will be unavailable)");
            None
        }
    }
}

async fn pair_mrp() -> Option<storage::Pairing> {
    println!("🔎 Discovering MRP devices (_mediaremotetv._tcp) …");
    let devices = match mdns::find_mrp(std::time::Duration::from_secs(7)).await {
        Ok(devices) => devices,
        Err(e) => {
            println!("⚠️  MRP discovery failed: {e} (skipping; live playback position will be unavailable)");
            return None;
        }
    };
    println!("Select the same device you just paired (for live playback position):");
    let device = prompt_device_selection(&devices)?;

    let mrp_stream = match TcpStream::connect(format!("{}:{}", device.host, device.port)).await {
        Ok(s) => s,
        Err(e) => {
            println!("⚠️  Could not connect to MRP service: {e} (skipping)");
            return None;
        }
    };
    let mut conn = mrp::connection::MrpConnection::new(mrp_stream);
    let pairing_id = random_pairing_id();

    // Must trigger the on-screen PIN (DeviceInfo handshake + M1) *before*
    // asking the user for it — nothing shows up otherwise on real hardware.
    if let Err(e) = mrp::pairing::device_info_handshake(&mut conn, &pairing_id, "family-filter").await {
        print_error_chain("❌ MRP pairing failed to start: ", &e, "(Companion pairing is still saved; live position will be unavailable)");
        return None;
    }
    let (salt, public_key) = match mrp::pairing::pair_setup_m1(&mut conn).await {
        Ok(v) => v,
        Err(e) => {
            print_error_chain("❌ MRP pairing failed to start: ", &e, "(Companion pairing is still saved; live position will be unavailable)");
            return None;
        }
    };

    print!("Enter MRP PIN shown on Apple TV: "); io::stdout().flush().unwrap();
    let mut pin = String::new(); io::stdin().read_line(&mut pin).unwrap();
    let pin = pin.trim();

    let result = async {
        let session_key = mrp::pairing::pair_setup_m3(&mut conn, pin, &salt, &public_key).await?;
        mrp::pairing::pair_setup_m5(&mut conn, &pairing_id, &session_key, "family-filter").await
    }
    .await;

    match result {
        Ok(creds) => {
            println!("✅ MRP paired");
            Some(storage::Pairing { host: device.host, port: device.port, creds })
        }
        Err(e) => {
            print_error_chain("❌ MRP pairing failed: ", &e, "(Companion pairing is still saved; live position will be unavailable)");
            None
        }
    }
}

async fn verify_flow() -> Result<(), &'static str> {
    let saved = match storage::load_pairing() {
        Ok(Some(s)) => s,
        Ok(None) => {
            println!("No saved pairing. Run option 1 first.");
            return Ok(());
        }
        Err(e) => {
            println!("Failed to load pairing.store: {e}");
            return Ok(());
        }
    };

    println!("Connecting to {}:{} …", saved.companion.host, saved.companion.port);
    let mut stream = TcpStream::connect(format!("{}:{}", saved.companion.host, saved.companion.port))
        .await
        .map_err(|_| "Failed to connect")?;

    match hap_pair::pair_verify(&mut stream, &saved.companion.creds).await {
        Ok(_keys) => println!("✅ Pair-Verify OK; session encryption keys derived."),
        Err(e) => println!("❌ Pair-Verify failed: {e}"),
    }
    Ok(())
}

async fn control_flow() -> Result<(), &'static str> {
    let saved = match storage::load_pairing() {
        Ok(Some(s)) => s,
        Ok(None) => {
            println!("No saved pairing. Run option 1 first.");
            return Ok(());
        }
        Err(e) => {
            println!("Failed to load pairing.store: {e}");
            return Ok(());
        }
    };

    println!("Connecting to {}:{} …", saved.companion.host, saved.companion.port);
    let mut stream = TcpStream::connect(format!("{}:{}", saved.companion.host, saved.companion.port))
        .await
        .map_err(|_| "Failed to connect")?;

    let keys = match hap_pair::pair_verify(&mut stream, &saved.companion.creds).await {
        Ok(keys) => {
            println!("✅ Pair-Verify OK");
            keys
        }
        Err(e) => {
            println!("❌ Pair-Verify failed: {e}");
            return Ok(());
        }
    };

    let mut session = companion::CompanionSession::new(stream, keys);
    if let Err(e) = session.bootstrap(&saved.companion.creds.pairing_id).await {
        for cause in e.chain() {
            println!("  caused by: {cause}");
        }
        println!("❌ Session bootstrap failed: {e}");
        return Ok(());
    }
    println!("✅ Control session ready");

    let mut live_session = connect_live_session(&saved).await;

    loop {
        println!("--- Control ---");
        println!("1) Mute (volume 0, via AirPlay/MRP)");
        println!("2) Unmute (restore volume, via AirPlay/MRP)");
        println!("3) Skip forward by N seconds");
        println!("4) Skip backward by N seconds");
        println!("5) Show live playback position");
        println!("6) Back");
        print!("Select: ");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        match input.trim() {
            "1" => match live_session.as_mut() {
                Some(live) => match live.mute().await {
                    Ok(()) => println!("Muted (via AirPlay/MRP)"),
                    Err(e) => println!("Mute failed: {e}"),
                },
                None => println!("No AirPlay/MRP transport paired/connected; run pairing (option 1) to enable this."),
            },
            "2" => match live_session.as_mut() {
                Some(live) => match live.unmute().await {
                    Ok(()) => println!("Unmuted (via AirPlay/MRP)"),
                    Err(e) => println!("Unmute failed: {e}"),
                },
                None => println!("No AirPlay/MRP transport paired/connected; run pairing (option 1) to enable this."),
            },
            "3" => match prompt_seconds() {
                Ok(secs) => match session.skip(secs).await {
                    Ok(()) => println!("Skipped forward {secs}s"),
                    Err(e) => println!("Skip failed: {e}"),
                },
                Err(e) => println!("{e}"),
            },
            "4" => match prompt_seconds() {
                Ok(secs) => match session.skip(-secs).await {
                    Ok(()) => println!("Skipped backward {secs}s"),
                    Err(e) => println!("Skip failed: {e}"),
                },
                Err(e) => println!("{e}"),
            },
            "5" => match live_session.as_mut() {
                Some(session) => show_live_position(session).await,
                None => println!("No live-position transport paired/connected; run pairing (option 1) to enable this."),
            },
            "6" => break,
            _ => println!("Invalid option"),
        }
    }
    Ok(())
}

/// Either transport that can supply live playback position: standalone MRP
/// (older tvOS / the local fake device) or MRP tunneled inside AirPlay 2
/// (tvOS 15+ real hardware — see `mrp/tunnel.rs`). Both expose the same
/// `PlaybackState`, so the display loop doesn't care which one is active.
enum LiveSession {
    Standalone(mrp::MrpSession),
    Tunneled(mrp::TunneledMrpSession),
}

impl LiveSession {
    fn playback(&self) -> &mrp::playback::PlaybackState {
        match self {
            LiveSession::Standalone(s) => &s.playback,
            LiveSession::Tunneled(s) => &s.playback,
        }
    }

    async fn recv_update(&mut self) -> anyhow::Result<()> {
        match self {
            LiveSession::Standalone(s) => s.recv_update().await,
            LiveSession::Tunneled(s) => s.recv_update().await,
        }
    }

    /// Mute via AirPlay/MRP's `SendHIDEventMessage` (volume-down burst)
    /// instead of Companion's `_hidC` — see `mrp::session::MrpSession::mute`
    /// / `mrp::tunnel::TunneledMrpSession::mute`.
    async fn mute(&mut self) -> anyhow::Result<()> {
        match self {
            LiveSession::Standalone(s) => s.mute().await,
            LiveSession::Tunneled(s) => s.mute().await,
        }
    }

    async fn unmute(&mut self) -> anyhow::Result<()> {
        match self {
            LiveSession::Standalone(s) => s.unmute().await,
            LiveSession::Tunneled(s) => s.unmute().await,
        }
    }
}

/// Print an anyhow error's full causal chain, not just the outermost
/// `.context()` message — the underlying cause (a timeout, a wrong HTTP
/// code, a decode failure) is usually where the actual bug is.
fn print_error_chain(prefix: &str, e: &anyhow::Error, suffix: &str) {
    println!("{prefix}{e}");
    for cause in e.chain().skip(1) {
        println!("  caused by: {cause}");
    }
    if !suffix.is_empty() {
        println!("  {suffix}");
    }
}

/// Prefer standalone MRP when its credentials/service are available; fall
/// back to the AirPlay-tunneled path otherwise (the normal case on tvOS
/// 15+, where MRP no longer advertises its own service at all).
async fn connect_live_session(saved: &storage::SavedDevice) -> Option<LiveSession> {
    if let Some(mrp) = &saved.mrp {
        match TcpStream::connect(format!("{}:{}", mrp.host, mrp.port)).await {
            Ok(mrp_stream) => match mrp::MrpSession::connect(mrp_stream, &mrp.creds, "family-filter").await {
                Ok(session) => {
                    println!("✅ MRP session ready (live playback position available)");
                    return Some(LiveSession::Standalone(session));
                }
                Err(e) => print_error_chain("⚠️  MRP session setup failed: ", &e, "(trying AirPlay tunnel instead)"),
            },
            Err(e) => println!("⚠️  Could not connect to MRP service: {e} (trying AirPlay tunnel instead)"),
        }
    }

    if let Some(airplay_pairing) = &saved.airplay {
        match airplay::Ap2Session::connect(&airplay_pairing.host, airplay_pairing.port, &airplay_pairing.creds, "family-filter").await {
            Ok(ap2) => match mrp::TunneledMrpSession::start(ap2.data_channel, &airplay_pairing.creds, "family-filter").await {
                Ok(session) => {
                    println!("✅ MRP-over-AirPlay session ready (live playback position available)");
                    return Some(LiveSession::Tunneled(session));
                }
                Err(e) => print_error_chain("⚠️  Tunneled MRP handshake failed: ", &e, "(live position unavailable)"),
            },
            Err(e) => print_error_chain("⚠️  AirPlay session setup failed: ", &e, "(live position unavailable)"),
        }
    }

    None
}

/// Print the extrapolated playback position every second until the user
/// presses Enter or the connection drops. Position comes from local
/// extrapolation (see `mrp::playback::PlaybackState::position_now`), not by
/// asking the device again each tick — the device only pushes updates when
/// something actually changes.
async fn show_live_position(session: &mut LiveSession) {
    println!("Live playback position (press Enter to stop) …");

    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::task::spawn_blocking(move || {
        let mut buf = String::new();
        let _ = io::stdin().read_line(&mut buf);
        let _ = stop_tx.send(());
    });

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let playback = session.playback();
                match playback.position_now() {
                    Some(pos) => {
                        let duration = playback.duration().map(fmt_mm_ss).unwrap_or_else(|| "--:--".to_string());
                        println!(
                            "{} [{:?}] {} / {}",
                            playback.title().unwrap_or("(unknown)"),
                            playback.playback_state(),
                            fmt_mm_ss(pos),
                            duration,
                        );
                    }
                    None => println!("(no playback position yet)"),
                }
            }
            res = session.recv_update() => {
                if let Err(e) = res {
                    println!("Connection lost: {e}");
                    return;
                }
            }
            _ = &mut stop_rx => {
                println!("Stopped.");
                return;
            }
        }
    }
}

fn fmt_mm_ss(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as i64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn prompt_seconds() -> Result<f64, &'static str> {
    print!("Seconds: ");
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input
        .trim()
        .parse::<f64>()
        .map_err(|_| "Invalid number")
        .and_then(|n| {
            if n > 0.0 {
                Ok(n)
            } else {
                Err("Seconds must be positive")
            }
        })
}

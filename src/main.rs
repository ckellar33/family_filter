//! Interactive CLI front end. All AppleTV protocol/orchestration logic
//! lives in the `appletv` library crate (`libs/appletv`) — this file only
//! does menus, prompts, and printing, and wires them into that library.

use tokio::net::TcpStream;
use std::future::Ready;
use std::io::{self, Write};

use appletv::{mdns, storage};

#[tokio::main]
async fn main() -> Result<(), &'static str> {
    init_logger();
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

const DISPLAY_NAME: &str = "family-filter";

/// Forwards `appletv`'s `log::info!`/`log::warn!` progress and fallback
/// notices (e.g. from `connect_live_session`) straight to stdout, matching
/// this CLI's previous `println!`-based output.
struct CliLogger;

impl log::Log for CliLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        println!("{}", record.args());
    }
    fn flush(&self) {}
}

fn init_logger() {
    static LOGGER: CliLogger = CliLogger;
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Info);
}

/// Builds a one-shot PIN prompt for `appletv::pair_companion`/`pair_mrp`/
/// `pair_airplay` — those only call it *after* triggering the on-screen
/// code, so the prompt text can safely say "shown on Apple TV".
fn prompt_pin(protocol: &'static str) -> impl FnOnce() -> Ready<String> {
    move || {
        print!("Enter {protocol} PIN shown on Apple TV: ");
        io::stdout().flush().unwrap();
        let mut pin = String::new();
        io::stdin().read_line(&mut pin).unwrap();
        std::future::ready(pin.trim().to_string())
    }
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

    let pairing_id = appletv::random_pairing_id();
    let companion_result = match appletv::pair_companion(&mut stream, &pairing_id, DISPLAY_NAME, prompt_pin("Companion")).await {
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
    // `appletv::mrp::tunnel`). Pair both whenever reachable.
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

    let mut conn = match appletv::airplay::rtsp::RtspConnection::connect(&device.host, device.port).await {
        Ok(c) => c,
        Err(e) => {
            println!("⚠️  Could not connect to AirPlay service: {e} (skipping)");
            return None;
        }
    };

    let pairing_id = appletv::random_pairing_id();
    match appletv::pair_airplay(&mut conn, &pairing_id, DISPLAY_NAME, prompt_pin("AirPlay")).await {
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
    let mut conn = appletv::mrp::connection::MrpConnection::new(mrp_stream);
    let pairing_id = appletv::random_pairing_id();

    match appletv::pair_mrp(&mut conn, &pairing_id, DISPLAY_NAME, prompt_pin("MRP")).await {
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

    match appletv::hap_pair::pair_verify(&mut stream, &saved.companion.creds).await {
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

    let keys = match appletv::hap_pair::pair_verify(&mut stream, &saved.companion.creds).await {
        Ok(keys) => {
            println!("✅ Pair-Verify OK");
            keys
        }
        Err(e) => {
            println!("❌ Pair-Verify failed: {e}");
            return Ok(());
        }
    };

    let mut session = appletv::companion::CompanionSession::new(stream, keys);
    if let Err(e) = session.bootstrap(&saved.companion.creds.pairing_id).await {
        for cause in e.chain() {
            println!("  caused by: {cause}");
        }
        println!("❌ Session bootstrap failed: {e}");
        return Ok(());
    }
    println!("✅ Control session ready");

    let mut live_session = appletv::connect_live_session(&saved, DISPLAY_NAME).await;

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

/// Print an anyhow error's full causal chain, not just the outermost
/// `.context()` message — the underlying cause (a timeout, a wrong HTTP
/// code, a decode failure) is usually where the actual bug is.
fn print_error_chain(prefix: &str, e: &anyhow::Error, suffix: &str) {
    let chain = appletv::error_chain(e);
    println!("{prefix}{}", chain[0]);
    for cause in &chain[1..] {
        println!("  caused by: {cause}");
    }
    if !suffix.is_empty() {
        println!("  {suffix}");
    }
}

/// How many 1-second display ticks between active `refresh_position()`
/// calls. Passive extrapolation alone measurably left `elapsedTime` a few
/// seconds stale in practice (apps don't always push a fresh value on every
/// change); an occasional active re-request closes that gap without
/// hammering the device every single tick.
const POSITION_REFRESH_EVERY_TICKS: u32 = 3;

/// Print the playback position every second until the user presses Enter or
/// the connection drops. Most ticks just extrapolate locally (see
/// `appletv::mrp::playback::PlaybackState::position_now`), but every few
/// ticks actively re-requests the current item's metadata
/// (`LiveSession::refresh_position`) rather than only ever trusting however
/// stale the app's own last push happened to be.
async fn show_live_position(session: &mut appletv::LiveSession) {
    println!("Live playback position (press Enter to stop) …");

    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::task::spawn_blocking(move || {
        let mut buf = String::new();
        let _ = io::stdin().read_line(&mut buf);
        let _ = stop_tx.send(());
    });

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut tick_count: u32 = 0;
    loop {
        tokio::select! {
            _ = interval.tick() => {
                tick_count += 1;
                if tick_count % POSITION_REFRESH_EVERY_TICKS == 0 {
                    if let Err(e) = session.refresh_position().await {
                        println!("(position refresh failed, showing extrapolated value: {e})");
                    }
                }
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

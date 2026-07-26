mod mdns;
mod crypto;
mod hap_pair;
mod srp;
mod storage;

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
        println!("3) Quit");
        print!("Select: "); io::stdout().flush().unwrap();
        let mut input = String::new(); io::stdin().read_line(&mut input).unwrap();
        match input.trim() {
            "1" => pair_flow().await?,
            "2" => verify_flow().await?,
            "3" => break,
            _ => println!("Invalid option"),
        }
    }
    Ok(())
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

    print!("Enter PIN on Apple TV: "); io::stdout().flush().unwrap();
    let mut pin = String::new(); io::stdin().read_line(&mut pin).unwrap();
    let pin = pin.trim();

    let pairing_id = format!("rust-{}", gethostname::gethostname().to_string_lossy());
    let session_key = hap_pair::pair_m3(&mut stream, pin, &salt, &public_key)
        .await
        .unwrap();
    match hap_pair::pair_m5(&mut stream, &pairing_id, &session_key).await {
        Ok(result) => {
            storage::save_pairing(&host, port, &result).unwrap();
            println!("✅ Paired; credentials saved to pairing.store");
        }
        Err(e) => println!("❌ Pairing failed: {e}"),
    }
    Ok(())
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

    println!("Connecting to {}:{} …", saved.host, saved.port);
    let mut stream = TcpStream::connect(format!("{}:{}", saved.host, saved.port))
        .await
        .map_err(|_| "Failed to connect")?;

    match hap_pair::pair_verify(&mut stream, &saved.creds).await {
        Ok(_keys) => println!("✅ Pair-Verify OK; session encryption keys derived."),
        Err(e) => println!("❌ Pair-Verify failed: {e}"),
    }
    Ok(())
}

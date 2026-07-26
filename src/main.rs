// mod storage;
mod mdns;
// mod tlv8;
mod crypto;
// mod pair_setup;
// mod pair_verify;
mod hap_pair;
mod srp;

// use storage::{load_store, save_device, Creds};
use tokio::net::TcpStream;
use std::io::{self, Write};
// use mdns::find_companion;

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

    // Connect
    let mut stream = TcpStream::connect(format!("{}:{}", devices[device].host, devices[device].port)).await.unwrap();

    // Send initial pair request
    let (salt, public_key) = hap_pair::initial_pair_m1(&mut stream).await.map_err(|_| "Failed to send initial pair request")?;

    print!("Enter PIN on Apple TV: "); io::stdout().flush().unwrap();
    let mut pin = String::new(); io::stdin().read_line(&mut pin).unwrap();
    let pin = pin.trim();

    let pairing_id = format!("rust-{}", gethostname::gethostname().to_string_lossy());
    let session_key = hap_pair::pair_m3(&mut stream, pin, &salt, &public_key).await.unwrap();
    let result = hap_pair::pair_m5(&mut stream, &pairing_id, &session_key).await;
    if result.is_ok() {
        println!("✅ Paired; long-term keys exchanged.");
    } else {
        println!("❌ Pairing failed.");
    }
    Ok(())
}

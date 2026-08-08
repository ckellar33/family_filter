use std::collections::HashMap;
use std::fs;
use std::path::Path;
use anyhow::{Context, Error, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use crate::hap_pair::PairingResult;

const STORE_PATH: &str = "pairing.store";

/// One protocol's saved pairing: its own host (mDNS instance host strings
/// differ per protocol/service even for the same physical device — see
/// `main.rs::prompt_device_selection`), port, and credentials.
pub struct Pairing {
    pub host: String,
    pub port: u16,
    pub creds: PairingResult,
}

fn pairing_fields(prefix: &str, pairing: &Pairing) -> String {
    format!(
        "{prefix}host={}\n\
         {prefix}port={}\n\
         {prefix}pairing_id={}\n\
         {prefix}accessory_id={}\n\
         {prefix}our_ltsk={}\n\
         {prefix}accessory_ltpk={}\n",
        pairing.host,
        pairing.port,
        pairing.creds.pairing_id,
        hex::encode(&pairing.creds.accessory_id),
        hex::encode(pairing.creds.our_ltsk.to_bytes()),
        hex::encode(pairing.creds.accessory_ltpk.as_bytes()),
    )
}

/// Persist Companion (and, if present, standalone-MRP and/or AirPlay)
/// pairing credentials so Pair-Verify can run on a later connection. Each is
/// an independent pairing ceremony (its own on-screen PIN, and often its own
/// resolvable host), so all but Companion's are optional here. AirPlay is
/// what tvOS 15+ actually needs for live playback position (see
/// `mrp/tunnel.rs`); standalone MRP only still works on older tvOS / the
/// local fake device.
pub fn save_pairing(companion: &Pairing, mrp: Option<&Pairing>, airplay: Option<&Pairing>) -> Result<()> {
    let mut body = pairing_fields("", companion);
    if let Some(mrp) = mrp {
        body.push_str(&pairing_fields("mrp_", mrp));
    }
    if let Some(airplay) = airplay {
        body.push_str(&pairing_fields("airplay_", airplay));
    }
    fs::write(STORE_PATH, body).context("failed to write pairing.store")?;
    Ok(())
}

pub struct SavedDevice {
    pub companion: Pairing,
    pub mrp: Option<Pairing>,
    pub airplay: Option<Pairing>,
}

/// Parse the `{prefix}host` / `{prefix}port` / ... fields for one credential
/// set. Returns `Ok(None)` if the host field is entirely absent (i.e. this
/// pairing was never performed), so MRP/AirPlay can be optional while
/// Companion's remain required.
fn parse_pairing(fields: &HashMap<String, String>, prefix: &str) -> Result<Option<Pairing>> {
    let key = |name: &str| format!("{prefix}{name}");
    let Some(host) = fields.get(&key("host")) else {
        return Ok(None);
    };
    let host = host.clone();
    let port: u16 = fields
        .get(&key("port"))
        .ok_or_else(|| Error::msg(format!("missing {prefix}port")))?
        .parse()
        .context("bad port")?;

    let field_hex = |name: &str| -> Result<Vec<u8>> {
        let raw = fields
            .get(&key(name))
            .ok_or_else(|| Error::msg(format!("missing {prefix}{name}")))?;
        hex::decode(raw).with_context(|| format!("bad {prefix}{name}"))
    };

    let pairing_id = fields
        .get(&key("pairing_id"))
        .cloned()
        .ok_or_else(|| Error::msg(format!("missing {prefix}pairing_id")))?;
    let accessory_id = field_hex("accessory_id")?;
    let our_ltsk = field_hex("our_ltsk")?;
    let accessory_ltpk = field_hex("accessory_ltpk")?;

    let ltsk_bytes: [u8; 32] = our_ltsk
        .as_slice()
        .try_into()
        .map_err(|_| Error::msg("our_ltsk must be 32 bytes"))?;
    let ltpk_bytes: [u8; 32] = accessory_ltpk
        .as_slice()
        .try_into()
        .map_err(|_| Error::msg("accessory_ltpk must be 32 bytes"))?;
    let our_ltsk = SigningKey::from_bytes(&ltsk_bytes);

    Ok(Some(Pairing {
        host,
        port,
        creds: PairingResult {
            pairing_id,
            accessory_id,
            accessory_ltpk: VerifyingKey::from_bytes(&ltpk_bytes)?,
            _our_ltpk: our_ltsk.verifying_key(),
            our_ltsk,
        },
    }))
}

pub fn load_pairing() -> Result<Option<SavedDevice>> {
    if !Path::new(STORE_PATH).exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(STORE_PATH).context("failed to read pairing.store")?;
    let fields: HashMap<String, String> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let companion = parse_pairing(&fields, "")?.ok_or_else(|| Error::msg("missing Companion pairing fields"))?;
    let mrp = parse_pairing(&fields, "mrp_")?;
    let airplay = parse_pairing(&fields, "airplay_")?;

    Ok(Some(SavedDevice { companion, mrp, airplay }))
}

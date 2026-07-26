use std::fs;
use std::path::Path;
use anyhow::{Context, Error, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use crate::hap_pair::PairingResult;

const STORE_PATH: &str = "pairing.store";

/// Persist pairing credentials so Pair-Verify can run on a later connection.
pub fn save_pairing(host: &str, port: u16, result: &PairingResult) -> Result<()> {
    let body = format!(
        "host={host}\n\
         port={port}\n\
         pairing_id={}\n\
         accessory_id={}\n\
         our_ltsk={}\n\
         accessory_ltpk={}\n",
        result.pairing_id,
        hex::encode(&result.accessory_id),
        hex::encode(result.our_ltsk.to_bytes()),
        hex::encode(result.accessory_ltpk.as_bytes()),
    );
    fs::write(STORE_PATH, body).context("failed to write pairing.store")?;
    Ok(())
}

pub struct SavedDevice {
    pub host: String,
    pub port: u16,
    pub creds: PairingResult,
}

pub fn load_pairing() -> Result<Option<SavedDevice>> {
    if !Path::new(STORE_PATH).exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(STORE_PATH).context("failed to read pairing.store")?;
    let mut host = None;
    let mut port = None;
    let mut pairing_id = None;
    let mut accessory_id = None;
    let mut our_ltsk = None;
    let mut accessory_ltpk = None;

    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "host" => host = Some(v.to_string()),
            "port" => port = Some(v.parse::<u16>().context("bad port")?),
            "pairing_id" => pairing_id = Some(v.to_string()),
            "accessory_id" => accessory_id = Some(hex::decode(v).context("bad accessory_id")?),
            "our_ltsk" => our_ltsk = Some(hex::decode(v).context("bad our_ltsk")?),
            "accessory_ltpk" => {
                accessory_ltpk = Some(hex::decode(v).context("bad accessory_ltpk")?)
            }
            _ => {}
        }
    }

    let ltsk_bytes: [u8; 32] = our_ltsk
        .ok_or_else(|| Error::msg("missing our_ltsk"))?
        .as_slice()
        .try_into()
        .map_err(|_| Error::msg("our_ltsk must be 32 bytes"))?;
    let ltpk_bytes: [u8; 32] = accessory_ltpk
        .ok_or_else(|| Error::msg("missing accessory_ltpk"))?
        .as_slice()
        .try_into()
        .map_err(|_| Error::msg("accessory_ltpk must be 32 bytes"))?;

    let our_ltsk = SigningKey::from_bytes(&ltsk_bytes);
    Ok(Some(SavedDevice {
        host: host.ok_or_else(|| Error::msg("missing host"))?,
        port: port.ok_or_else(|| Error::msg("missing port"))?,
        creds: PairingResult {
            pairing_id: pairing_id.ok_or_else(|| Error::msg("missing pairing_id"))?,
            accessory_id: accessory_id.ok_or_else(|| Error::msg("missing accessory_id"))?,
            accessory_ltpk: VerifyingKey::from_bytes(&ltpk_bytes)?,
            our_ltpk: our_ltsk.verifying_key(),
            our_ltsk,
        },
    }))
}

//! AirPlay 2 Pair-Setup / Pair-Verify.
//!
//! Cryptographically identical to Companion's (`hap_pair.rs`) and standalone
//! MRP's (`mrp/pairing.rs`) HAP pairing — same SRP-6a + Ed25519 M1-M6 dance —
//! but the simplest transport of the three: plain RTSP `POST` requests with
//! raw TLV8 bodies, no OPACK or protobuf wrapping at all
//! (`pyatv/protocols/airplay/auth/hap.py`). Pair-Setup is preceded by an
//! empty `POST /pair-pin-start`, which is what makes the PIN actually appear
//! on screen.

use anyhow::{Context, Error, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use tlv8::{Method, State, Tlv8, T};
use x25519_dalek::{EphemeralSecret, PublicKey};

use crate::crypto::{hap_nonce, hkdf_512, AeadCipher};
use crate::hap_pair::{check_tlv_error, PairingResult, VerifyResult};
use crate::srp::AppleTvSrp;

use super::rtsp::RtspConnection;

fn normalize_pin(pin: &str) -> String {
    pin.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Matches pyatv's `_AIRPLAY_HEADERS` exactly (`auth/hap.py`) — real devices
/// have already proven picky about exact field/header matching elsewhere
/// (Companion's M5 Name TLV), and this specifically overrides the
/// `AirPlay/550.10` User-Agent used by SETUP/RECORD/feedback with the
/// different one pyatv uses for pairing requests.
fn octet_stream_headers() -> Vec<(String, String)> {
    vec![
        ("User-Agent".to_string(), "AirPlay/320.20".to_string()),
        ("Connection".to_string(), "keep-alive".to_string()),
        ("X-Apple-HKP".to_string(), "3".to_string()),
        ("Content-Type".to_string(), "application/octet-stream".to_string()),
    ]
}

fn generate_srp_proof(pin: &str, salt: &[u8], public_key_b: &[u8]) -> Result<(Vec<u8>, Vec<u8>, AppleTvSrp)> {
    let pin = normalize_pin(pin);
    let mut srp = AppleTvSrp::new("Pair-Setup", &pin);
    let proof = srp
        .process_challenge(salt, public_key_b)
        .map_err(|e| Error::msg(format!("failed to generate SRP proof: {e}")))?;
    let public_key_a = srp.public_key().to_vec();
    Ok((public_key_a, proof, srp))
}

async fn pair_setup_request(conn: &mut RtspConnection, tlv: Tlv8) -> Result<Vec<(T, Vec<u8>)>> {
    let headers = octet_stream_headers();
    let resp = conn
        .send_and_receive("POST", Some("/pair-setup"), &headers, &tlv.encode())
        .await
        .context("POST /pair-setup failed")?;
    if resp.code != 200 {
        return Err(Error::msg(format!("/pair-setup returned HTTP {}", resp.code)));
    }
    let tlv = Tlv8::decode(&resp.body)?;
    check_tlv_error(&tlv)?;
    Ok(tlv)
}

async fn pair_verify_request(conn: &mut RtspConnection, tlv: Tlv8) -> Result<Vec<(T, Vec<u8>)>> {
    let headers = octet_stream_headers();
    let resp = conn
        .send_and_receive("POST", Some("/pair-verify"), &headers, &tlv.encode())
        .await
        .context("POST /pair-verify failed")?;
    if resp.code != 200 {
        return Err(Error::msg(format!("/pair-verify returned HTTP {}", resp.code)));
    }
    let tlv = Tlv8::decode(&resp.body)?;
    check_tlv_error(&tlv)?;
    Ok(tlv)
}

/// Pair-Setup phase 1: triggers the on-screen PIN and returns the device's
/// salt/public key. Must run *before* the user is asked for the PIN — the
/// device doesn't show one until `/pair-pin-start` (and, on real hardware,
/// this M1 request) actually arrives.
pub async fn pair_setup_start(conn: &mut RtspConnection) -> Result<(Vec<u8>, Vec<u8>)> {
    println!("AirPlay: POST /pair-pin-start");
    let resp = conn
        .send_and_receive("POST", Some("/pair-pin-start"), &octet_stream_headers(), &[])
        .await
        .context("POST /pair-pin-start failed")?;
    println!("AirPlay: /pair-pin-start returned HTTP {}", resp.code);
    if resp.code != 200 {
        return Err(Error::msg(format!(
            "/pair-pin-start returned HTTP {} (device likely didn't show a PIN)",
            resp.code
        )));
    }

    println!("AirPlay: Pair-Setup M1");
    let m1 = Tlv8::new()
        .add_u8(T::Method, Method::PairSetup as u8)
        .add_u8(T::SeqNum, State::M1 as u8);
    let result = pair_setup_request(conn, m1).await?;
    let mut salt = None;
    let mut public_key = None;
    for (t, bytes) in result {
        match t {
            T::Salt => salt = Some(bytes),
            T::PublicKey => public_key = Some(bytes),
            _ => {}
        }
    }
    let salt = salt.ok_or_else(|| Error::msg("M2 response missing salt"))?;
    let public_key = public_key.ok_or_else(|| Error::msg("M2 response missing public key"))?;
    println!("AirPlay: Pair-Setup M1/M2 OK: salt len={} pubkey len={}", salt.len(), public_key.len());
    Ok((salt, public_key))
}

/// Pair-Setup phase 2 (M3-M6): needs the PIN the user read off-screen after
/// `pair_setup_start` triggered it.
pub async fn pair_setup_finish(
    conn: &mut RtspConnection,
    pairing_id: &str,
    display_name: &str,
    pin: &str,
    salt: &[u8],
    public_key: &[u8],
) -> Result<PairingResult> {
    println!("AirPlay: Pair-Setup M3");
    let (a_pub, proof, srp) = generate_srp_proof(pin, salt, public_key)?;
    let m3 = Tlv8::new()
        .add_u8(T::SeqNum, State::M3 as u8)
        .add(T::PublicKey, a_pub)
        .add(T::Proof, proof);
    let result = pair_setup_request(conn, m3).await?;
    let server_proof = result
        .into_iter()
        .find_map(|(t, bytes)| (t == T::Proof).then_some(bytes))
        .ok_or_else(|| Error::msg("M4 missing server proof"))?;
    srp.verify_server(&server_proof)?;
    let session_key = srp.session_key()?.to_vec();
    println!("AirPlay: Pair-Setup M3/M4 OK: session key len={}", session_key.len());

    println!("AirPlay: Pair-Setup M5");
    let mut csprng = rand::rngs::OsRng;
    let ltsk = SigningKey::generate(&mut csprng);
    let ltpk = ltsk.verifying_key();

    let device_x = hkdf_512(&session_key, b"Pair-Setup-Controller-Sign-Salt", b"Pair-Setup-Controller-Sign-Info", 32);
    let mut sign_material = Vec::with_capacity(32 + pairing_id.len() + 32);
    sign_material.extend_from_slice(&device_x);
    sign_material.extend_from_slice(pairing_id.as_bytes());
    sign_material.extend_from_slice(ltpk.as_bytes());
    let signature = ltsk.sign(&sign_material);

    let mut name_opack_dict = std::collections::HashMap::new();
    name_opack_dict.insert("name".to_string(), opack::Value::Str(display_name.to_string()));
    let name_opack = opack::encode(&opack::Value::Dict(name_opack_dict)).map_err(|e| Error::msg(format!("opack encode name: {e}")))?;

    let sub = Tlv8::new()
        .add(T::Identifier, pairing_id.as_bytes())
        .add(T::PublicKey, ltpk.as_bytes().to_vec())
        .add(T::Signature, signature.to_bytes().to_vec())
        .add(T::Name, name_opack)
        .encode();

    let enc_key = hkdf_512(&session_key, b"Pair-Setup-Encrypt-Salt", b"Pair-Setup-Encrypt-Info", 32);
    let aead = AeadCipher::new(&enc_key);
    let ciphertext = aead.seal(&hap_nonce(b"PS-Msg05"), &sub);

    let m5 = Tlv8::new().add_u8(T::SeqNum, State::M5 as u8).add(T::EncryptedData, ciphertext);
    let result = pair_setup_request(conn, m5).await?;

    let enc = result
        .into_iter()
        .find_map(|(t, bytes)| (t == T::EncryptedData).then_some(bytes))
        .ok_or_else(|| Error::msg("M6 missing encrypted data"))?;
    let plain = aead.open(&hap_nonce(b"PS-Msg06"), &enc)?;
    let sub_tlv = Tlv8::decode(&plain)?;

    let mut acc_id = None;
    let mut acc_ltpk_bytes = None;
    let mut acc_sig_bytes = None;
    for (t, bytes) in sub_tlv {
        match t {
            T::Identifier => acc_id = Some(bytes),
            T::PublicKey => acc_ltpk_bytes = Some(bytes),
            T::Signature => acc_sig_bytes = Some(bytes),
            _ => {}
        }
    }
    let acc_id = acc_id.ok_or_else(|| Error::msg("M6 sub-TLV missing identifier"))?;
    let acc_ltpk_bytes = acc_ltpk_bytes.ok_or_else(|| Error::msg("M6 sub-TLV missing accessory LTPK"))?;
    let acc_sig_bytes = acc_sig_bytes.ok_or_else(|| Error::msg("M6 sub-TLV missing signature"))?;

    let acc_ltpk = VerifyingKey::from_bytes(
        acc_ltpk_bytes.as_slice().try_into().map_err(|_| Error::msg("accessory LTPK is not 32 bytes"))?,
    )?;
    let acc_sig = Signature::from_bytes(
        acc_sig_bytes.as_slice().try_into().map_err(|_| Error::msg("accessory signature is not 64 bytes"))?,
    );

    let accessory_x = hkdf_512(&session_key, b"Pair-Setup-Accessory-Sign-Salt", b"Pair-Setup-Accessory-Sign-Info", 32);
    let mut verify_material = Vec::with_capacity(32 + acc_id.len() + 32);
    verify_material.extend_from_slice(&accessory_x);
    verify_material.extend_from_slice(&acc_id);
    verify_material.extend_from_slice(acc_ltpk.as_bytes());
    acc_ltpk
        .verify_strict(&verify_material, &acc_sig)
        .context("M6 accessory signature invalid")?;

    println!("AirPlay: Pair-Setup M5/M6 OK: accessory id={}", String::from_utf8_lossy(&acc_id));
    Ok(PairingResult {
        pairing_id: pairing_id.to_string(),
        accessory_id: acc_id,
        accessory_ltpk: acc_ltpk,
        _our_ltpk: ltpk,
        our_ltsk: ltsk,
    })
}

/// AirPlay Pair-Verify on a fresh connection using saved long-term keys.
/// Returns the derived shared secret (needed by the caller to derive
/// per-channel keys for the event/data channels) alongside the
/// Control-channel session keys.
pub async fn pair_verify(conn: &mut RtspConnection, creds: &PairingResult) -> Result<([u8; 32], VerifyResult)> {
    println!("AirPlay: Pair-Verify M1");
    let eph = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let our_eph_pub = PublicKey::from(&eph);

    let m1 = Tlv8::new()
        .add_u8(T::SeqNum, State::M1 as u8)
        .add(T::PublicKey, our_eph_pub.as_bytes().to_vec());
    let result = pair_verify_request(conn, m1).await?;

    let mut acc_eph_bytes = None;
    let mut enc_data = None;
    for (t, bytes) in result {
        match t {
            T::PublicKey => acc_eph_bytes = Some(bytes),
            T::EncryptedData => enc_data = Some(bytes),
            _ => {}
        }
    }
    let acc_eph_bytes = acc_eph_bytes.ok_or_else(|| Error::msg("PV M2 missing accessory public key"))?;
    let enc_data = enc_data.ok_or_else(|| Error::msg("PV M2 missing encrypted data"))?;

    let acc_eph = PublicKey::from(
        <[u8; 32]>::try_from(acc_eph_bytes.as_slice()).map_err(|_| Error::msg("accessory ephemeral public key is not 32 bytes"))?,
    );
    let shared = eph.diffie_hellman(&acc_eph);

    let pv_key = hkdf_512(shared.as_bytes(), b"Pair-Verify-Encrypt-Salt", b"Pair-Verify-Encrypt-Info", 32);
    let aead = AeadCipher::new(&pv_key);
    let plain = aead.open(&hap_nonce(b"PV-Msg02"), &enc_data)?;
    let sub_tlv = Tlv8::decode(&plain)?;

    let mut acc_id = None;
    let mut acc_sig_bytes = None;
    for (t, bytes) in sub_tlv {
        match t {
            T::Identifier => acc_id = Some(bytes),
            T::Signature => acc_sig_bytes = Some(bytes),
            _ => {}
        }
    }
    let acc_id = acc_id.ok_or_else(|| Error::msg("PV M2 sub-TLV missing identifier"))?;
    let acc_sig_bytes = acc_sig_bytes.ok_or_else(|| Error::msg("PV M2 sub-TLV missing signature"))?;

    if acc_id != creds.accessory_id {
        return Err(Error::msg("PV M2 accessory id does not match saved pairing"));
    }
    let acc_sig = Signature::from_bytes(
        acc_sig_bytes.as_slice().try_into().map_err(|_| Error::msg("accessory signature is not 64 bytes"))?,
    );

    let mut verify_material = Vec::with_capacity(32 + acc_id.len() + 32);
    verify_material.extend_from_slice(acc_eph.as_bytes());
    verify_material.extend_from_slice(&acc_id);
    verify_material.extend_from_slice(our_eph_pub.as_bytes());
    creds
        .accessory_ltpk
        .verify_strict(&verify_material, &acc_sig)
        .context("PV M2 accessory signature invalid")?;

    println!("AirPlay: Pair-Verify M3");
    let mut sign_material = Vec::with_capacity(32 + creds.pairing_id.len() + 32);
    sign_material.extend_from_slice(our_eph_pub.as_bytes());
    sign_material.extend_from_slice(creds.pairing_id.as_bytes());
    sign_material.extend_from_slice(acc_eph.as_bytes());
    let sig = creds.our_ltsk.sign(&sign_material);

    let sub = Tlv8::new()
        .add(T::Identifier, creds.pairing_id.as_bytes())
        .add(T::Signature, sig.to_bytes().to_vec())
        .encode();
    let enc = aead.seal(&hap_nonce(b"PV-Msg03"), &sub);

    let m3 = Tlv8::new().add_u8(T::SeqNum, State::M3 as u8).add(T::EncryptedData, enc);
    pair_verify_request(conn, m3).await?;

    let client_encrypt_key = hkdf_512(shared.as_bytes(), b"Control-Salt", b"Control-Write-Encryption-Key", 32);
    let server_encrypt_key = hkdf_512(shared.as_bytes(), b"Control-Salt", b"Control-Read-Encryption-Key", 32);
    println!(
        "AirPlay: Pair-Verify OK; client_key len={} server_key len={}",
        client_encrypt_key.len(),
        server_encrypt_key.len()
    );

    Ok((
        *shared.as_bytes(),
        VerifyResult {
            client_encrypt_key,
            server_encrypt_key,
        },
    ))
}

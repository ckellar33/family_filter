//! MRP Pair-Setup / Pair-Verify.
//!
//! Cryptographically identical to Companion's HAP pairing in `hap_pair.rs`
//! (same SRP-6a + Ed25519 M1-M6 dance, same TLV8 vocabulary) — only the
//! transport differs: TLV8 blobs travel inside `CryptoPairingMessage.pairingData`
//! wrapped in a protobuf `ProtocolMessage` envelope over varint-framed MRP
//! frames, instead of raw OPACK dicts over Companion's 4-byte-header frames.
//! MRP also derives its final session keys with different HKDF labels.
//!
//! Every MRP connection — pairing or verifying — must send `DeviceInfoMessage`
//! and receive the device's response before anything else (confirmed from
//! pyatv's `MrpProtocol.start()`, which both `MrpPairingHandler` and normal
//! connections funnel through).

use anyhow::{Context, Error, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use protobuf_lite::WireValue;
use tlv8::{Method, State, Tlv8, T};
use x25519_dalek::{EphemeralSecret, PublicKey};

use crate::crypto::{hap_nonce, hkdf_512, AeadCipher};
use crate::hap_pair::{check_tlv_error, PairingResult, VerifyResult};
use crate::srp::AppleTvSrp;

use super::connection::{MrpConnection, HANDSHAKE_TIMEOUT};
use super::messages;

fn normalize_pin(pin: &str) -> String {
    pin.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Send `DeviceInfoMessage` and wait for the device's own `DeviceInfoMessage`
/// reply. Must be the first exchange on every fresh MRP connection.
pub async fn device_info_handshake(conn: &mut MrpConnection, pairing_id: &str, display_name: &str) -> Result<()> {
    println!("MRP: sending DeviceInfoMessage (pairing_id={pairing_id})");
    let inner = messages::device_info_message(pairing_id, display_name, "20F66");
    let msg = messages::wrap(
        messages::TYPE_DEVICE_INFO_MESSAGE,
        Some(&messages::random_identifier()),
        Some((messages::FIELD_DEVICE_INFO_MESSAGE, inner)),
    );
    conn.send(&msg).await.context("send DeviceInfoMessage")?;

    let resp = conn
        .recv_timeout(HANDSHAKE_TIMEOUT)
        .await
        .context("recv DeviceInfoMessage response")?;
    let (msg_type, _fields) = messages::parse_envelope(&resp)?;
    println!("MRP: received response type={msg_type} ({} bytes)", resp.len());
    if msg_type != messages::TYPE_DEVICE_INFO_MESSAGE {
        return Err(Error::msg(format!(
            "expected DeviceInfoMessage (type {}) response, got type {msg_type}",
            messages::TYPE_DEVICE_INFO_MESSAGE
        )));
    }
    Ok(())
}

async fn crypto_pairing_exchange(conn: &mut MrpConnection, tlv: Tlv8, is_pairing: bool) -> Result<Vec<(T, Vec<u8>)>> {
    let inner = messages::crypto_pairing_message(&tlv.encode(), is_pairing);
    let msg = messages::wrap(
        messages::TYPE_CRYPTO_PAIRING_MESSAGE,
        None,
        Some((messages::FIELD_CRYPTO_PAIRING_MESSAGE, inner)),
    );
    println!("MRP: sending CryptoPairingMessage ({} bytes TLV, is_pairing={is_pairing})", tlv.encode().len());
    conn.send(&msg).await.context("send CryptoPairingMessage")?;

    let resp = conn
        .recv_timeout(HANDSHAKE_TIMEOUT)
        .await
        .context("recv CryptoPairingMessage response")?;
    let (msg_type, fields) = messages::parse_envelope(&resp)?;
    println!("MRP: received response type={msg_type} ({} bytes)", resp.len());
    if msg_type != messages::TYPE_CRYPTO_PAIRING_MESSAGE {
        return Err(Error::msg(format!(
            "expected CryptoPairingMessage (type {}) response, got type {msg_type}",
            messages::TYPE_CRYPTO_PAIRING_MESSAGE
        )));
    }
    let inner_bytes = protobuf_lite::last_field(&fields, messages::FIELD_CRYPTO_PAIRING_MESSAGE)
        .and_then(WireValue::as_bytes)
        .ok_or_else(|| Error::msg("CryptoPairingMessage response missing inner message"))?;
    let pairing_data = messages::crypto_pairing_data(inner_bytes)?;
    let tlv = Tlv8::decode(&pairing_data)?;
    check_tlv_error(&tlv)?;
    Ok(tlv)
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

/// Pair-Setup M1: request salt + device public key.
pub async fn pair_setup_m1(conn: &mut MrpConnection) -> Result<(Vec<u8>, Vec<u8>)> {
    let tlv = Tlv8::new()
        .add_u8(T::Method, Method::PairSetup as u8)
        .add_u8(T::SeqNum, State::M1 as u8);
    let result = crypto_pairing_exchange(conn, tlv, true).await?;

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
    println!("MRP Pair-Setup M1/M2 OK: salt len={} pubkey len={}", salt.len(), public_key.len());
    Ok((salt, public_key))
}

/// Pair-Setup M3: exchange SRP proofs, return the shared session key.
pub async fn pair_setup_m3(conn: &mut MrpConnection, pin: &str, salt: &[u8], public_key: &[u8]) -> Result<Vec<u8>> {
    let (a_pub, proof, srp) = generate_srp_proof(pin, salt, public_key)?;
    let tlv = Tlv8::new()
        .add_u8(T::SeqNum, State::M3 as u8)
        .add(T::PublicKey, a_pub)
        .add(T::Proof, proof);
    let result = crypto_pairing_exchange(conn, tlv, false).await?;

    let server_proof = result
        .into_iter()
        .find_map(|(t, bytes)| (t == T::Proof).then_some(bytes))
        .ok_or_else(|| Error::msg("M4 missing server proof"))?;
    srp.verify_server(&server_proof)?;
    let session_key = srp.session_key()?.to_vec();
    println!("MRP Pair-Setup M3/M4 OK: server proof verified, session key len={}", session_key.len());
    Ok(session_key)
}

/// Pair-Setup M5: exchange long-term Ed25519 identities.
pub async fn pair_setup_m5(
    conn: &mut MrpConnection,
    pairing_id: &str,
    session_key: &[u8],
    display_name: &str,
) -> Result<PairingResult> {
    let mut csprng = rand::rngs::OsRng;
    let ltsk = SigningKey::generate(&mut csprng);
    let ltpk = ltsk.verifying_key();

    let device_x = hkdf_512(session_key, b"Pair-Setup-Controller-Sign-Salt", b"Pair-Setup-Controller-Sign-Info", 32);
    let mut sign_material = Vec::with_capacity(32 + pairing_id.len() + 32);
    sign_material.extend_from_slice(&device_x);
    sign_material.extend_from_slice(pairing_id.as_bytes());
    sign_material.extend_from_slice(ltpk.as_bytes());
    let signature = ltsk.sign(&sign_material);

    let mut name_opack_dict = std::collections::HashMap::new();
    name_opack_dict.insert("name".to_string(), opack::Value::Str(display_name.to_string()));
    let name_opack = opack::encode(&opack::Value::Dict(name_opack_dict))
        .map_err(|e| Error::msg(format!("opack encode name: {e}")))?;

    let sub = Tlv8::new()
        .add(T::Identifier, pairing_id.as_bytes())
        .add(T::PublicKey, ltpk.as_bytes().to_vec())
        .add(T::Signature, signature.to_bytes().to_vec())
        .add(T::Name, name_opack)
        .encode();

    let enc_key = hkdf_512(session_key, b"Pair-Setup-Encrypt-Salt", b"Pair-Setup-Encrypt-Info", 32);
    let aead = AeadCipher::new(&enc_key);
    let ciphertext = aead.seal(&hap_nonce(b"PS-Msg05"), &sub);

    let tlv = Tlv8::new()
        .add_u8(T::SeqNum, State::M5 as u8)
        .add(T::EncryptedData, ciphertext);
    let result = crypto_pairing_exchange(conn, tlv, false).await?;

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

    let accessory_x = hkdf_512(session_key, b"Pair-Setup-Accessory-Sign-Salt", b"Pair-Setup-Accessory-Sign-Info", 32);
    let mut verify_material = Vec::with_capacity(32 + acc_id.len() + 32);
    verify_material.extend_from_slice(&accessory_x);
    verify_material.extend_from_slice(&acc_id);
    verify_material.extend_from_slice(acc_ltpk.as_bytes());
    acc_ltpk
        .verify_strict(&verify_material, &acc_sig)
        .context("M6 accessory signature invalid")?;

    println!(
        "MRP Pair-Setup M5/M6 OK: accessory id={}",
        String::from_utf8_lossy(&acc_id)
    );
    Ok(PairingResult {
        pairing_id: pairing_id.to_string(),
        accessory_id: acc_id,
        accessory_ltpk: acc_ltpk,
        _our_ltpk: ltpk,
        our_ltsk: ltsk,
    })
}

/// Pair-Verify on a fresh MRP connection using saved long-term keys, deriving
/// MRP's session encryption keys (different HKDF labels from Companion).
pub async fn pair_verify(conn: &mut MrpConnection, creds: &PairingResult, display_name: &str) -> Result<VerifyResult> {
    device_info_handshake(conn, &creds.pairing_id, display_name).await?;

    let eph = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let our_eph_pub = PublicKey::from(&eph);

    let tlv = Tlv8::new()
        .add_u8(T::SeqNum, State::M1 as u8)
        .add(T::PublicKey, our_eph_pub.as_bytes().to_vec());
    let result = crypto_pairing_exchange(conn, tlv, false).await.context("MRP Pair-Verify M1/M2 failed")?;

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

    let tlv = Tlv8::new().add_u8(T::SeqNum, State::M3 as u8).add(T::EncryptedData, enc);
    crypto_pairing_exchange(conn, tlv, false).await.context("MRP Pair-Verify M3/M4 failed")?;

    // MRP-specific HKDF labels (Companion uses empty salt + ClientEncrypt-main/
    // ServerEncrypt-main; see protocol.py SRP_SALT/SRP_OUTPUT_INFO/SRP_INPUT_INFO).
    let output_key = hkdf_512(shared.as_bytes(), b"MediaRemote-Salt", b"MediaRemote-Write-Encryption-Key", 32);
    let input_key = hkdf_512(shared.as_bytes(), b"MediaRemote-Salt", b"MediaRemote-Read-Encryption-Key", 32);

    println!("MRP Pair-Verify OK; client_key len={} server_key len={}", output_key.len(), input_key.len());
    Ok(VerifyResult {
        client_encrypt_key: output_key,
        server_encrypt_key: input_key,
    })
}

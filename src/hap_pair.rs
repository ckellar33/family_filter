use std::collections::HashMap;
use anyhow::{Context, Error, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream};
use x25519_dalek::{EphemeralSecret, PublicKey};
use tlv8::{Tlv8, T, State, Method, Error as HapError};
use opack::{Value, decode, encode};
use log::debug;

use crate::srp::AppleTvSrp;
use crate::crypto::{hap_nonce, hkdf_512, AeadCipher};

pub struct PairingResult {
    /// Our controller pairing identifier (sent in M5 / Pair-Verify M3).
    pub pairing_id: String,
    /// Accessory pairing identifier (from M6).
    pub accessory_id: Vec<u8>,
    pub accessory_ltpk: VerifyingKey,
    pub _our_ltpk: VerifyingKey,
    pub our_ltsk: SigningKey,
}

/// Session keys derived after successful Pair-Verify (Companion encryption).
pub struct VerifyResult {
    /// HKDF info `ClientEncrypt-main` — encrypt outgoing frames.
    pub client_encrypt_key: Vec<u8>,
    /// HKDF info `ServerEncrypt-main` — decrypt incoming frames.
    pub server_encrypt_key: Vec<u8>,
}

fn write_opack_frame(frame_type: u8, dict: HashMap<String, Value>) -> Vec<u8> {
    let content = encode(&Value::Dict(dict)).unwrap();
    let content_len = u32::to_be_bytes(content.len() as u32);
    [vec![frame_type], content_len[1..4].to_vec(), content].concat()
}

async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; 8192];
    let read_size = stream
        .read(&mut buf)
        .await
        .map_err(|_| Error::msg("failed to read response"))?;
    buf.truncate(read_size);
    Ok(buf)
}

fn normalize_pin(pin: &str) -> String {
    pin.chars().filter(|c| c.is_ascii_digit()).collect()
}

pub(crate) fn check_tlv_error(tlv: &[(T, Vec<u8>)]) -> Result<()> {
    for (t, bytes) in tlv {
        if *t == T::Error {
            let code = bytes.first().copied().unwrap_or(0);
            let msg = match code {
                c if c == HapError::Authentication as u8 => {
                    "authentication failed (wrong PIN, bad SRP proof, or M5 decrypt/signature failed)"
                }
                c if c == HapError::Busy as u8 => "accessory is busy",
                c if c == HapError::MaxTries as u8 => "max pairing attempts exceeded",
                // Real Apple TVs often return Unknown when Companion M5 lacks the
                // Name (0x11) OPACK device-info TLV that pyatv always sends.
                c if c == HapError::Unknown as u8 => {
                    "unknown pairing error (real Apple TV often rejects M5 without Name TLV 0x11)"
                }
                other => return Err(Error::msg(format!("pairing error code {other}"))),
            };
            return Err(Error::msg(msg));
        }
    }
    Ok(())
}

/// OPACK device-info blob for Companion Pair-Setup M5 (TLV type 0x11 / Name).
///
/// pyatv sends `opack.pack({"name": display_name})`. Fake devices ignore it;
/// real Apple TVs require it and otherwise reply with Error=Unknown at M6.
fn companion_device_info_opack(display_name: &str) -> Result<Vec<u8>> {
    let mut dict = HashMap::new();
    dict.insert("name".to_string(), Value::Str(display_name.to_string()));
    encode(&Value::Dict(dict)).map_err(|e| Error::msg(format!("opack encode name: {e}")))
}

/// Generates the SRP proof for HomeKit / Companion pairing.
///
/// Returns (public key A, proof M1, SRP client) so M4 can verify the accessory proof.
fn generate_srp_proof(pin: &str, salt: &[u8], public_key_b: &[u8]) -> Result<(Vec<u8>, Vec<u8>, AppleTvSrp)> {
    let pin = normalize_pin(pin);
    println!("SRP PIN (normalized): {:?} (len={})", pin, pin.len());
    println!("SRP salt len={}, B len={}", salt.len(), public_key_b.len());

    let mut srp = AppleTvSrp::new("Pair-Setup", &pin);
    let proof = srp
        .process_challenge(salt, public_key_b)
        .map_err(|e| Error::msg(format!("Failed to generate proof: {e}")))?;
    let public_key_a = srp.public_key().to_vec();

    println!(
        "public key A len={}\n proof M1 len={}",
        public_key_a.len(),
        proof.len()
    );

    Ok((public_key_a, proof, srp))
}

pub async fn initial_pair_m1(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>)> {
    // Initial pair request
    let m1 = Tlv8::new()
        .add_u8(T::SeqNum, State::M1 as u8)
        .add_u8(T::Method, Method::PairSetup as u8)
        .encode();
    println!("{:?}", m1);
    let mut dict = HashMap::new();
    // 03000013e2435f7064 76000100060101 455f7077547909
    // 03 - frame type
    // 00 00 13 - length
    // e2 - dictionary with 2 entries
    // 43 - string with 3 chars
    // 5f 70 64 - "_pd"
    // 76 - raw bytes - 6
    // TLV
    // 00 - code
    // 01 - length
    // 00 - value
    // 06 - code
    // 01 - length
    // 01 - value
    // 45 - string with 5 chars
    // 5f 70 77 54 79 - "_pwTy"
    // 09 - decimal 1

    dict.insert("_pd".to_string(), Value::Bytes(m1.to_vec()));
    dict.insert("_pwTy".to_string(), Value::Decimal(1u8)); // 1 = M1
    let mut bytes : Vec<u8> = vec![];
    // Prepend frame type
    bytes.push(0x03);
    // Need to add the length here as 3 bytes
    let content = encode(&opack::Value::Dict(dict)).unwrap();
    let content_len = u32::to_be_bytes(content.len() as u32);
    println!("content_len: {:?}", content_len);
    bytes = [bytes, content_len[1..4].to_vec(), content].concat();
    println!("bytes: {:x?}", bytes);

    stream.write_all(&bytes).await?;

    /* Read the M2 Response - Includes Salt and public key */
    let mut buf = vec![0u8; 8192];
    if let Ok(read_size) = stream.read(&mut buf).await {
        buf.truncate(read_size);
        println!("Read M2 Response: {:x?}", buf.as_slice());
        if buf[0] != 0x4 { // Expected 0x4 for a M2 Message
            println!("Failed to receive M2: Pair-Setup Next");
        }
        let result = process_response(buf.as_slice())?;
        check_tlv_error(&result)?;
        let mut salt = vec![];
        let mut public_key = vec![];
        for (t, bytes) in result {
            match t {
                T::Salt => salt = bytes,
                T::PublicKey => public_key = bytes,
                _ => {}
            }
        }
        if salt.is_empty() || public_key.is_empty() {
            return Err(Error::msg("M2 response missing salt or public key"));
        }
        Ok((salt, public_key))
    } else {
        return Err(Error::msg("Failed to read M2 response."))
    }
}

pub async fn pair_m3(stream: &mut TcpStream, pin: &str, salt: &[u8], public_key: &[u8]) -> Result<Vec<u8>> {
    let (a_pub, proof, srp) = generate_srp_proof(pin, salt, public_key)?;

    let m3 = Tlv8::new()
        .add_u8(T::SeqNum, State::M3 as u8)
        .add(T::PublicKey, a_pub)
        .add(T::Proof, proof)
        .encode();
    println!("{:?}", m3);
    let mut dict = HashMap::new();
    dict.insert("_pd".to_string(), Value::Bytes(m3.to_vec()));
    dict.insert("_pwTy".to_string(), Value::Decimal(1u8));
    let mut bytes: Vec<u8> = vec![];
    // Prepend frame type (PS_Next)
    bytes.push(0x04);
    let content = encode(&opack::Value::Dict(dict)).unwrap();
    let content_len = u32::to_be_bytes(content.len() as u32);
    println!("content_len: {:?}", content_len);
    bytes = [bytes, content_len[1..4].to_vec(), content].concat();
    println!("bytes: {:x?}", bytes);

    stream.write_all(&bytes).await?;

    /* Read the M4 Response - Includes accessory proof */
    let mut buf = vec![0u8; 8192];
    if let Ok(read_size) = stream.read(&mut buf).await {
        buf.truncate(read_size);
        println!("Read M4 Response: {:x?}", buf.as_slice());
        if buf[0] != 0x4 {
            println!("Failed to receive M4: Pair-Setup Next");
        }
        let result = process_response(buf.as_slice())?;
        check_tlv_error(&result)?;
        let mut server_proof = None;
        for (t, bytes) in result {
            if t == T::Proof {
                server_proof = Some(bytes);
            }
        }
        let server_proof = server_proof.ok_or_else(|| Error::msg("M4 missing server proof"))?;
        srp.verify_server(&server_proof)?;
        let session_key = srp.session_key()?.to_vec();
        println!("M4 server proof verified; session key len={}", session_key.len());
        Ok(session_key)
    } else {
        Err(Error::msg("Failed to read M4 response."))
    }
}

/// Pair-Setup M5/M6: exchange long-term Ed25519 keys (HAP §5.6.5–5.6.6).
///
/// For Companion (Apple TV), the encrypted M5 sub-TLV must also include
/// `T::Name` (0x11) with an OPACK `{"name": ...}` dict — same as pyatv.
/// Without it, real devices return Error=Unknown at M6 while fake_device still succeeds.
pub async fn pair_m5(
    stream: &mut TcpStream,
    pairing_id: &str,
    session_key: &[u8],
    display_name: &str,
) -> Result<PairingResult> {
    let mut csprng = rand::rngs::OsRng;
    let ltsk = SigningKey::generate(&mut csprng);
    let ltpk = ltsk.verifying_key();

    // DeviceX = HKDF(SRP K, Pair-Setup-Controller-Sign-*)
    let device_x = hkdf_512(
        session_key,
        b"Pair-Setup-Controller-Sign-Salt",
        b"Pair-Setup-Controller-Sign-Info",
        32,
    );
    let mut sign_material = Vec::with_capacity(32 + pairing_id.len() + 32);
    sign_material.extend_from_slice(&device_x);
    sign_material.extend_from_slice(pairing_id.as_bytes());
    sign_material.extend_from_slice(ltpk.as_bytes());
    let signature = ltsk.sign(&sign_material);

    // Companion requires Name (0x11) inside the encrypted payload (pyatv step3).
    let name_opack = companion_device_info_opack(display_name)?;
    let sub = Tlv8::new()
        .add(T::Identifier, pairing_id.as_bytes())
        .add(T::PublicKey, ltpk.as_bytes().to_vec())
        .add(T::Signature, signature.to_bytes().to_vec())
        .add(T::Name, name_opack)
        .encode();

    let enc_key = hkdf_512(
        session_key,
        b"Pair-Setup-Encrypt-Salt",
        b"Pair-Setup-Encrypt-Info",
        32,
    );
    let aead = AeadCipher::new(&enc_key);
    let ciphertext = aead.seal(&hap_nonce(b"PS-Msg05"), &sub);

    let m5 = Tlv8::new()
        .add_u8(T::SeqNum, State::M5 as u8)
        .add(T::EncryptedData, ciphertext)
        .encode();
    println!("M5 TLV: {:?}", m5);

    let mut dict = HashMap::new();
    dict.insert("_pd".to_string(), Value::Bytes(m5.to_vec()));
    dict.insert("_pwTy".to_string(), Value::Decimal(1u8));
    let mut bytes: Vec<u8> = vec![];
    bytes.push(0x04); // PS_Next
    let content = encode(&opack::Value::Dict(dict)).unwrap();
    let content_len = u32::to_be_bytes(content.len() as u32);
    bytes = [bytes, content_len[1..4].to_vec(), content].concat();
    println!("M5 bytes: {:x?}", bytes);

    stream.write_all(&bytes).await?;

    /* Read M6: encrypted accessory identifier + LTPK + signature */
    let mut buf = vec![0u8; 8192];
    let read_size = stream.read(&mut buf).await.map_err(|_| Error::msg("Failed to read M6 response."))?;
    buf.truncate(read_size);
    println!("Read M6 Response: {:x?}", buf.as_slice());
    if buf.first().copied() != Some(0x4) {
        println!("Failed to receive M6: Pair-Setup Next");
    }

    let result = process_response(buf.as_slice())?;
    check_tlv_error(&result)?;
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
    let acc_ltpk_bytes =
        acc_ltpk_bytes.ok_or_else(|| Error::msg("M6 sub-TLV missing accessory LTPK"))?;
    let acc_sig_bytes =
        acc_sig_bytes.ok_or_else(|| Error::msg("M6 sub-TLV missing signature"))?;

    let acc_ltpk = VerifyingKey::from_bytes(
        acc_ltpk_bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::msg("accessory LTPK is not 32 bytes"))?,
    )?;
    let acc_sig = Signature::from_bytes(
        acc_sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::msg("accessory signature is not 64 bytes"))?,
    );

    // AccessoryX = HKDF(SRP K, Pair-Setup-Accessory-Sign-*)
    let accessory_x = hkdf_512(
        session_key,
        b"Pair-Setup-Accessory-Sign-Salt",
        b"Pair-Setup-Accessory-Sign-Info",
        32,
    );
    let mut verify_material = Vec::with_capacity(32 + acc_id.len() + 32);
    verify_material.extend_from_slice(&accessory_x);
    verify_material.extend_from_slice(&acc_id);
    verify_material.extend_from_slice(acc_ltpk.as_bytes());
    acc_ltpk
        .verify_strict(&verify_material, &acc_sig)
        .context("M6 accessory signature invalid")?;

    println!(
        "M6 verified; accessory id={}",
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

/// Pair-Verify on a fresh Companion connection using saved long-term keys (HAP §5.7).
///
/// Frame types: M1 = `PV_Start` (0x05), M2–M4 = `PV_Next` (0x06).
/// M1 also carries `_auTy: 4` (Companion auth type).
pub async fn pair_verify(
    stream: &mut TcpStream,
    creds: &PairingResult,
) -> Result<VerifyResult> {
    // M1: ephemeral X25519 public key
    let eph = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let our_eph_pub = PublicKey::from(&eph);

    let m1 = Tlv8::new()
        .add_u8(T::SeqNum, State::M1 as u8)
        .add(T::PublicKey, our_eph_pub.as_bytes().to_vec())
        .encode();
    let mut dict = HashMap::new();
    dict.insert("_pd".to_string(), Value::Bytes(m1.to_vec()));
    dict.insert("_auTy".to_string(), Value::Decimal(4u8));
    let bytes = write_opack_frame(0x05, dict); // PV_Start
    println!("PV M1 bytes: {:x?}", bytes);
    stream.write_all(&bytes).await?;

    // M2: accessory ephemeral pub + encrypted (AccessoryID, Signature)
    let buf = read_frame(stream).await?;
    println!("Read PV M2 Response: {:x?}", buf.as_slice());
    if buf.first().copied() != Some(0x06) {
        println!("Expected PV_Next (0x06) for M2");
    }
    let result = process_response(buf.as_slice())?;
    check_tlv_error(&result)?;

    let mut acc_eph_bytes = None;
    let mut enc_data = None;
    for (t, bytes) in result {
        match t {
            T::PublicKey => acc_eph_bytes = Some(bytes),
            T::EncryptedData => enc_data = Some(bytes),
            _ => {}
        }
    }
    let acc_eph_bytes =
        acc_eph_bytes.ok_or_else(|| Error::msg("PV M2 missing accessory public key"))?;
    let enc_data = enc_data.ok_or_else(|| Error::msg("PV M2 missing encrypted data"))?;

    let acc_eph = PublicKey::from(
        <[u8; 32]>::try_from(acc_eph_bytes.as_slice())
            .map_err(|_| Error::msg("accessory ephemeral public key is not 32 bytes"))?,
    );
    let shared = eph.diffie_hellman(&acc_eph);

    let pv_key = hkdf_512(
        shared.as_bytes(),
        b"Pair-Verify-Encrypt-Salt",
        b"Pair-Verify-Encrypt-Info",
        32,
    );
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
    let acc_sig_bytes =
        acc_sig_bytes.ok_or_else(|| Error::msg("PV M2 sub-TLV missing signature"))?;

    if acc_id != creds.accessory_id {
        return Err(Error::msg("PV M2 accessory id does not match saved pairing"));
    }

    let acc_sig = Signature::from_bytes(
        acc_sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::msg("accessory signature is not 64 bytes"))?,
    );

    // Companion / pyatv: AccessoryEphPub || AccessoryID || ControllerEphPub
    let mut verify_material =
        Vec::with_capacity(32 + acc_id.len() + 32);
    verify_material.extend_from_slice(acc_eph.as_bytes());
    verify_material.extend_from_slice(&acc_id);
    verify_material.extend_from_slice(our_eph_pub.as_bytes());
    creds
        .accessory_ltpk
        .verify_strict(&verify_material, &acc_sig)
        .context("PV M2 accessory signature invalid")?;

    // M3: ControllerEphPub || ControllerID || AccessoryEphPub
    let mut sign_material =
        Vec::with_capacity(32 + creds.pairing_id.len() + 32);
    sign_material.extend_from_slice(our_eph_pub.as_bytes());
    sign_material.extend_from_slice(creds.pairing_id.as_bytes());
    sign_material.extend_from_slice(acc_eph.as_bytes());
    let sig = creds.our_ltsk.sign(&sign_material);

    let sub = Tlv8::new()
        .add(T::Identifier, creds.pairing_id.as_bytes())
        .add(T::Signature, sig.to_bytes().to_vec())
        .encode();
    let enc = aead.seal(&hap_nonce(b"PV-Msg03"), &sub);

    let m3 = Tlv8::new()
        .add_u8(T::SeqNum, State::M3 as u8)
        .add(T::EncryptedData, enc)
        .encode();
    let mut dict = HashMap::new();
    dict.insert("_pd".to_string(), Value::Bytes(m3.to_vec()));
    let bytes = write_opack_frame(0x06, dict); // PV_Next
    println!("PV M3 bytes: {:x?}", bytes);
    stream.write_all(&bytes).await?;

    // M4: State only (or error)
    let buf = read_frame(stream).await?;
    println!("Read PV M4 Response: {:x?}", buf.as_slice());
    if buf.first().copied() != Some(0x06) {
        println!("Expected PV_Next (0x06) for M4");
    }
    let result = process_response(buf.as_slice())?;
    check_tlv_error(&result)?;

    // Companion session keys (empty salt)
    let client_encrypt_key = hkdf_512(shared.as_bytes(), b"", b"ClientEncrypt-main", 32);
    let server_encrypt_key = hkdf_512(shared.as_bytes(), b"", b"ServerEncrypt-main", 32);
    println!(
        "Pair-Verify OK; client_key len={}, server_key len={}",
        client_encrypt_key.len(),
        server_encrypt_key.len()
    );

    Ok(VerifyResult {
        client_encrypt_key,
        server_encrypt_key,
    })
}

pub fn process_response(buf: &[u8]) -> Result<Vec<(T, Vec<u8>)>> {
    if buf.len() < 4 {
        return Err(Error::msg("response too short"));
    }
    let len = u32::from_be_bytes([0, buf[1], buf[2], buf[3]]) as usize;
    let end = 4 + len;
    if buf.len() < end {
        return Err(Error::msg("response truncated"));
    }
    let mut pos = 4;
    let mut out: Vec<(T, Vec<u8>)> = Vec::new();
    while pos < end {
        match decode(&buf[pos..end]) {
            Ok((value, consumed)) => {
                debug!("value: {:?}", value);
                if consumed == 0 {
                    return Err(Error::msg("opack decode made no progress"));
                }
                pos += consumed;
                if let Value::Dict(dict) = value {
                    for (key, v) in dict {
                        let bytes = match v {
                            Value::Bytes(b) | Value::ByteArray(b) => b,
                            _ => continue,
                        };
                        if key == "_pd" || key.is_empty() {
                            let result = Tlv8::decode(&bytes)?;
                            debug!("decoded: {:?}", result);
                            out.extend(result);
                        }
                    }
                }
            }
            Err(e) => {
                return Err(Error::msg(format!("failed to parse opack: {e:?}")));
            }
        }
    }
    Ok(out)
}

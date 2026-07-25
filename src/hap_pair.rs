use std::pin;
use std::collections::HashMap;
use anyhow::{Context, Error, Result};
use bytes::BytesMut;
use ed25519_dalek::{SigningKey, VerifyingKey, Signer};
use rand::RngCore;
use srp::client::SrpClient;
use sha2::{Sha512, Digest};
use srp::groups::G_3072;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream};
use x25519_dalek::{EphemeralSecret, PublicKey};
use tlv8::{Tlv8, T, State, Method, Error as HapError};
use opack::{Value, decode, encode};
use crate::srp::AppleTvSrp;

use crate::crypto::{hkdf_512, AeadCipher};

pub struct PairingResult {
    pub accessory_ltpk: VerifyingKey,
    pub our_ltpk: VerifyingKey,
    pub our_ltsk: SigningKey,
}

fn normalize_pin(pin: &str) -> String {
    pin.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn check_tlv_error(tlv: &[(T, Vec<u8>)]) -> Result<()> {
    for (t, bytes) in tlv {
        if *t == T::Error {
            let code = bytes.first().copied().unwrap_or(0);
            let msg = match code {
                c if c == HapError::Authentication as u8 => "authentication failed (wrong PIN or bad SRP proof)",
                c if c == HapError::Busy as u8 => "accessory is busy",
                c if c == HapError::MaxTries as u8 => "max pairing attempts exceeded",
                c if c == HapError::Unknown as u8 => "unknown pairing error",
                other => return Err(Error::msg(format!("pairing error code {other}"))),
            };
            return Err(Error::msg(msg));
        }
    }
    Ok(())
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

pub async fn pair_m3(stream: &mut TcpStream, pin: &str, salt: &[u8], public_key: &[u8]) -> Result<()> {
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
        println!("M4 server proof verified; session key len={}", srp.session_key()?.len());
    } else {
        return Err(Error::msg("Failed to read M4 response."))
    }
    Ok(())
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
                println!("value: {:?}", value);
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
                            println!("decoded: {:?}", result);
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

// Perform /pair-setup then /pair-verify per HAP (§5.6, §5.7).
// pub async fn pair_and_verify(stream: &mut TcpStream, pairing_id: &str, pin: &str) -> Result<PairingResult> {
//     // ======== M1: SRP Start Request ========
//     let m1 = Tlv8::new()
//         .add_u8(T::SeqNum, State::M1 as u8)
//         .add_u8(T::Method, Method::PairSetup as u8)
//         .encode();
//     stream.write_all(&m1).await?;
//     println!("write");
//     let mut buf = vec![0u8; 8192];
//     if let Err(_) = stream.read(&mut buf).await {
//         return Err(Error::msg("Failed to read"));
//     }
//     println!("read");
//     process_response(buf.as_slice()).await;
//     let len = u32::from_le_bytes([0, buf[1], buf[2], buf[3]]);
//     let mut pos = 4;
//     while len != pos as u32 {
//         if let Ok((value, new_pos)) = decode(&buf[pos..]) {
//             println!("value: {:?}", value);
//             pos = new_pos;
//             match value {
//                 Value::Dict(map) => {
//                     for (key, value) in map {
//                         if &key == "_pd" { // decode using TLV
//                             match value {
//                                 Value::Bytes(bytes) => {
//                                     let value = Tlv8::decode(&bytes);
//                                 },
//                                 _ => ()
//                             }
//                         }
//                     }
//                 },
//                 _ => ()
//             }
//         } else {
//             println!("failed to parse the input.");
//             break;
//         }
//     }
//     // let tlv = Tlv8::decode(&body)?;
//     // let salt = Tlv8::get(&tlv, T::Salt).context("no salt")?;
//     // let srp_pub_b = Tlv8::get(&tlv, T::PublicKey).context("no srp pub b")?;

//     // ======== M3: SRP Verify Request ========
//     // Compute SRP (controller) with user "Pair-Setup" and the code the user typed.
//     // For demo: prompt from STDIN. In your app, pass it in non-interactively.
//     let code = std::env::var("ATV_PIN").context("set ATV_PIN=123-45-678")?;
//     let pin = code.replace("-", "");

//     let client = SrpClient::<Sha512>::new(&G_3072);
//     // let verifier = client.compute_verifier("Pair-Setup".as_bytes(), pin.as_bytes(), &salt);
//     // Client computes the public A value and the clientVerifier containing the key, m1, and m2
//     let mut a = [0u8; 64];
//     rand::rngs::OsRng.try_fill_bytes(&mut a).unwrap();
//     // let client_verifier = client
//     //     .process_reply(&a, "Pair-Setup".as_bytes(), pin.as_bytes(), salt, &srp_pub_b)
//     //     .unwrap();
//     let a_pub = client.compute_public_ephemeral(&a);
//     // let client_proof = client_verifier.proof();
//     // let (key, proof_m1) = client.compute_key(&sec_a, &pub_a, srp_pub_b, salt, b"Pair-Setup", pin.as_bytes())?;

//     // let m3 = Tlv8::new()
//     //     .add_u8(T::SeqNum, State::M3 as u8)
//     //     .add(T::PublicKey, srp_pub_b)
//     //     .add(T::Proof, client_proof.to_vec())
//     //     .encode();
//     let tlv = Tlv8::decode(&body)?;
//     let m2_proof = Tlv8::get(&tlv, T::Proof).context("no accessory proof")?;
//     // client_verifier.verify_server(m2_proof).unwrap();
//     // let client_key = client_verifier.key();

//     // ======== M5: Exchange Request (Ed25519, encrypted sub-TLV) ========
//     let mut csprng = rand::rngs::OsRng{};
//     let ltsk = SigningKey::generate(&mut csprng);
//     let ltpk = ltsk.verifying_key();

//     let key = [0u8; 512];
//     let ios_x = hkdf_512(&key, "Pair-Setup-Controller-Sign-Salt".as_bytes(), "Pair-Setup-Controller-Sign-Info".as_bytes(), 32);
//     let mut sign_material = vec![];
//     sign_material.extend_from_slice(&ios_x);
//     sign_material.extend_from_slice(pairing_id.as_bytes());
//     sign_material.extend_from_slice(ltpk.as_bytes());
//     let signature = ltsk.sign(&sign_material);

//     let sub = Tlv8::new()
//         .add(T::Identifier, pairing_id.as_bytes())
//         .add(T::PublicKey, ltpk.as_bytes())
//         .add(T::Signature, signature.to_bytes().to_vec())
//         .encode();

//     let enc_key = hkdf_512(&key, "Pair-Setup-Encrypt-Salt".as_bytes(), "Pair-Setup-Encrypt-Info".as_bytes(), 32);
//     let aead = AeadCipher::new(enc_key[..32].try_into().unwrap());
//     let ciphertext = aead.seal(b"PS-Msg05    ", &sub);

//     let m5 = Tlv8::new()
//         .add_u8(T::SeqNum, State::M5 as u8)
//         .add(T::EncryptedData, ciphertext)
//         .encode();

//     // ======== M6: Accessory -> Controller, contains accessory LTPK ========
//     let tlv = Tlv8::decode(&body)?;
//     let enc = Tlv8::get(&tlv, T::EncryptedData).context("no enc data m6")?;
//     let sub = aead.open(b"PS-Msg06    ", enc);

//     let sub_tlv = Tlv8::decode(&sub)?;
//     let acc_ltpk = VerifyingKey::from_bytes(Tlv8::get(&sub_tlv, T::PublicKey).context("no acc ltpk")?.try_into()?)?;

//     // ======== Pair Verify (M1..M4) derive session keys ========
//     let eph = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
//     let pubkey = PublicKey::from(&eph);

//     // M1
//     let m1 = Tlv8::new()
//         .add_u8(T::SeqNum, State::M1 as u8)
//         .add(T::PublicKey, pubkey.as_bytes().to_vec())
//         .encode();

//     // M2: receive acc public key + enc sub-TLV
//     let acc_pub = Tlv8::get(&tlv, T::PublicKey).context("no pv acc pub")?;
//     let acc_pub = PublicKey::from(<[u8;32]>::try_from(acc_pub).unwrap());
//     let shared = eph.diffie_hellman(&acc_pub);

//     let req_key = hkdf_512(shared.as_bytes(), "Pair-Verify-Encrypt-Salt".as_bytes(), "Pair-Verify-Encrypt-Info".as_bytes(), 32);
//     let aead = AeadCipher::new(req_key[..32].try_into().unwrap());
//     let sub = aead.open(b"PV-Msg02    ", Tlv8::get(&tlv, T::EncryptedData).unwrap());
//     let sub_tlv = Tlv8::decode(&sub)?;
//     let acc_id = Tlv8::get(&sub_tlv, T::Identifier).unwrap();

//     // verify accessory signature over (acc_pub || our_pub)
//     use ed25519_dalek::Signature;
//     let acc_sig = Signature::from_bytes(Tlv8::get(&sub_tlv, T::Signature).unwrap().try_into()?);
//     let mut verify_material = vec![];
//     verify_material.extend_from_slice(acc_pub.as_bytes());
//     verify_material.extend_from_slice(pubkey.as_bytes());
//     acc_ltpk.verify_strict(&verify_material, &acc_sig).context("PV accessory signature bad")?;

//     // M3: send our signed proof
//     let ios_x = hkdf_512(shared.as_bytes(), "Pair-Verify-Controller-Sign-Salt".as_bytes(), "Pair-Verify-Controller-Sign-Info".as_bytes(), 32);
//     let mut sign_mat = vec![];
//     sign_mat.extend_from_slice(pubkey.as_bytes());
//     sign_mat.extend_from_slice(acc_pub.as_bytes());
//     let sig = ltsk.sign(&sign_mat);

//     let sub = Tlv8::new()
//         .add(T::Identifier, pairing_id.as_bytes())
//         .add(T::Signature, sig.to_bytes().to_vec())
//         .encode();
//     let enc = aead.seal(b"PV-Msg03    ", &sub);
//     let m3 = Tlv8::new()
//         .add_u8(T::SeqNum, State::M3 as u8)
//         .add(T::EncryptedData, enc)
//         .encode();

//     // M4: OK

//     Ok(PairingResult {
//         accessory_ltpk: acc_ltpk,
//         our_ltpk: ltpk,
//         our_ltsk: ltsk,
//     })
// }

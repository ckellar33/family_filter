use ed25519_dalek::{SigningKey, VerifyingKey, Signature};
use x25519_dalek::{EphemeralSecret, PublicKey};
use crate::tlv8::Tlv8;
use crate::storage::Creds;
use crate::crypto::AeadCipher;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, Context};
use hex;

pub async fn pair_verify(stream: &mut TcpStream, creds: &Creds) -> Result<()> {
    let eph = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let pubkey = PublicKey::from(&eph);

    let mut m1 = Tlv8::new();
    m1.add_u8(0x01, 1); // State M1
    m1.add(0x03, pubkey.as_bytes().to_vec());
    send_http_post(stream, "/pair-verify", &m1.encode()).await?;

    let body = read_http_body(stream).await?;
    let tlv = Tlv8::decode(&body)?;
    let acc_pub_bytes = tlv.get(0x03).context("no accessory pub")?;
    let acc_pub = PublicKey::from(<[u8;32]>::try_from(acc_pub_bytes.as_slice()).unwrap());

    let shared = eph.diffie_hellman(&acc_pub);
    let aead_key = crate::crypto::hkdf_512(
        shared.as_bytes(),
        b"Pair-Verify-Encrypt-Salt",
        b"Pair-Verify-Encrypt-Info",
        32,
    );
    let aead = AeadCipher::new(&aead_key[..32]);

    let enc_data = tlv.get(0x05).context("no encrypted data")?;
    let sub = aead.open(&[0;12], b"PV-Msg02", enc_data)?;
    let sub_tlv = Tlv8::decode(&sub)?;

    let acc_sig = Signature::from_bytes(sub_tlv.get(0x0A).unwrap().as_slice().try_into()?)?;
    let acc_ltpk = VerifyingKey::from_bytes(&hex::decode(&creds.accessory_ltpk)?)?;
    let mut material = Vec::new();
    material.extend_from_slice(acc_pub.as_bytes());
    material.extend_from_slice(pubkey.as_bytes());
    acc_ltpk.verify(&material, &acc_sig)?;

    let our_ltsk = SigningKey::from_bytes(&hex::decode(&creds.ltsk)?[..32].try_into().unwrap());
    let mut sign_material = Vec::new();
    sign_material.extend_from_slice(pubkey.as_bytes());
    sign_material.extend_from_slice(acc_pub.as_bytes());
    let sig = our_ltsk.sign(&sign_material);

    let mut sub2 = Tlv8::new();
    sub2.add(0x01, creds.pairing_id.as_bytes().to_vec());
    sub2.add(0x0A, sig.to_bytes().to_vec());
    let enc = aead.seal(&[0;12], b"PV-Msg03", &sub2.encode());

    let mut m3 = Tlv8::new();
    m3.add_u8(0x01, 3);
    m3.add(0x05, enc);
    send_http_post(stream, "/pair-verify", &m3.encode()).await?;
    Ok(())
}

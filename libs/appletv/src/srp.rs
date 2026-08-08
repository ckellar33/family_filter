//! HAP / Companion-compatible SRP-6a (SHA-512, 3072-bit group).
//!
//! The stock `srp` crate uses `M1 = H(A|B|S)` without padding. Apple HAP expects
//! RFC 5054 proofs with 384-byte padded public keys and `K = H(PAD(S))`.
//! See HAP R2 §5.5 / aiohomekit's crypto.srp.

use anyhow::{Error, Result};
use num_bigint::BigUint;
use rand::RngCore;
use sha2::{Digest, Sha512};
use srp::groups::G_3072;

const KEY_LEN: usize = 384; // 3072 / 8
const SALT_LEN: usize = 16;

fn pad_left(bytes: &[u8], len: usize) -> Vec<u8> {
    if bytes.len() >= len {
        bytes[bytes.len() - len..].to_vec()
    } else {
        let mut out = vec![0u8; len];
        out[len - bytes.len()..].copy_from_slice(bytes);
        out
    }
}

fn sha512(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = Sha512::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().to_vec()
}

/// Static k = H(N | PAD(g)) for the fixed HAP 3072-bit group.
fn compute_k() -> BigUint {
    let n = G_3072.n.to_bytes_be();
    let g = pad_left(&G_3072.g.to_bytes_be(), n.len());
    BigUint::from_bytes_be(&sha512(&[&n, &g]))
}

/// H(N) XOR H(g) with unpadded g (Apple / HAP).
fn hash_n_xor_hash_g() -> Vec<u8> {
    let h_n = sha512(&[&G_3072.n.to_bytes_be()]);
    let h_g = sha512(&[&G_3072.g.to_bytes_be()]);
    h_n.into_iter().zip(h_g).map(|(a, b)| a ^ b).collect()
}

pub struct AppleTvSrp {
    username: Vec<u8>,
    password: Vec<u8>,
    a: BigUint,
    a_pub: Vec<u8>,
    proof: Option<Vec<u8>>,
    server_proof: Option<Vec<u8>>,
    session_key: Option<Vec<u8>>,
}

impl AppleTvSrp {
    pub fn new(username: &str, pin: &str) -> Self {
        let mut a_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut a_bytes);
        let a = BigUint::from_bytes_be(&a_bytes);
        let a_pub_int = G_3072.g.modpow(&a, &G_3072.n);
        let a_pub = pad_left(&a_pub_int.to_bytes_be(), KEY_LEN);

        Self {
            username: username.as_bytes().to_vec(),
            password: pin.as_bytes().to_vec(),
            a,
            a_pub,
            proof: None,
            server_proof: None,
            session_key: None,
        }
    }

    pub fn public_key(&self) -> &[u8] {
        &self.a_pub
    }

    pub fn process_challenge(&mut self, salt: &[u8], server_public: &[u8]) -> Result<Vec<u8>> {
        let salt_b = pad_left(salt, SALT_LEN);
        let b_pub = pad_left(server_public, KEY_LEN);
        let b = BigUint::from_bytes_be(&b_pub);

        if &b % &G_3072.n == BigUint::default() {
            return Err(Error::msg("illegal server public key B"));
        }

        let identity_hash = sha512(&[&self.username, b":", &self.password]);
        let x = BigUint::from_bytes_be(&sha512(&[&salt_b, &identity_hash]));

        let u = BigUint::from_bytes_be(&sha512(&[&self.a_pub, &b_pub]));
        let k = compute_k();
        let v = G_3072.g.modpow(&x, &G_3072.n);

        // S = (B - k*v) ^ (a + u*x) mod N
        let kv = (&k * &v) % &G_3072.n;
        let base = (&G_3072.n + &b - kv) % &G_3072.n;
        let exp = &self.a + &u * &x;
        let s = base.modpow(&exp, &G_3072.n);
        let s_bytes = pad_left(&s.to_bytes_be(), KEY_LEN);

        // K = H(PAD(S))
        let session_key = sha512(&[&s_bytes]);

        // M1 = H( H(N) xor H(g) | H(I) | s | A | B | K )
        let m1 = sha512(&[
            &hash_n_xor_hash_g(),
            &sha512(&[&self.username]),
            &salt_b,
            &self.a_pub,
            &b_pub,
            &session_key,
        ]);

        // M2 = H(A | M1 | K)
        let m2 = sha512(&[&self.a_pub, &m1, &session_key]);

        self.proof = Some(m1.clone());
        self.server_proof = Some(m2);
        self.session_key = Some(session_key);

        Ok(m1)
    }

    pub fn verify_server(&self, proof: &[u8]) -> Result<()> {
        let expected = self
            .server_proof
            .as_ref()
            .ok_or_else(|| Error::msg("process_challenge() not called"))?;
        if expected.as_slice() != proof {
            return Err(Error::msg("server SRP proof mismatch"));
        }
        Ok(())
    }

    pub fn session_key(&self) -> Result<&[u8]> {
        self.session_key
            .as_deref()
            .ok_or_else(|| Error::msg("process_challenge() not called"))
    }
}

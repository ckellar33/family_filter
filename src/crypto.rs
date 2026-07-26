use hkdf::Hkdf;
use sha2::Sha512;
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};

pub fn hkdf_512(input_key: &[u8], salt: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let hk = Hkdf::<Sha512>::new(Some(salt), input_key);
    let mut okm = vec![0u8; length];
    hk.expand(info, &mut okm).unwrap();
    okm
}

/// HAP uses ChaCha20-Poly1305 with a 12-byte nonce of four zero bytes
/// followed by an 8-byte ASCII label (e.g. `PS-Msg05`).
pub fn hap_nonce(label: &[u8; 8]) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..12].copy_from_slice(label);
    nonce
}

/// Companion session nonce: 12-byte little-endian counter.
pub fn companion_nonce(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&counter.to_le_bytes());
    nonce
}

pub struct AeadCipher(ChaCha20Poly1305);

impl AeadCipher {
    pub fn new(key: &[u8]) -> Self {
        Self(ChaCha20Poly1305::new(Key::from_slice(key)))
    }

    pub fn seal(&self, nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
        self.0
            .encrypt(Nonce::from_slice(nonce), plaintext)
            .expect("AEAD encrypt")
    }

    pub fn seal_with_aad(&self, nonce: &[u8; 12], plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
        self.0
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("AEAD encrypt")
    }

    pub fn open(&self, nonce: &[u8; 12], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.0
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow::Error::msg("AEAD decrypt failed"))
    }

    pub fn open_with_aad(
        &self,
        nonce: &[u8; 12],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        self.0
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| anyhow::Error::msg("AEAD decrypt failed"))
    }
}

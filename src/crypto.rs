use hkdf::Hkdf;
use sha2::Sha512;
use aes_gcm::{Aes256Gcm, aead::{Aead, KeyInit, generic_array::GenericArray}};

pub fn hkdf_512(input_key: &[u8], salt: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let hk = Hkdf::<Sha512>::new(Some(salt), input_key);
    let mut okm = vec![0u8; length];
    hk.expand(info, &mut okm).unwrap();
    okm
}

pub struct AeadCipher(Aes256Gcm);

impl AeadCipher {
    pub fn new(key: &[u8]) -> Self { Self(Aes256Gcm::new(GenericArray::from_slice(key))) }
    pub fn seal(&self, nonce: &[u8;12], plaintext: &[u8]) -> Vec<u8> {
        self.0.encrypt(GenericArray::from_slice(nonce), plaintext).unwrap()
    }
    pub fn open(&self, nonce: &[u8;12], ciphertext: &[u8]) -> Vec<u8> {
        self.0.decrypt(GenericArray::from_slice(nonce), ciphertext).unwrap()
    }
}
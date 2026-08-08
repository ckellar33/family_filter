//! "HAP channel" chunked encryption used by AirPlay 2's control/event/data
//! connections after Pair-Verify (`pyatv.auth.hap_session.HAPSession`).
//!
//! Distinct from both other framings already in this codebase: Companion uses
//! a 4-byte frame header as AAD with no chunking (`companion.rs`); standalone
//! MRP uses no AAD and no chunking (`mrp/connection.rs`). This one chunks
//! plaintext to <=1024 bytes and prepends each chunk with a 2-byte
//! little-endian length used as the AEAD's AAD. The nonce layout (4 zero
//! bytes + little-endian 8-byte counter) matches standalone MRP's, so
//! `crypto::hap_channel_nonce` is shared with it (`mrp/connection.rs`) —
//! *not* `crypto::companion_nonce`, which pads on the other side.

use anyhow::Result;

use crate::crypto::{hap_channel_nonce, AeadCipher};

const FRAME_LENGTH: usize = 1024;
const AUTH_TAG_LEN: usize = 16;

pub struct HapChannelCipher {
    enc: AeadCipher,
    dec: AeadCipher,
    out_counter: u64,
    in_counter: u64,
}

impl HapChannelCipher {
    pub fn new(output_key: &[u8], input_key: &[u8]) -> Self {
        Self {
            enc: AeadCipher::new(output_key),
            dec: AeadCipher::new(input_key),
            out_counter: 0,
            in_counter: 0,
        }
    }

    pub fn encrypt(&mut self, mut data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + data.len() / FRAME_LENGTH * (2 + AUTH_TAG_LEN) + 2 + AUTH_TAG_LEN);
        while !data.is_empty() {
            let chunk_len = data.len().min(FRAME_LENGTH);
            let (chunk, rest) = data.split_at(chunk_len);
            data = rest;

            let len_bytes = (chunk_len as u16).to_le_bytes();
            let nonce = hap_channel_nonce(self.out_counter);
            self.out_counter += 1;
            let ciphertext = self.enc.seal_with_aad(&nonce, chunk, &len_bytes);

            out.extend_from_slice(&len_bytes);
            out.extend_from_slice(&ciphertext);
        }
        out
    }

    /// Feed newly-received ciphertext bytes into `buf` (an accumulator owned
    /// by the caller, since frames may arrive split across multiple reads)
    /// and return any newly-decrypted plaintext.
    pub fn decrypt(&mut self, buf: &mut Vec<u8>) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            if buf.len() < 2 {
                break;
            }
            let len_bytes = [buf[0], buf[1]];
            let chunk_len = u16::from_le_bytes(len_bytes) as usize;
            let block_len = chunk_len + AUTH_TAG_LEN;
            if buf.len() < 2 + block_len {
                break;
            }

            let nonce = hap_channel_nonce(self.in_counter);
            self.in_counter += 1;
            let plain = self.dec.open_with_aad(&nonce, &buf[2..2 + block_len], &len_bytes)?;
            out.extend_from_slice(&plain);
            buf.drain(..2 + block_len);
        }
        Ok(out)
    }
}

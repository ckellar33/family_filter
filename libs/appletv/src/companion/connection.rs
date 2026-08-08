//! Companion transport: a `[type:u8][len:u24 big-endian]` header followed by
//! a ChaCha20-Poly1305-sealed payload, with the header itself as additional
//! authenticated data. Frame layout differs from MRP (`mrp/connection.rs`):
//! MRP has no fixed header (just a varint length) and authenticates
//! nothing, while Companion authenticates its 4-byte header. The nonce
//! layout also differs — see `crypto::companion_nonce`.

use anyhow::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::crypto::{AeadCipher, companion_nonce};
use crate::hap_pair::VerifyResult;

pub const FRAME_E_OPACK: u8 = 0x08;
const AUTH_TAG_LEN: usize = 16;
const HEADER_LEN: usize = 4;

pub struct CompanionConnection {
    stream: TcpStream,
    enc: AeadCipher,
    dec: AeadCipher,
    out_counter: u64,
    in_counter: u64,
    read_buf: Vec<u8>,
}

impl CompanionConnection {
    pub fn new(stream: TcpStream, keys: &VerifyResult) -> Self {
        // Nagle's algorithm can hold back small writes for tens to hundreds
        // of milliseconds waiting to coalesce; that's fatal for a burst of
        // rapid-fire HID button presses like mute.
        let _ = stream.set_nodelay(true);
        Self {
            stream,
            enc: AeadCipher::new(&keys.client_encrypt_key),
            dec: AeadCipher::new(&keys.server_encrypt_key),
            out_counter: 0,
            in_counter: 0,
            read_buf: Vec::new(),
        }
    }

    pub async fn send_frame(&mut self, frame_type: u8, plaintext: &[u8]) -> Result<()> {
        let cipher_len = if plaintext.is_empty() {
            0
        } else {
            plaintext.len() + AUTH_TAG_LEN
        };
        let mut header = [0u8; HEADER_LEN];
        header[0] = frame_type; // 0x08 for OPACK
        header[1] = ((cipher_len >> 16) & 0xFF) as u8;
        header[2] = ((cipher_len >> 8) & 0xFF) as u8;
        header[3] = (cipher_len & 0xFF) as u8;

        let body = if plaintext.is_empty() {
            Vec::new()
        } else {
            let nonce = companion_nonce(self.out_counter);
            self.out_counter += 1;
            self.enc.seal_with_aad(&nonce, plaintext, &header)
        };

        self.stream.write_all(&header).await?;
        self.stream.write_all(&body).await?;
        Ok(())
    }

    pub async fn recv_frame(&mut self) -> Result<Vec<u8>> {
        loop {
            if self.read_buf.len() >= HEADER_LEN {
                let payload_len =
                    u32::from_be_bytes([0, self.read_buf[1], self.read_buf[2], self.read_buf[3]])
                        as usize;
                let total = HEADER_LEN + payload_len;
                if self.read_buf.len() >= total {
                    let header: [u8; 4] = self.read_buf[..HEADER_LEN].try_into().unwrap();
                    let ciphertext = self.read_buf[HEADER_LEN..total].to_vec();
                    self.read_buf.drain(..total);

                    if ciphertext.is_empty() {
                        return Ok(Vec::new());
                    }
                    let nonce = companion_nonce(self.in_counter);
                    self.in_counter += 1;
                    return self.dec.open_with_aad(&nonce, &ciphertext, &header);
                }
            }

            let mut tmp = [0u8; 8192];
            let n = self
                .stream
                .read(&mut tmp)
                .await
                .map_err(|_| Error::msg("connection closed while reading frame"))?;
            if n == 0 {
                return Err(Error::msg("connection closed while reading frame"));
            }
            self.read_buf.extend_from_slice(&tmp[..n]);
        }
    }
}

//! MRP transport: protobuf varint-length-prefixed frames over TCP, with
//! ChaCha20-Poly1305 encryption enabled after Pair-Verify.
//!
//! Frame layout differs from Companion (`companion.rs`): no fixed header, just
//! `[varint length][ciphertext]`, and the AEAD has no additional authenticated
//! data (Companion authenticates its 4-byte header; MRP authenticates nothing).
//! The nonce layout (4 zero bytes + little-endian 8-byte counter) matches
//! the AirPlay HAP channels' (`airplay/hap_channel.rs`), so
//! `crypto::hap_channel_nonce` is shared with those — *not*
//! `crypto::companion_nonce`, which pads on the other side.

use std::time::Duration;

use anyhow::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::crypto::{hap_channel_nonce, AeadCipher};

/// Timeout for request/response exchanges during handshake and pairing.
/// Not used for `recv()` itself, since the post-handshake push-listening
/// loop (`MrpSession::recv_update`) legitimately waits indefinitely for the
/// device's next unsolicited `SetStateMessage`.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

struct Cipher {
    enc: AeadCipher,
    dec: AeadCipher,
    out_counter: u64,
    in_counter: u64,
}

pub struct MrpConnection {
    stream: TcpStream,
    read_buf: Vec<u8>,
    cipher: Option<Cipher>,
}

impl MrpConnection {
    pub fn new(stream: TcpStream) -> Self {
        let _ = stream.set_nodelay(true);
        Self {
            stream,
            read_buf: Vec::new(),
            cipher: None,
        }
    }

    /// Enable encryption after Pair-Verify. `output_key` encrypts what we
    /// send, `input_key` decrypts what we receive (matches pyatv's naming).
    pub fn enable_encryption(&mut self, output_key: &[u8], input_key: &[u8]) {
        self.cipher = Some(Cipher {
            enc: AeadCipher::new(output_key),
            dec: AeadCipher::new(input_key),
            out_counter: 0,
            in_counter: 0,
        });
    }

    pub async fn send(&mut self, plaintext: &[u8]) -> Result<()> {
        let payload = match self.cipher.as_mut() {
            Some(cipher) => {
                let nonce = hap_channel_nonce(cipher.out_counter);
                cipher.out_counter += 1;
                cipher.enc.seal(&nonce, plaintext)
            }
            None => plaintext.to_vec(),
        };
        let mut frame = protobuf_lite::write_varint(payload.len() as u64);
        frame.extend_from_slice(&payload);
        self.stream.write_all(&frame).await?;
        Ok(())
    }

    /// Wait for the next frame, bounded by `timeout` — use this for
    /// request/response exchanges (handshake, pairing) where a hung
    /// or crashed peer should surface as an error instead of an
    /// indefinite silent hang.
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        tokio::time::timeout(timeout, self.recv())
            .await
            .map_err(|_| Error::msg(format!("timed out after {timeout:?} waiting for MRP frame")))?
    }

    /// Wait for the next frame with no timeout — used for the post-handshake
    /// push-listening loop, which may legitimately idle for a long time.
    pub async fn recv(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Ok((len, consumed)) = protobuf_lite::read_varint(&self.read_buf) {
                let len = len as usize;
                if self.read_buf.len() >= consumed + len {
                    let payload: Vec<u8> = self.read_buf[consumed..consumed + len].to_vec();
                    self.read_buf.drain(..consumed + len);
                    let plain = match self.cipher.as_mut() {
                        Some(cipher) => {
                            let nonce = hap_channel_nonce(cipher.in_counter);
                            cipher.in_counter += 1;
                            cipher.dec.open(&nonce, &payload)?
                        }
                        None => payload,
                    };
                    return Ok(plain);
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

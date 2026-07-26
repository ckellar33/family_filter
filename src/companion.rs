//! Companion encrypted session and media-control commands after Pair-Verify.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Error, Result};
use opack::{Value, decode, encode};
use rand::Rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::crypto::{AeadCipher, companion_nonce};
use crate::hap_pair::VerifyResult;

const FRAME_E_OPACK: u8 = 0x08;
const AUTH_TAG_LEN: usize = 16;
const HEADER_LEN: usize = 4;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);

const MCC_GET_VOLUME: i64 = 5;
const MCC_SET_VOLUME: i64 = 6;
const MCC_SKIP_BY: i64 = 7;

const MSG_EVENT: i64 = 1;
const MSG_REQUEST: i64 = 2;
const MSG_RESPONSE: i64 = 3;

fn map_of(entries: &[(&str, Value)]) -> HashMap<String, Value> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

pub struct CompanionSession {
    stream: TcpStream,
    enc: AeadCipher,
    dec: AeadCipher,
    out_counter: u64,
    in_counter: u64,
    xid: i64,
    read_buf: Vec<u8>,
    /// Volume level (0.0–1.0) captured before the last mute.
    pre_mute_volume: Option<f64>,
}

impl CompanionSession {
    pub fn new(stream: TcpStream, keys: VerifyResult) -> Self {
        let mut rng = rand::thread_rng();
        Self {
            stream,
            enc: AeadCipher::new(&keys.client_encrypt_key),
            dec: AeadCipher::new(&keys.server_encrypt_key),
            out_counter: 0,
            in_counter: 0,
            xid: rng.gen_range(0..0xFFFF),
            read_buf: Vec::new(),
            pre_mute_volume: None,
        }
    }

    /// Bootstrap a media-control session (systemInfo → sessionStart → interest).
    pub async fn bootstrap(&mut self, pairing_id: &str) -> Result<()> {
        let system_info = map_of(&[
            ("_i", Value::Str("_systemInfo".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            (
                "_c",
                Value::Dict(map_of(&[
                    ("_bf", Value::Int(0)),
                    ("_cf", Value::Int(512)),
                    ("_clFl", Value::Int(128)),
                    ("_i", Value::Str(pairing_id.to_string())),
                    ("_idsID", Value::Str(pairing_id.to_string())),
                    ("_pubID", Value::Str(pairing_id.to_string())),
                    ("_sf", Value::Int(256)),
                    ("_sv", Value::Str("170.18".into())),
                    ("model", Value::Str("family-filter".into())),
                    ("name", Value::Str("family-filter".into())),
                ])),
            ),
        ]);
        self.exchange_opack(system_info)
            .await
            .context("_systemInfo failed")?;

        let local_sid: i64 = rand::thread_rng().gen_range(0..i64::from(u32::MAX));
        let session_start = map_of(&[
            ("_i", Value::Str("_sessionStart".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            (
                "_c",
                Value::Dict(map_of(&[
                    ("_srvT", Value::Str("com.apple.tvremoteservices".into())),
                    ("_sid", Value::Int(local_sid)),
                ])),
            ),
        ]);
        let resp = self
            .exchange_opack(session_start)
            .await
            .context("_sessionStart failed")?;

        if let Some(Value::Dict(c)) = resp.get("_c") {
            if let Some(Value::Int(remote_sid)) = c.get("_sid") {
                println!(
                    "Session started; local_sid={local_sid:#x} remote_sid={remote_sid:#x}"
                );
            }
        }

        // Interest is an event (_t=1); no response is expected.
        let interest = map_of(&[
            ("_i", Value::Str("_interest".into())),
            ("_t", Value::Int(MSG_EVENT)),
            (
                "_c",
                Value::Dict(map_of(&[(
                    "_regEvents",
                    Value::List(vec![Value::Str("_iMC".into())]),
                )])),
            ),
        ]);
        self.send_opack(interest)
            .await
            .context("_interest failed")?;

        Ok(())
    }

    /// Current volume as 0.0–1.0 from GetVolume.
    pub async fn get_volume(&mut self) -> Result<f64> {
        let msg = map_of(&[
            ("_i", Value::Str("_mcc".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            (
                "_c",
                Value::Dict(map_of(&[("_mcc", Value::Int(MCC_GET_VOLUME))])),
            ),
        ]);
        let resp = self
            .exchange_opack(msg)
            .await
            .context("GetVolume failed")?;
        let vol = resp
            .get("_c")
            .and_then(|c| match c {
                Value::Dict(d) => d.get("_vol"),
                _ => None,
            })
            .and_then(|v| match v {
                Value::Float(f) => Some(*f),
                Value::Int(i) => Some(*i as f64),
                _ => None,
            })
            .ok_or_else(|| Error::msg("GetVolume response missing _vol"))?;
        Ok(vol)
    }

    /// Save the current volume (if non-zero) and set volume to 0.
    pub async fn mute(&mut self) -> Result<()> {
        match self.get_volume().await {
            Ok(vol) if vol > 0.0 => {
                self.pre_mute_volume = Some(vol);
                println!("Saved volume {:.0}% before mute", vol * 100.0);
            }
            Ok(_) => {
                // Already muted; keep any previously saved level.
            }
            Err(e) => {
                // Still mute; unmute may fall back if nothing was saved.
                println!("Could not read volume before mute: {e}");
            }
        }
        self.set_volume(0.0).await
    }

    /// Restore volume saved by the last mute.
    pub async fn unmute(&mut self) -> Result<()> {
        let Some(vol) = self.pre_mute_volume.take() else {
            return Err(Error::msg(
                "no saved volume to restore (mute first while audio is playing)",
            ));
        };
        self.set_volume(vol).await?;
        println!("Restored volume to {:.0}%", vol * 100.0);
        Ok(())
    }

    pub async fn set_volume(&mut self, level: f64) -> Result<()> {
        // Companion expects 0.0–1.0; callers may pass 0.0–100.0 percent.
        let vol = if level > 1.0 { level / 100.0 } else { level };
        let msg = map_of(&[
            ("_i", Value::Str("_mcc".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            (
                "_c",
                Value::Dict(map_of(&[
                    ("_mcc", Value::Int(MCC_SET_VOLUME)),
                    ("_vol", Value::Float(vol)),
                ])),
            ),
        ]);
        let _ = self
            .exchange_opack(msg)
            .await
            .context("SetVolume failed")?;
        Ok(())
    }

    /// Skip by `seconds` (positive = forward, negative = backward).
    pub async fn skip(&mut self, seconds: f64) -> Result<()> {
        let msg = map_of(&[
            ("_i", Value::Str("_mcc".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            (
                "_c",
                Value::Dict(map_of(&[
                    ("_mcc", Value::Int(MCC_SKIP_BY)),
                    ("_skpS", Value::Float(seconds)),
                ])),
            ),
        ]);
        let _ = self.exchange_opack(msg).await.context("SkipBy failed")?;
        Ok(())
    }

    async fn exchange_opack(
        &mut self,
        mut data: HashMap<String, Value>,
    ) -> Result<HashMap<String, Value>> {
        let xid = self.next_xid();
        data.insert("_x".into(), Value::Int(xid));
        self.send_opack(data).await?;

        let deadline = tokio::time::Instant::now() + EXCHANGE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::msg("timed out waiting for OPACK response"));
            }
            let frame = timeout(remaining, self.recv_frame())
                .await
                .map_err(|_| Error::msg("timed out waiting for OPACK response"))??;
            let (value, _) =
                decode(&frame).map_err(|e| Error::msg(format!("opack decode: {e}")))?;
            let Value::Dict(dict) = value else {
                continue;
            };
            match dict.get("_t") {
                Some(Value::Int(t)) if *t == MSG_EVENT => continue,
                Some(Value::Int(t)) if *t == MSG_RESPONSE => {
                    if let Some(Value::Str(em)) = dict.get("_em") {
                        return Err(Error::msg(format!("command failed: {em}")));
                    }
                    match dict.get("_x") {
                        Some(Value::Int(x)) if *x == xid => return Ok(dict),
                        _ => continue,
                    }
                }
                _ => continue,
            }
        }
    }

    async fn send_opack(&mut self, mut data: HashMap<String, Value>) -> Result<()> {
        if !data.contains_key("_x") {
            let xid = self.next_xid();
            data.insert("_x".into(), Value::Int(xid));
        }
        let payload =
            encode(&Value::Dict(data)).map_err(|e| Error::msg(format!("opack encode: {e}")))?;
        self.send_frame(FRAME_E_OPACK, &payload).await
    }

    fn next_xid(&mut self) -> i64 {
        let xid = self.xid;
        self.xid = self.xid.wrapping_add(1);
        xid
    }

    async fn send_frame(&mut self, frame_type: u8, plaintext: &[u8]) -> Result<()> {
        let cipher_len = if plaintext.is_empty() {
            0
        } else {
            plaintext.len() + AUTH_TAG_LEN
        };
        let mut header = [0u8; HEADER_LEN];
        header[0] = frame_type;
        let len_be = (cipher_len as u32).to_be_bytes();
        header[1..4].copy_from_slice(&len_be[1..4]);

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

    async fn recv_frame(&mut self) -> Result<Vec<u8>> {
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

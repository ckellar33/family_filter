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
}

impl CompanionSession {
    pub fn new(stream: TcpStream, keys: VerifyResult) -> Self {
        // Nagle's algorithm can hold back small writes for tens to hundreds
        // of milliseconds waiting to coalesce; that's fatal for a burst of
        // rapid-fire HID button presses like mute.
        let _ = stream.set_nodelay(true);
        let mut rng = rand::thread_rng();
        Self {
            stream,
            enc: AeadCipher::new(&keys.client_encrypt_key),
            dec: AeadCipher::new(&keys.server_encrypt_key),
            out_counter: 0,
            in_counter: 0,
            xid: rng.gen_range(0..0xFFFF),
            read_buf: Vec::new(),
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

        // Real Apple TVs require a touch session before _sessionStart will
        // succeed; fake_device doesn't enforce this ordering (pyatv api.py
        // calls _touch_start() between system_info() and _session_start()).
        let touch_start = map_of(&[
            ("_i", Value::Str("_touchStart".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            (
                "_c",
                Value::Dict(map_of(&[
                    ("_height", Value::Float(1000.0)),
                    ("_tFl", Value::Int(0)),
                    ("_width", Value::Float(1000.0)),
                ])),
            ),
        ]);
        self.exchange_opack(touch_start)
            .await
            .context("_touchStart failed")?;

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

        // Best-effort: not all devices support a TV Remote Client session.
        let tv_rc_session_start = map_of(&[
            ("_i", Value::Str("TVRCSessionStart".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            (
                "_c",
                Value::Dict(map_of(&[(
                    "ProtocolVersionKey",
                    Value::Str("1.2".into()),
                )])),
            ),
        ]);
        if let Err(e) = self.exchange_opack(tv_rc_session_start).await {
            println!("TVRCSessionStart not supported: {e}");
        }

        let text_input_start = map_of(&[
            ("_i", Value::Str("_tiStart".into())),
            ("_t", Value::Int(MSG_REQUEST)),
            ("_c", Value::Dict(HashMap::new())),
        ]);
        self.exchange_opack(text_input_start)
            .await
            .context("_tiStart failed")?;

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
            // println!("exchange_opack frame: {:?}", frame);
            let (value, _) = decode(&frame).map_err(|e| Error::msg(format!("opack decode: {e}")))?;
            // println!("decoded frame: {value:?}");
            let Value::Dict(dict) = value else {
                continue;
            };
            // println!("  _t={:?} _i={:?} _x={:?}", dict.get("_t"), dict.get("_i"), dict.get("_x"));

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
        // println!("{:?}", payload);
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
        // header[0] = frame_type;
        // let len_be = (cipher_len as u32).to_be_bytes();
        // header[1..4].copy_from_slice(&len_be[1..4]);
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

//! Companion encrypted session and media-control commands after Pair-Verify.

use std::collections::HashMap;

use anyhow::{Context, Error, Result};
use opack::{Value, decode, encode};
use rand::Rng;
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

use crate::hap_pair::VerifyResult;

use super::connection::{CompanionConnection, FRAME_E_OPACK};
use super::messages::{self, MSG_EVENT, MSG_RESPONSE};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct CompanionSession {
    conn: CompanionConnection,
    xid: i64,
}

impl CompanionSession {
    pub fn new(stream: TcpStream, keys: VerifyResult) -> Self {
        let mut rng = rand::thread_rng();
        Self {
            conn: CompanionConnection::new(stream, &keys),
            xid: rng.gen_range(0..0xFFFF),
        }
    }

    /// Bootstrap a media-control session (systemInfo → sessionStart → interest).
    pub async fn bootstrap(&mut self, pairing_id: &str) -> Result<()> {
        self.exchange_opack(messages::system_info_message(pairing_id))
            .await
            .context("_systemInfo failed")?;

        self.exchange_opack(messages::touch_start_message())
            .await
            .context("_touchStart failed")?;

        let local_sid: i64 = rand::thread_rng().gen_range(0..i64::from(u32::MAX));
        let resp = self
            .exchange_opack(messages::session_start_message(local_sid))
            .await
            .context("_sessionStart failed")?;

        if let Some(Value::Dict(c)) = resp.get("_c") {
            if let Some(Value::Int(remote_sid)) = c.get("_sid") {
                println!(
                    "Session started; local_sid={local_sid:#x} remote_sid={remote_sid:#x}"
                );
            }
        }

        if let Err(e) = self.exchange_opack(messages::tv_rc_session_start_message()).await {
            println!("TVRCSessionStart not supported: {e}");
        }

        self.exchange_opack(messages::text_input_start_message())
            .await
            .context("_tiStart failed")?;

        // Interest is an event (_t=1); no response is expected.
        self.send_opack(messages::interest_message())
            .await
            .context("_interest failed")?;

        Ok(())
    }

    /// Skip by `seconds` (positive = forward, negative = backward).
    pub async fn skip(&mut self, seconds: f64) -> Result<()> {
        let _ = self
            .exchange_opack(messages::skip_by_message(seconds))
            .await
            .context("SkipBy failed")?;
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
            let frame = timeout(remaining, self.conn.recv_frame())
                .await
                .map_err(|_| Error::msg("timed out waiting for OPACK response"))??;
            let (value, _) = decode(&frame).map_err(|e| Error::msg(format!("opack decode: {e}")))?;
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
        self.conn.send_frame(FRAME_E_OPACK, &payload).await
    }

    fn next_xid(&mut self) -> i64 {
        let xid = self.xid;
        self.xid = self.xid.wrapping_add(1);
        xid
    }
}

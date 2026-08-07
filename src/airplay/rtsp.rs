//! Minimal RTSP request/response framing used for AirPlay 2's control
//! connection (`pyatv.support.http` / `pyatv.support.rtsp`). Text-based,
//! HTTP/1.1-like: `METHOD uri RTSP/1.0\r\nHeader: value\r\n...\r\n\r\n<body>`,
//! response `RTSP/1.0 CODE MESSAGE\r\n...`, body length from `Content-Length`.
//!
//! Only one request is ever in flight at a time in this client (unlike
//! pyatv's async multi-outstanding-request dispatcher), so there's no need
//! for CSeq-based response matching — send, then read exactly one response.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Error, Result};
use rand::Rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::hap_channel::HapChannelCipher;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct RtspResponse {
    pub code: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

fn format_request(method: &str, uri: &str, headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
    let mut msg = format!("{method} {uri} RTSP/1.0");
    for (k, v) in headers {
        msg.push_str(&format!("\r\n{k}: {v}"));
    }
    msg.push_str("\r\n\r\n");
    let mut out = msg.into_bytes();
    out.extend_from_slice(body);
    out
}

pub(super) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Try to parse one response out of `buf`. Returns `(response, bytes_consumed)`
/// or `None` if more data is needed.
fn try_parse_response(buf: &[u8]) -> Result<Option<(RtspResponse, usize)>> {
    let Some(header_end) = find_subslice(buf, b"\r\n\r\n") else {
        return Ok(None);
    };
    let header_str = std::str::from_utf8(&buf[..header_end]).map_err(|_| Error::msg("RTSP response headers not UTF-8"))?;
    let mut lines = header_str.split("\r\n");
    let status_line = lines.next().ok_or_else(|| Error::msg("empty RTSP response"))?;

    // e.g. "RTSP/1.0 200 OK"
    let mut parts = status_line.splitn(3, ' ');
    parts.next().ok_or_else(|| Error::msg("bad RTSP status line"))?;
    let code: u16 = parts
        .next()
        .ok_or_else(|| Error::msg("bad RTSP status line"))?
        .parse()
        .map_err(|_| Error::msg("bad RTSP status code"))?;

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(": ") {
            headers.insert(k.to_ascii_lowercase(), v.to_string());
        }
    }

    let content_length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let body_start = header_end + 4;
    if buf.len() < body_start + content_length {
        return Ok(None);
    }
    let body = buf[body_start..body_start + content_length].to_vec();

    Ok(Some((RtspResponse { code, headers, body }, body_start + content_length)))
}

pub struct RtspConnection {
    stream: TcpStream,
    /// Raw bytes read off the socket but not yet decrypted (only used once a
    /// cipher is enabled; before that, reads go straight into `plain_buf`).
    enc_buf: Vec<u8>,
    /// Plaintext RTSP bytes ready to be parsed as a response.
    plain_buf: Vec<u8>,
    cipher: Option<HapChannelCipher>,
    cseq: u32,
    dacp_id: String,
    active_remote: u32,
    session_id: u32,
    local_ip: std::net::IpAddr,
}

impl RtspConnection {
    pub async fn connect(host: &str, port: u16) -> Result<Self> {
        let stream = TcpStream::connect(format!("{host}:{port}")).await?;
        let _ = stream.set_nodelay(true);
        let local_ip = stream.local_addr()?.ip();
        let mut rng = rand::thread_rng();
        Ok(Self {
            stream,
            enc_buf: Vec::new(),
            plain_buf: Vec::new(),
            cipher: None,
            cseq: 0,
            dacp_id: format!("{:016X}", rng.r#gen::<u64>()),
            active_remote: rng.r#gen(),
            session_id: rng.r#gen(),
            local_ip,
        })
    }

    /// Default request URI: `rtsp://<local-ip>/<session-id>`, matching pyatv.
    pub fn default_uri(&self) -> String {
        format!("rtsp://{}/{}", self.local_ip, self.session_id)
    }

    /// Enable HAP-channel encryption on this connection (call once, right
    /// after Pair-Verify succeeds).
    pub fn enable_encryption(&mut self, output_key: &[u8], input_key: &[u8]) {
        self.cipher = Some(HapChannelCipher::new(output_key, input_key));
    }

    pub async fn send_and_receive(
        &mut self,
        method: &str,
        uri: Option<&str>,
        extra_headers: &[(String, String)],
        body: &[u8],
    ) -> Result<RtspResponse> {
        let cseq = self.cseq;
        self.cseq += 1;
        let uri = uri.map(str::to_string).unwrap_or_else(|| self.default_uri());

        let mut headers = vec![
            ("CSeq".to_string(), cseq.to_string()),
            ("DACP-ID".to_string(), self.dacp_id.clone()),
            ("Active-Remote".to_string(), self.active_remote.to_string()),
            ("Client-Instance".to_string(), self.dacp_id.clone()),
            ("User-Agent".to_string(), "AirPlay/550.10".to_string()),
        ];
        // Let extra_headers override defaults (e.g. pairing uses a
        // different User-Agent) instead of emitting duplicate header lines.
        for (k, v) in extra_headers {
            if let Some(existing) = headers.iter_mut().find(|(hk, _)| hk.eq_ignore_ascii_case(k)) {
                existing.1 = v.clone();
            } else {
                headers.push((k.clone(), v.clone()));
            }
        }
        if !body.is_empty() {
            headers.push(("Content-Length".to_string(), body.len().to_string()));
        }

        let request = format_request(method, &uri, &headers, body);
        self.send_raw(&request).await?;
        tokio::time::timeout(RESPONSE_TIMEOUT, self.recv_response())
            .await
            .map_err(|_| Error::msg(format!("timed out after {RESPONSE_TIMEOUT:?} waiting for RTSP response to {method} {uri}")))?
    }

    async fn send_raw(&mut self, data: &[u8]) -> Result<()> {
        let out = match self.cipher.as_mut() {
            Some(cipher) => cipher.encrypt(data),
            None => data.to_vec(),
        };
        self.stream.write_all(&out).await?;
        Ok(())
    }

    async fn recv_response(&mut self) -> Result<RtspResponse> {
        let mut total_read = 0usize;
        loop {
            if let Some((resp, consumed)) = try_parse_response(&self.plain_buf)? {
                self.plain_buf.drain(..consumed);
                return Ok(resp);
            }

            let mut tmp = [0u8; 8192];
            let n = self
                .stream
                .read(&mut tmp)
                .await
                .map_err(|e| Error::msg(format!("RTSP read error after {total_read} bytes: {e}")))?;
            if n == 0 {
                return Err(Error::msg(format!(
                    "connection closed while reading RTSP response (received {total_read} bytes total first)"
                )));
            }
            total_read += n;

            match self.cipher.as_mut() {
                Some(cipher) => {
                    self.enc_buf.extend_from_slice(&tmp[..n]);
                    let plain = cipher.decrypt(&mut self.enc_buf)?;
                    self.plain_buf.extend_from_slice(&plain);
                }
                None => self.plain_buf.extend_from_slice(&tmp[..n]),
            }
        }
    }
}

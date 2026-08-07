//! AirPlay 2 event/data channels: separate TCP connections (beyond the main
//! RTSP control connection), each HAP-channel-encrypted with keys derived
//! from the same Pair-Verify shared secret (`pyatv/protocols/airplay/channels.py`).
//!
//! The event channel's content is unused (it just needs to exist and reply
//! `200 OK` to whatever the device sends). The data channel carries our
//! existing `mrp::messages`-encoded `ProtocolMessage` bytes, wrapped in a
//! small binary `DataFrame` header plus a bplist envelope.

use anyhow::{Error, Result};
use rand::Rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::hap_channel::HapChannelCipher;
use super::rtsp::find_subslice;

/// `size:u32 message_type:[u8;12] command:[u8;4] seqno:u64 padding:u32`, all
/// big-endian (`pyatv.support.packet.defpacket` uses `>` = big-endian).
const DATA_HEADER_LEN: usize = 32;

const MSG_TYPE_SYNC: [u8; 12] = *b"sync\0\0\0\0\0\0\0\0";
const MSG_TYPE_RPLY: [u8; 12] = *b"rply\0\0\0\0\0\0\0\0";
const CMD_COMM: [u8; 4] = *b"comm";
const CMD_ZERO: [u8; 4] = [0, 0, 0, 0];

struct HapChannel {
    stream: TcpStream,
    enc_buf: Vec<u8>,
    plain_buf: Vec<u8>,
    cipher: HapChannelCipher,
}

impl HapChannel {
    async fn connect(host: &str, port: u16, output_key: &[u8], input_key: &[u8]) -> Result<Self> {
        let stream = TcpStream::connect((host, port)).await?;
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream,
            enc_buf: Vec::new(),
            plain_buf: Vec::new(),
            cipher: HapChannelCipher::new(output_key, input_key),
        })
    }

    async fn send(&mut self, data: &[u8]) -> Result<()> {
        let out = self.cipher.encrypt(data);
        self.stream.write_all(&out).await?;
        Ok(())
    }

    /// Read more bytes off the socket and decrypt into `plain_buf`. Returns
    /// `false` if the connection closed.
    async fn poll_recv(&mut self) -> Result<bool> {
        let mut tmp = [0u8; 8192];
        let n = self.stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(false);
        }
        self.enc_buf.extend_from_slice(&tmp[..n]);
        let plain = self.cipher.decrypt(&mut self.enc_buf)?;
        self.plain_buf.extend_from_slice(&plain);
        Ok(true)
    }
}

fn encode_data_header(size: u32, message_type: &[u8; 12], command: &[u8; 4], seqno: u64, padding: u32) -> [u8; DATA_HEADER_LEN] {
    let mut out = [0u8; DATA_HEADER_LEN];
    out[0..4].copy_from_slice(&size.to_be_bytes());
    out[4..16].copy_from_slice(message_type);
    out[16..20].copy_from_slice(command);
    out[20..28].copy_from_slice(&seqno.to_be_bytes());
    out[28..32].copy_from_slice(&padding.to_be_bytes());
    out
}

struct DataFrameHeader {
    size: u32,
    message_type: [u8; 12],
    seqno: u64,
}

fn decode_data_header(buf: &[u8]) -> Option<DataFrameHeader> {
    if buf.len() < DATA_HEADER_LEN {
        return None;
    }
    Some(DataFrameHeader {
        size: u32::from_be_bytes(buf[0..4].try_into().unwrap()),
        message_type: buf[4..16].try_into().unwrap(),
        seqno: u64::from_be_bytes(buf[20..28].try_into().unwrap()),
    })
}

fn encode_mrp_bplist(mrp_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut inner = plist::Dictionary::new();
    inner.insert("data".to_string(), plist::Value::Data(mrp_bytes.to_vec()));
    let mut params = plist::Dictionary::new();
    params.insert("params".to_string(), plist::Value::Dictionary(inner));

    let mut payload = Vec::new();
    plist::Value::Dictionary(params)
        .to_writer_binary(&mut payload)
        .map_err(|e| Error::msg(format!("bplist encode: {e}")))?;
    Ok(payload)
}

fn decode_mrp_bplist(payload: &[u8]) -> Result<Option<Vec<u8>>> {
    let value = plist::Value::from_reader(std::io::Cursor::new(payload)).map_err(|e| Error::msg(format!("bplist decode: {e}")))?;
    Ok(value
        .as_dictionary()
        .and_then(|d| d.get("params"))
        .and_then(|v| v.as_dictionary())
        .and_then(|d| d.get("data"))
        .and_then(|v| v.as_data())
        .map(|d| d.to_vec()))
}

/// The `data` field of a tunneled frame is a *stream* of one or more
/// `ProtocolMessage`s, not a single message: normally each is
/// varint-length-prefixed, but the device is known to send at least
/// `ConfigureConnectionMessage` raw/unprefixed — detectable because every
/// `ProtocolMessage` starts with the `type` field's tag byte `0x08` (field 1,
/// varint), which can never be a valid length prefix for a message that
/// small (`pyatv.protocols.airplay.channels.decode_protobufs`).
fn split_protocol_messages(mut data: &[u8]) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    while !data.is_empty() {
        if data[0] == 0x08 {
            messages.push(data.to_vec());
            break;
        }
        let Ok((length, consumed)) = protobuf_lite::read_varint(data) else { break };
        let length = length as usize;
        if data.len() < consumed + length {
            break;
        }
        messages.push(data[consumed..consumed + length].to_vec());
        data = &data[consumed + length..];
    }
    messages
}

/// The data channel: carries MRP `ProtocolMessage` bytes (as built/parsed by
/// `mrp::messages`) tunneled through AirPlay.
pub struct DataChannel {
    channel: HapChannel,
    send_seqno: u64,
    pending: std::collections::VecDeque<Vec<u8>>,
}

impl DataChannel {
    pub async fn connect(host: &str, port: u16, output_key: &[u8], input_key: &[u8]) -> Result<Self> {
        let channel = HapChannel::connect(host, port, output_key, input_key).await?;
        let send_seqno = rand::thread_rng().gen_range(0x1_0000_0000u64..0x1_FFFF_FFFFu64);
        Ok(Self { channel, send_seqno, pending: std::collections::VecDeque::new() })
    }

    /// Send raw MRP `ProtocolMessage` bytes (as built by `mrp::messages::wrap`).
    /// Varint-length-prefixed within the `data` field, matching pyatv's
    /// `encode_protobufs` — a *different* length prefix than the outer
    /// transport framing standalone `MrpConnection` uses.
    pub async fn send_mrp(&mut self, mrp_bytes: &[u8]) -> Result<()> {
        let mut data = protobuf_lite::write_varint(mrp_bytes.len() as u64);
        data.extend_from_slice(mrp_bytes);
        let payload = encode_mrp_bplist(&data)?;
        let seqno = self.send_seqno;
        self.send_seqno += 1;
        let header = encode_data_header((DATA_HEADER_LEN + payload.len()) as u32, &MSG_TYPE_SYNC, &CMD_COMM, seqno, 0);
        let mut frame = header.to_vec();
        frame.extend_from_slice(&payload);
        self.channel.send(&frame).await
    }

    /// Block until the next MRP `ProtocolMessage` bytes arrive, replying to
    /// `sync` frames as required by the protocol along the way. Mirrors
    /// `mrp::connection::MrpConnection::recv` (blocks indefinitely; used both
    /// for handshake request/response and the post-handshake push loop).
    pub async fn recv_mrp(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Some(msg) = self.pending.pop_front() {
                return Ok(msg);
            }

            if let Some(header) = decode_data_header(&self.channel.plain_buf) {
                let total = header.size as usize;
                if self.channel.plain_buf.len() >= total {
                    let payload: Vec<u8> = self.channel.plain_buf[DATA_HEADER_LEN..total].to_vec();
                    self.channel.plain_buf.drain(..total);
                    println!(
                        "AirPlay: data channel frame type={:?} seqno={} payload={} bytes",
                        String::from_utf8_lossy(&header.message_type).trim_end_matches('\0'),
                        header.seqno,
                        payload.len()
                    );

                    if header.message_type[..4] == *b"sync" {
                        let reply = encode_data_header(DATA_HEADER_LEN as u32, &MSG_TYPE_RPLY, &CMD_ZERO, header.seqno, 0);
                        self.channel.send(&reply).await?;
                    }

                    if payload.is_empty() {
                        continue;
                    }
                    if let Some(mrp_bytes) = decode_mrp_bplist(&payload)? {
                        let split = split_protocol_messages(&mrp_bytes);
                        let types: Vec<String> = split
                            .iter()
                            .map(|m| {
                                protobuf_lite::decode(m)
                                    .ok()
                                    .and_then(|f| protobuf_lite::last_field(&f, 1).and_then(protobuf_lite::WireValue::as_i64))
                                    .map(|t| t.to_string())
                                    .unwrap_or_else(|| "?".to_string())
                            })
                            .collect();
                        println!(
                            "AirPlay: data channel 'data' field = {} bytes, split into {} message(s), types=[{}]",
                            mrp_bytes.len(),
                            split.len(),
                            types.join(", ")
                        );
                        self.pending.extend(split);
                    }
                    continue;
                }
            }

            if !self.channel.poll_recv().await? {
                return Err(Error::msg("AirPlay data channel connection closed"));
            }
        }
    }
}

/// The event channel: content is unused by this client, but the connection
/// must exist and reply `200 OK` to anything the device sends on it.
pub struct EventChannel {
    channel: HapChannel,
}

impl EventChannel {
    pub async fn connect(host: &str, port: u16, output_key: &[u8], input_key: &[u8]) -> Result<Self> {
        Ok(Self {
            channel: HapChannel::connect(host, port, output_key, input_key).await?,
        })
    }

    /// Run forever, acknowledging anything received. Intended to be spawned
    /// as a background task and left running for the life of the session.
    pub async fn run(mut self) {
        println!("AirPlay: event channel listening");
        loop {
            match self.channel.poll_recv().await {
                Ok(true) => {
                    println!("AirPlay: event channel received {} bytes (plain, buffered)", self.channel.plain_buf.len());
                    loop {
                        let Some(header_end) = find_subslice(&self.channel.plain_buf, b"\r\n\r\n") else {
                            break;
                        };
                        let header_str = String::from_utf8_lossy(&self.channel.plain_buf[..header_end]).to_string();
                        // Requests can carry a body (e.g. POST /command with a
                        // bplist payload) — must skip past Content-Length
                        // bytes too, not just the header terminator, or the
                        // body corrupts parsing of every subsequent message.
                        let content_length: usize = header_str
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        let total = header_end + 4 + content_length;
                        if self.channel.plain_buf.len() < total {
                            break; // full body hasn't arrived yet
                        }
                        println!("AirPlay: event channel request: {header_str:?} (+{content_length} byte body)");
                        // The device uses this for actual commands (e.g. a
                        // reason it's about to tear the session down) — decode
                        // and print it instead of discarding it blind, since
                        // "what does it actually say" is exactly what's
                        // needed to explain an otherwise-unprompted teardown.
                        if content_length > 0 {
                            let body = &self.channel.plain_buf[header_end + 4..total];
                            match plist::Value::from_reader(std::io::Cursor::new(body)) {
                                Ok(value) => println!("AirPlay: event channel body: {value:#?}"),
                                Err(e) => println!("AirPlay: event channel body: not a plist ({e}): {body:x?}"),
                            }
                        }
                        self.channel.plain_buf.drain(..total);
                        let cseq = header_str.lines().find_map(|l| l.strip_prefix("CSeq: ")).unwrap_or("0");
                        let resp = format!("RTSP/1.0 200 OK\r\nContent-Length: 0\r\nAudio-Latency: 0\r\nCSeq: {cseq}\r\n\r\n");
                        if let Err(e) = self.channel.send(resp.as_bytes()).await {
                            println!("AirPlay: event channel send failed: {e}");
                            return;
                        }
                    }
                }
                Ok(false) => {
                    println!("AirPlay: event channel connection closed by device");
                    return;
                }
                Err(e) => {
                    println!("AirPlay: event channel error: {e}");
                    return;
                }
            }
        }
    }
}

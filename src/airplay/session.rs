//! High-level AirPlay 2 "remote control" session orchestration
//! (`pyatv/protocols/airplay/ap2_session.py`): Pair-Verify on the control
//! connection, negotiate the event and data channels, RECORD, then keep the
//! session alive with a 2-second feedback heartbeat. The returned
//! `data_channel` is what carries tunneled MRP `ProtocolMessage` traffic.

use std::time::Duration;

use anyhow::{Context, Error, Result};

use crate::crypto::hkdf_512;
use crate::hap_pair::PairingResult;

use super::channels::{DataChannel, EventChannel};
use super::rtsp::RtspConnection;

const FEEDBACK_INTERVAL: Duration = Duration::from_secs(2);
// Arbitrary but well-formed values; real devices haven't been observed caring
// about their content, only that plausible fields are present (same lesson
// learned from Companion's M5 Name TLV and MRP's DeviceInfoMessage).
const CLIENT_TYPE_UUID: &str = "1910A70F-DBC0-4242-AF95-115DB30604E1";

pub struct Ap2Session {
    pub data_channel: DataChannel,
}

fn random_uuid() -> String {
    crate::random_pairing_id().to_uppercase()
}

async fn setup(control: &mut RtspConnection, body: plist::Dictionary) -> Result<plist::Value> {
    let mut bytes = Vec::new();
    plist::Value::Dictionary(body)
        .to_writer_binary(&mut bytes)
        .map_err(|e| Error::msg(format!("bplist encode: {e}")))?;
    let headers = vec![("Content-Type".to_string(), "application/x-apple-binary-plist".to_string())];
    let resp = control.send_and_receive("SETUP", None, &headers, &bytes).await?;
    if resp.code != 200 {
        return Err(Error::msg(format!("SETUP returned HTTP {}", resp.code)));
    }
    plist::Value::from_reader(std::io::Cursor::new(&resp.body)).map_err(|e| Error::msg(format!("bplist decode: {e}")))
}

impl Ap2Session {
    pub async fn connect(host: &str, port: u16, creds: &PairingResult, display_name: &str) -> Result<Self> {
        let mut control = RtspConnection::connect(host, port).await?;
        let (shared_secret, keys) = super::pairing::pair_verify(&mut control, creds)
            .await
            .context("AirPlay Pair-Verify failed")?;
        control.enable_encryption(&keys.client_encrypt_key, &keys.server_encrypt_key);

        println!("AirPlay: SETUP (remote control)");
        let mut setup_body = plist::Dictionary::new();
        setup_body.insert("isRemoteControlOnly".into(), true.into());
        setup_body.insert("osName".into(), "iPhone OS".into());
        setup_body.insert("sourceVersion".into(), "550.10".into());
        setup_body.insert("timingProtocol".into(), "None".into());
        setup_body.insert("model".into(), "iPhone10,6".into());
        setup_body.insert("deviceID".into(), "FF:EE:DD:CC:BB:AA".into());
        setup_body.insert("osVersion".into(), "14.7.1".into());
        setup_body.insert("osBuildVersion".into(), "18G82".into());
        setup_body.insert("macAddress".into(), "AA:BB:CC:DD:EE:FF".into());
        setup_body.insert("sessionUUID".into(), random_uuid().into());
        setup_body.insert("name".into(), display_name.into());
        let resp = setup(&mut control, setup_body).await.context("SETUP (remote control) failed")?;
        let event_port = resp
            .as_dictionary()
            .and_then(|d| d.get("eventPort"))
            .and_then(|v| v.as_unsigned_integer())
            .ok_or_else(|| Error::msg("SETUP response missing eventPort"))? as u16;
        println!("AirPlay: eventPort={event_port}");

        // Read/Write reversed here: the event connection originates from the
        // receiver, so what we write it decrypts with our "read" key and
        // vice versa (matches pyatv's comment in ap2_session.py).
        let events_output = hkdf_512(&shared_secret, b"Events-Salt", b"Events-Read-Encryption-Key", 32);
        let events_input = hkdf_512(&shared_secret, b"Events-Salt", b"Events-Write-Encryption-Key", 32);
        let event_channel = EventChannel::connect(host, event_port, &events_output, &events_input)
            .await
            .context("event channel connect failed")?;
        tokio::spawn(event_channel.run());

        println!("AirPlay: RECORD");
        control.send_and_receive("RECORD", None, &[], &[]).await.context("RECORD failed")?;

        println!("AirPlay: SETUP (streams)");
        let seed: u64 = rand::random();
        let mut stream_dict = plist::Dictionary::new();
        stream_dict.insert("controlType".into(), 2i64.into());
        stream_dict.insert("channelID".into(), random_uuid().into());
        stream_dict.insert("seed".into(), seed.into());
        stream_dict.insert("clientUUID".into(), random_uuid().into());
        stream_dict.insert("type".into(), 130i64.into());
        stream_dict.insert("wantsDedicatedSocket".into(), true.into());
        stream_dict.insert("clientTypeUUID".into(), CLIENT_TYPE_UUID.into());
        let mut streams_body = plist::Dictionary::new();
        streams_body.insert("streams".into(), plist::Value::Array(vec![plist::Value::Dictionary(stream_dict)]));
        let resp = setup(&mut control, streams_body).await.context("SETUP (streams) failed")?;
        let data_port = resp
            .as_dictionary()
            .and_then(|d| d.get("streams"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_dictionary())
            .and_then(|d| d.get("dataPort"))
            .and_then(|v| v.as_unsigned_integer())
            .ok_or_else(|| Error::msg("SETUP response missing streams[0].dataPort"))? as u16;
        println!("AirPlay: dataPort={data_port}");

        let data_salt = format!("DataStream-Salt{seed}");
        let data_output = hkdf_512(&shared_secret, data_salt.as_bytes(), b"DataStream-Output-Encryption-Key", 32);
        let data_input = hkdf_512(&shared_secret, data_salt.as_bytes(), b"DataStream-Input-Encryption-Key", 32);
        let data_channel = DataChannel::connect(host, data_port, &data_output, &data_input)
            .await
            .context("data channel connect failed")?;

        // The control connection's only remaining job is the feedback
        // heartbeat; hand it off to a background task for the life of the
        // session (mirrors pyatv's `start_keep_alive`).
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(FEEDBACK_INTERVAL);
            loop {
                interval.tick().await;
                match control.send_and_receive("POST", Some("/feedback"), &[], &[]).await {
                    Ok(resp) if resp.code != 200 => {
                        println!("AirPlay: /feedback returned HTTP {}; stopping heartbeat", resp.code);
                        break;
                    }
                    Err(e) => {
                        println!("AirPlay: /feedback failed, stopping heartbeat: {e}");
                        break;
                    }
                    Ok(_) => {}
                }
            }
        });

        println!("AirPlay: remote control session ready");
        Ok(Self { data_channel })
    }
}

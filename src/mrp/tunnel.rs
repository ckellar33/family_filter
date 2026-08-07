//! Runs MRP (device-info handshake and the standard client handshake/push-loop)
//! over an AirPlay 2 data channel instead of a standalone `MrpConnection` —
//! required on tvOS 15+, which no longer advertises MRP as its own service
//! (see `mrp/connection.rs` module docs and the plan notes).
//!
//! No MRP-level Pair-Verify runs here, unlike standalone MRP: per pyatv's
//! `airplay/__init__.py::_create_mrp_tunnel_data`, the synthetic MRP service
//! it creates for the tunnel has no credentials, and `MrpProtocol._enable_encryption`
//! bails out immediately (`if self.service.credentials is None: return`)
//! *before* ever constructing `MrpPairVerifyProcedure`. The device doesn't
//! expect a `CryptoPairingMessage` on the tunnel at all — it just silently
//! drops it — since the connection is already authenticated by the outer
//! AirPlay-level Pair-Verify.

use std::time::Duration;

use anyhow::{Context, Error, Result};

use crate::airplay::channels::DataChannel;
use crate::hap_pair::PairingResult;

use super::messages;
use super::playback::PlaybackState;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

async fn recv_timeout(data_channel: &mut DataChannel) -> Result<Vec<u8>> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, data_channel.recv_mrp())
        .await
        .map_err(|_| Error::msg(format!("timed out after {HANDSHAKE_TIMEOUT:?} waiting for tunneled MRP frame")))?
}

async fn device_info_handshake(data_channel: &mut DataChannel, pairing_id: &str, display_name: &str) -> Result<()> {
    println!("MRP (tunnel): sending DeviceInfoMessage");
    let inner = messages::device_info_message(pairing_id, display_name, "20F66");
    let msg = messages::wrap(
        messages::TYPE_DEVICE_INFO_MESSAGE,
        Some(&messages::random_identifier()),
        Some((messages::FIELD_DEVICE_INFO_MESSAGE, inner)),
    );
    data_channel.send_mrp(&msg).await.context("send DeviceInfoMessage")?;

    let resp = recv_timeout(data_channel).await.context("recv DeviceInfoMessage response")?;
    let (msg_type, _fields) = messages::parse_envelope(&resp)?;
    println!("MRP (tunnel): received response type={msg_type} ({} bytes)", resp.len());
    if msg_type != messages::TYPE_DEVICE_INFO_MESSAGE {
        return Err(Error::msg(format!(
            "expected DeviceInfoMessage (type {}) response, got type {msg_type}",
            messages::TYPE_DEVICE_INFO_MESSAGE
        )));
    }
    Ok(())
}

pub struct TunneledMrpSession {
    data_channel: DataChannel,
    pub playback: PlaybackState,
}

impl TunneledMrpSession {
    pub async fn start(mut data_channel: DataChannel, creds: &PairingResult, display_name: &str) -> Result<Self> {
        device_info_handshake(&mut data_channel, &creds.pairing_id, display_name).await?;

        println!("MRP (tunnel): sending SetConnectionState(Connected)");
        data_channel
            .send_mrp(&messages::wrap(
                messages::TYPE_SET_CONNECTION_STATE_MESSAGE,
                None,
                Some((messages::FIELD_SET_CONNECTION_STATE_MESSAGE, messages::set_connection_state_connected())),
            ))
            .await
            .context("SetConnectionState failed")?;

        let mut playback = PlaybackState::default();

        println!("MRP (tunnel): sending ClientUpdatesConfig");
        Self::send_and_expect(
            &mut data_channel,
            messages::TYPE_CLIENT_UPDATES_CONFIG_MESSAGE,
            Some((messages::FIELD_CLIENT_UPDATES_CONFIG_MESSAGE, messages::client_updates_config_message())),
            &mut playback,
        )
        .await
        .context("ClientUpdatesConfig failed")?;
        println!("MRP (tunnel): ClientUpdatesConfig acknowledged");

        println!("MRP (tunnel): sending GetKeyboardSession");
        Self::send_and_expect(&mut data_channel, messages::TYPE_GET_KEYBOARD_SESSION_MESSAGE, None, &mut playback)
            .await
            .context("GetKeyboardSession failed")?;
        println!("MRP (tunnel): GetKeyboardSession acknowledged; handshake complete");

        Ok(Self { data_channel, playback })
    }

    /// Matches by `identifier`, not declared `type` — see the equivalent
    /// comment on `MrpSession::send_and_expect` (`mrp/session.rs`); the same
    /// mismatch is what made the tunneled `ClientUpdatesConfig` handshake
    /// time out on real hardware despite the device actually replying.
    async fn send_and_expect(
        data_channel: &mut DataChannel,
        msg_type: i64,
        inner: Option<(u32, Vec<u8>)>,
        playback: &mut PlaybackState,
    ) -> Result<()> {
        let identifier = messages::random_identifier();
        let msg = messages::wrap(msg_type, Some(&identifier), inner);
        data_channel.send_mrp(&msg).await?;
        loop {
            let resp = recv_timeout(data_channel).await?;
            let (got_type, fields) = messages::parse_envelope(&resp)?;
            playback.apply_envelope(got_type, &fields);
            if messages::envelope_identifier(&fields).as_deref() == Some(identifier.as_str()) {
                return Ok(());
            }
        }
    }

    /// Block until the next now-playing-relevant push (`SetStateMessage`,
    /// `SetNowPlayingClientMessage`, or `SetNowPlayingPlayerMessage`)
    /// arrives and apply it to `self.playback`. No timeout — mirrors
    /// `MrpSession::recv_update`.
    pub async fn recv_update(&mut self) -> Result<()> {
        loop {
            let resp = self.data_channel.recv_mrp().await?;
            let (got_type, fields) = messages::parse_envelope(&resp)?;
            if self.playback.apply_envelope(got_type, &fields) {
                return Ok(());
            }
        }
    }

    /// Press and release a single HID Consumer-page key. Fire-and-forget,
    /// matching pyatv's own volume handling (`_send_hid_key(..., flush=False)`
    /// in `protocols/mrp/__init__.py`) — MRP's `SendHIDEventMessage` carries
    /// no identifier and gets no reply at all, unlike Companion's `_hidC`
    /// (`companion.rs::hid_command`), so there's nothing to wait for here.
    async fn press_key(&mut self, usage: u16) -> Result<()> {
        for down in [true, false] {
            self.data_channel
                .send_mrp(&messages::wrap(
                    messages::TYPE_SEND_HID_EVENT_MESSAGE,
                    None,
                    Some((
                        messages::FIELD_SEND_HID_EVENT_MESSAGE,
                        messages::send_hid_event_message(messages::HID_USAGE_PAGE_CONSUMER, usage, down),
                    )),
                ))
                .await?;
        }
        Ok(())
    }

    /// Mute/unmute via the USB HID Consumer "Mute" usage
    /// ([`messages::HID_USAGE_MUTE`]) over the AirPlay-tunneled MRP data
    /// channel — no Companion pairing/session involved. It's a real toggle,
    /// so both are the same single press-and-release. Untested against
    /// pyatv (it never implements mute at all); revert to a volume-down
    /// burst if the device doesn't honor this.
    pub async fn mute(&mut self) -> Result<()> {
        self.press_key(messages::HID_USAGE_MUTE).await
    }

    pub async fn unmute(&mut self) -> Result<()> {
        self.press_key(messages::HID_USAGE_MUTE).await
    }
}

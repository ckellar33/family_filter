//! Post-Pair-Verify MRP session: runs the standard handshake (matches
//! pyatv's `MrpProtocol.start()`: `SetConnectionState` → `ClientUpdatesConfig`
//! → `GetKeyboardSession`), then just reads pushed `SetStateMessage`s into a
//! `PlaybackState` — nothing is polled from the device itself.

use anyhow::{Context, Result};

use super::connection::{MrpConnection, HANDSHAKE_TIMEOUT};
use super::messages;
use super::playback::PlaybackState;
use crate::hap_pair::{PairingResult, VerifyResult};

pub struct MrpSession {
    conn: MrpConnection,
    pub playback: PlaybackState,
}

impl MrpSession {
    /// Pair-Verify + full handshake on a fresh connection.
    pub async fn connect(
        stream: tokio::net::TcpStream,
        creds: &PairingResult,
        display_name: &str,
    ) -> Result<Self> {
        let mut conn = MrpConnection::new(stream);
        let keys: VerifyResult = super::pairing::pair_verify(&mut conn, creds, display_name)
            .await
            .context("MRP Pair-Verify failed")?;
        conn.enable_encryption(&keys.client_encrypt_key, &keys.server_encrypt_key);

        println!("MRP: sending SetConnectionState(Connected)");
        conn.send(&messages::wrap(
            messages::TYPE_SET_CONNECTION_STATE_MESSAGE,
            None,
            Some((messages::FIELD_SET_CONNECTION_STATE_MESSAGE, messages::set_connection_state_connected())),
        ))
        .await
        .context("SetConnectionState failed")?;

        println!("MRP: sending ClientUpdatesConfig");
        Self::send_and_expect(
            &mut conn,
            messages::TYPE_CLIENT_UPDATES_CONFIG_MESSAGE,
            Some((messages::FIELD_CLIENT_UPDATES_CONFIG_MESSAGE, messages::client_updates_config_message())),
            &mut PlaybackState::default(),
        )
        .await
        .context("ClientUpdatesConfig failed")?;
        println!("MRP: ClientUpdatesConfig acknowledged");

        println!("MRP: sending GetKeyboardSession");
        Self::send_and_expect(
            &mut conn,
            messages::TYPE_GET_KEYBOARD_SESSION_MESSAGE,
            None,
            &mut PlaybackState::default(),
        )
        .await
        .context("GetKeyboardSession failed")?;
        println!("MRP: GetKeyboardSession acknowledged; handshake complete");

        Ok(Self {
            conn,
            playback: PlaybackState::default(),
        })
    }

    /// Send a request and wait for the device to echo back its `identifier`,
    /// feeding any `SetStateMessage`s encountered along the way (including
    /// the matching reply itself, if the device folds data into it) into
    /// `playback` rather than discarding them. Matches by `identifier`, not
    /// declared `type` — pyatv's `MrpProtocol.send_and_receive` correlates
    /// the same way (`protocol.py::message_received`), and a real device's
    /// reply need not be typed the same as the request, or arrive alone
    /// rather than interleaved with unrelated pushes. Bounded by
    /// `HANDSHAKE_TIMEOUT` so an unresponsive device surfaces as an error
    /// instead of hanging forever.
    async fn send_and_expect(
        conn: &mut MrpConnection,
        msg_type: i64,
        inner: Option<(u32, Vec<u8>)>,
        playback: &mut PlaybackState,
    ) -> Result<()> {
        let identifier = messages::random_identifier();
        let msg = messages::wrap(msg_type, Some(&identifier), inner);
        conn.send(&msg).await?;
        loop {
            let resp = conn.recv_timeout(HANDSHAKE_TIMEOUT).await?;
            let (got_type, fields) = messages::parse_envelope(&resp)?;
            playback.apply_envelope(got_type, &fields);
            if messages::envelope_identifier(&fields).as_deref() == Some(identifier.as_str()) {
                return Ok(());
            }
        }
    }

    /// Block until the next now-playing-relevant push (`SetStateMessage`,
    /// `SetNowPlayingClientMessage`, or `SetNowPlayingPlayerMessage`)
    /// arrives and apply it to `self.playback`. Any other message type is
    /// ignored.
    pub async fn recv_update(&mut self) -> Result<()> {
        loop {
            let resp = self.conn.recv().await?;
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
            self.conn
                .send(&messages::wrap(
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
    /// ([`messages::HID_USAGE_MUTE`]) — a real toggle, so both are the same
    /// single press-and-release; there's no separate "restore volume" step
    /// like the old volume-down/-up press-burst approximation needed.
    /// Untested against pyatv (it never implements mute at all); revert to
    /// a volume-down burst if the device doesn't honor this.
    pub async fn mute(&mut self) -> Result<()> {
        self.press_key(messages::HID_USAGE_MUTE).await
    }

    pub async fn unmute(&mut self) -> Result<()> {
        self.press_key(messages::HID_USAGE_MUTE).await
    }

    /// Actively request a fresh copy of the current queue item's metadata
    /// instead of only ever waiting on the app's own (possibly throttled)
    /// `SetStateMessage` pushes — the passive-only approach measurably left
    /// `elapsedTime` a few seconds stale in practice. Mirrors pyatv's
    /// artwork-fetch use of `PlaybackQueueRequestMessage`
    /// (`protocols/mrp/__init__.py::_fetch_remote_artwork`), repurposed here
    /// for its metadata rather than its artwork bytes.
    pub async fn refresh_position(&mut self) -> Result<()> {
        let location = self.playback.active_queue_location();
        let playback = &mut self.playback;
        Self::send_and_expect(
            &mut self.conn,
            messages::TYPE_PLAYBACK_QUEUE_REQUEST_MESSAGE,
            Some((messages::FIELD_PLAYBACK_QUEUE_REQUEST_MESSAGE, messages::playback_queue_request_message(location))),
            playback,
        )
        .await
        .context("PlaybackQueueRequestMessage failed")
    }
}

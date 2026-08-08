//! Builders/parsers for the small subset of MRP `ProtocolMessage` variants
//! this project needs. Field numbers are transcribed from pyatv's `.proto`
//! sources (`pyatv/protocols/mrp/protobuf/*.proto`); see the plan notes for
//! the exact files consulted. proto2 `extend ProtocolMessage { ... }` blocks
//! just reserve a field number on the envelope for a given sub-message type —
//! there is no separate wire encoding for "extension" vs. "regular" fields.

use anyhow::{Error, Result};
use protobuf_lite::{last_field, MessageBuilder, WireValue};

// ProtocolMessage envelope field numbers.
const FIELD_TYPE: u32 = 1;
const FIELD_IDENTIFIER: u32 = 2;
const FIELD_ERROR_CODE: u32 = 4;
const FIELD_UNIQUE_IDENTIFIER: u32 = 85;

// ProtocolMessage.Type enum values.
pub const TYPE_SET_STATE_MESSAGE: i64 = 4;
pub const TYPE_DEVICE_INFO_MESSAGE: i64 = 15;
pub const TYPE_CLIENT_UPDATES_CONFIG_MESSAGE: i64 = 16;
pub const TYPE_GET_KEYBOARD_SESSION_MESSAGE: i64 = 24;
pub const TYPE_CRYPTO_PAIRING_MESSAGE: i64 = 34;
pub const TYPE_SET_CONNECTION_STATE_MESSAGE: i64 = 38;
pub const TYPE_SET_NOW_PLAYING_CLIENT_MESSAGE: i64 = 46;
pub const TYPE_SET_NOW_PLAYING_PLAYER_MESSAGE: i64 = 47;
pub const TYPE_SEND_HID_EVENT_MESSAGE: i64 = 8;
pub const TYPE_PLAYBACK_QUEUE_REQUEST_MESSAGE: i64 = 32;

// Extension field numbers on ProtocolMessage for each sub-message.
pub const FIELD_DEVICE_INFO_MESSAGE: u32 = 20;
pub const FIELD_CLIENT_UPDATES_CONFIG_MESSAGE: u32 = 21;
pub const FIELD_CRYPTO_PAIRING_MESSAGE: u32 = 39;
pub const FIELD_SET_CONNECTION_STATE_MESSAGE: u32 = 42;
pub const FIELD_SET_STATE_MESSAGE: u32 = 9;
pub const FIELD_SET_NOW_PLAYING_CLIENT_MESSAGE: u32 = 50;
pub const FIELD_SET_NOW_PLAYING_PLAYER_MESSAGE: u32 = 51;
pub const FIELD_SEND_HID_EVENT_MESSAGE: u32 = 13;
pub const FIELD_PLAYBACK_QUEUE_REQUEST_MESSAGE: u32 = 37;

/// USB HID Consumer-page usage codes `SendHIDEventMessage` accepts.
/// `HID_USAGE_MUTE` is a real toggle control, unlike Volume
/// Increment/Decrement (which is all pyatv's `_KEY_LOOKUP` has —
/// `protocols/mrp/__init__.py` — since it never implements a mute command
/// at all, per `protocols/mrp/audio.py`). Using the dedicated Mute usage
/// here is therefore untested against pyatv itself; `SendHIDEventMessage`
/// forwards whatever `(usagePage, usage)` it's given, so there's a
/// reasonable chance the device's CEC bridge honors it as an actual toggle,
/// but verify it actually mutes before relying on it — if not, the fallback
/// is a repeated-decrement approximation like Companion's HID path used
/// (`git log` has the old `press_key(..., times)` burst version).
pub const HID_USAGE_PAGE_CONSUMER: u16 = 12;
pub const HID_USAGE_MUTE: u16 = 0xE2;

/// A fresh request/response correlation token for the `identifier` param of
/// [`wrap`] (envelope field 2 — see its docs for how this differs from the
/// `uniqueIdentifier` field `wrap` always sets).
pub fn random_identifier() -> String {
    crate::random_pairing_id().to_uppercase()
}

/// Wrap an encoded sub-message in a `ProtocolMessage` envelope.
///
/// Two distinct identifier fields are in play here (`MrpProtocol.create` /
/// `send_and_receive` in pyatv):
/// - `uniqueIdentifier` (field 85) is stamped on *every* outgoing message
///   unconditionally — without it the device silently drops the message
///   instead of responding.
/// - `identifier` (field 2, `identifier` param here) is a client-side
///   request/response correlation token: set it when the device is expected
///   to echo it back on the matching reply (DeviceInfoMessage,
///   ClientUpdatesConfig, GetKeyboardSession); leave it `None` for messages
///   that never carry one back (CryptoPairingMessage) or that expect no
///   reply at all (SetConnectionState).
pub fn wrap(msg_type: i64, identifier: Option<&str>, inner: Option<(u32, Vec<u8>)>) -> Vec<u8> {
    let mut b = MessageBuilder::new()
        .varint(FIELD_TYPE, msg_type)
        .varint(FIELD_ERROR_CODE, 0)
        .string(FIELD_UNIQUE_IDENTIFIER, &crate::random_pairing_id().to_uppercase());
    if let Some(id) = identifier {
        b = b.string(FIELD_IDENTIFIER, id);
    }
    if let Some((field, bytes)) = inner {
        b = b.submessage(field, &bytes);
    }
    b.encode()
}

/// Parse a `ProtocolMessage` envelope, returning its `type` and raw fields so
/// callers can pull out whichever extension field they expect.
pub fn parse_envelope(data: &[u8]) -> Result<(i64, Vec<(u32, WireValue)>)> {
    let fields = protobuf_lite::decode(data).map_err(|e| Error::msg(format!("protobuf decode: {e}")))?;
    let msg_type = last_field(&fields, FIELD_TYPE)
        .and_then(WireValue::as_i64)
        .unwrap_or(0);
    Ok((msg_type, fields))
}

/// Extract the `identifier` field (2) from an already-parsed envelope, if
/// present. This is the request/response correlation token pyatv's
/// `MrpProtocol.send_and_receive` actually keys its `_outstanding` map on
/// (`protocol.py::message_received`) — *not* the message `type`, which a
/// real device's reply to e.g. `ClientUpdatesConfig` need not even carry:
/// it can (and does) answer by echoing our identifier on a message the
/// device otherwise classifies under some other/unmodeled type, interleaved
/// with a burst of unrelated pushes.
pub fn envelope_identifier(fields: &[(u32, WireValue)]) -> Option<String> {
    last_field(fields, FIELD_IDENTIFIER).and_then(WireValue::as_string)
}

/// `DeviceInfoMessage` fields mirrored from pyatv's `messages.device_information`
/// (real Apple TVs are picky about seeing the same set Companion pairing
/// already taught us they require for its own DeviceInfo-equivalent step).
pub fn device_info_message(pairing_id: &str, display_name: &str, os_build: &str) -> Vec<u8> {
    MessageBuilder::new()
        .string(1, pairing_id) // uniqueIdentifier
        .string(2, display_name) // name (required)
        .string(3, "iPhone") // localizedModelName
        .string(4, os_build) // systemBuildVersion
        .string(5, "com.apple.TVRemote") // applicationBundleIdentifier
        .string(6, "344.28") // applicationBundleVersion
        .varint(7, 1) // protocolVersion
        .varint(8, 108) // lastSupportedMessageType
        .bool(9, true) // supportsSystemPairing
        .bool(10, true) // allowsPairing
        .string(12, "com.apple.TVMusic") // systemMediaApplication
        .bool(13, true) // supportsACL
        .bool(14, true) // supportsSharedQueue
        .bool(15, true) // supportsExtendedMotion
        .varint(17, 2) // sharedQueueVersion
        .varint(21, 1) // deviceClass = iPhone
        .varint(22, 1) // logicalDeviceCount
        .encode()
}

/// `CryptoPairingMessage`: wraps a TLV8 pairing blob. `is_pairing` matches
/// pyatv's flag on the very first Pair-Setup message only (sets `state = 2`).
pub fn crypto_pairing_message(pairing_data_tlv: &[u8], is_pairing: bool) -> Vec<u8> {
    MessageBuilder::new()
        .bytes(1, pairing_data_tlv) // pairingData
        .varint(2, 0) // status
        .bool(3, false) // isRetrying
        .bool(4, false) // isUsingSystemPairing
        .varint(5, if is_pairing { 2 } else { 0 }) // state
        .encode()
}

/// Extract `CryptoPairingMessage.pairingData` (field 1) from an envelope's
/// already-located sub-message bytes.
pub fn crypto_pairing_data(inner: &[u8]) -> Result<Vec<u8>> {
    let fields = protobuf_lite::decode(inner).map_err(|e| Error::msg(format!("protobuf decode: {e}")))?;
    last_field(&fields, 1)
        .and_then(WireValue::as_bytes)
        .map(|b| b.to_vec())
        .ok_or_else(|| Error::msg("CryptoPairingMessage missing pairingData"))
}

/// `SendHIDEventMessage.hidEventData`: an opaque, fixed-format binary blob
/// (an encoded `IOHIDEvent`, not itself protobuf) transcribed byte-for-byte
/// from pyatv's `messages.send_hid_event` (`protocols/mrp/messages.py`).
/// The leading 8 bytes stand in for a Mach `AbsoluteTime` timestamp pyatv
/// hardcodes rather than generates ("the device does not seem to care much
/// about the value"); everything else pyatv also treats as an undecoded
/// fixed template except the trailing `(usagePage, usage, down)` triple,
/// each big-endian `u16`.
fn hid_event_data(usage_page: u16, usage: u16, down: bool) -> Vec<u8> {
    let mut data = hex::decode("438922cf08020000").expect("valid hex literal");
    data.extend(
        hex::decode(concat!(
            "00000000000000000100000000000000020",
            "00000200000000300000001000000000000"
        ))
        .expect("valid hex literal"),
    );
    data.extend_from_slice(&usage_page.to_be_bytes());
    data.extend_from_slice(&usage.to_be_bytes());
    data.extend_from_slice(&(if down { 1u16 } else { 0u16 }).to_be_bytes());
    data.extend(hex::decode("0000000000000001000000").expect("valid hex literal"));
    data
}

/// `SendHIDEventMessage` sub-message bytes for a single button press/release
/// (`type` = [`TYPE_SEND_HID_EVENT_MESSAGE`], extension field
/// [`FIELD_SEND_HID_EVENT_MESSAGE`]) — the MRP-tunnel-over-AirPlay analog of
/// Companion's `_hidC` (`companion.rs::hid_command`), used e.g. to simulate
/// volume-button presses when driving mute via AirPlay/MRP instead of
/// Companion. Unlike Companion's HID path, MRP's is fire-and-forget by
/// design: pyatv sends it with plain `protocol.send()` (`flush=False` for
/// volume specifically), not `send_and_receive`, so there's no ack to wait
/// for here either.
pub fn send_hid_event_message(usage_page: u16, usage: u16, down: bool) -> Vec<u8> {
    MessageBuilder::new().bytes(1, &hid_event_data(usage_page, usage, down)).encode()
}

/// `PlaybackQueueRequestMessage` sub-message bytes: an *active* request for
/// a fresh copy of one queue item's metadata, instead of only ever waiting
/// on the app's own (possibly throttled/stale) `SetStateMessage` pushes.
/// Transcribed from pyatv's `messages.playback_queue_request`
/// (`protocols/mrp/messages.py`, used there for on-demand artwork fetching
/// — `PlaybackQueueRequestMessage.proto`'s `location`/`length` fields are
/// what matter here; `includeMetadata` isn't in pyatv's version at all, but
/// is added explicitly here since metadata, not artwork, is the point).
/// The device replies with an ordinary `SetStateMessage` (confirmed by
/// pyatv's artwork fetch reading `extract_inner(resp).playbackQueue` off
/// the response), matched via the standard `identifier` echo like any other
/// request.
pub fn playback_queue_request_message(location: i64) -> Vec<u8> {
    MessageBuilder::new()
        .varint(1, location) // location
        .varint(2, 1) // length: just the one item
        .bool(3, true) // includeMetadata
        .encode()
}

pub fn set_connection_state_connected() -> Vec<u8> {
    MessageBuilder::new().varint(1, 2).encode() // state = Connected
}

/// Exactly pyatv's `client_updates_config` defaults. `nowPlayingUpdates` is
/// tempting to flip on here since that sounds like what unlocks live
/// playback position, but pyatv — which does get playback position — never
/// sets it `true` anywhere; `SetStateMessage` pushes arrive regardless,
/// dispatched unconditionally by `PlayerStateManager`. This project used to
/// pass `true`, which the real device silently ignored (never acked the
/// message at all), leaving nothing to catch since this is genuinely
/// untested territory relative to every known client.
pub fn client_updates_config_message() -> Vec<u8> {
    MessageBuilder::new()
        .bool(1, true) // artworkUpdates
        .bool(2, false) // nowPlayingUpdates
        .bool(3, true) // volumeUpdates
        .bool(4, true) // keyboardUpdates
        .bool(5, true) // outputDeviceUpdates
        .encode()
}

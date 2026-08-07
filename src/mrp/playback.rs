//! Now-playing state and the position-extrapolation math pyatv uses to get
//! sub-second accuracy without polling the device: the device pushes
//! `SetStateMessage` only when something changes, and clients extrapolate the
//! current position locally from the last known `(elapsedTime, timestamp,
//! playbackRate)` triple.
//!
//! Real devices run *multiple* players concurrently (e.g. an idle system
//! player alongside whatever app is actually playing) and push
//! `SetStateMessage` for all of them, each tagged with a `playerPath`
//! (client bundle id + player id) identifying which one it's about. Only one
//! is "now playing" at a time, and the device says which via two more
//! messages: `SetNowPlayingClientMessage` (which bundle id is active) and
//! `SetNowPlayingPlayerMessage` (which player id is active *for a given
//! client*, defaulting to `DEFAULT_PLAYER_ID` if never set) — mirrors
//! pyatv's `PlayerStateManager` (`protocols/mrp/player_state.py`). Treating
//! every incoming `SetStateMessage` as one global state, as this module used
//! to, meant a push from an unrelated idle/background player could silently
//! overwrite the state of whatever was actually playing.

use std::collections::HashMap;

use protobuf_lite::{all_fields, decode, last_field, WireValue};

use super::messages;

/// Seconds between the Unix epoch (1970-01-01) and the Cocoa epoch
/// (2001-01-01), which is what `elapsedTimeTimestamp` is measured against.
const COCOA_EPOCH_OFFSET: f64 = 978_307_200.0;

// SetStateMessage field numbers.
const FIELD_PLAYBACK_STATE: u32 = 6;
const FIELD_PLAYBACK_QUEUE: u32 = 3;
const FIELD_SET_STATE_PLAYER_PATH: u32 = 9;

// PlaybackQueue field numbers.
const FIELD_QUEUE_LOCATION: u32 = 1;
const FIELD_QUEUE_CONTENT_ITEMS: u32 = 2;

// ContentItem field numbers.
const FIELD_ITEM_METADATA: u32 = 2;

// ContentItemMetadata field numbers.
const FIELD_META_TITLE: u32 = 1;
const FIELD_META_DURATION: u32 = 14;
const FIELD_META_ELAPSED_TIME: u32 = 35;
const FIELD_META_PLAYBACK_RATE: u32 = 39;
const FIELD_META_ELAPSED_TIMESTAMP: u32 = 74;

// PlayerPath field numbers (`PlayerPath.proto`): origin=1, client=2, player=3.
const FIELD_PLAYER_PATH_CLIENT: u32 = 2;
const FIELD_PLAYER_PATH_PLAYER: u32 = 3;

// NowPlayingClient field numbers (`NowPlayingClient.proto`).
const FIELD_NOW_PLAYING_CLIENT_BUNDLE_ID: u32 = 2;

// NowPlayingPlayer field numbers (`NowPlayingPlayer.proto`).
const FIELD_NOW_PLAYING_PLAYER_IDENTIFIER: u32 = 1;

// SetNowPlayingClientMessage / SetNowPlayingPlayerMessage body field numbers.
const FIELD_SNPC_CLIENT: u32 = 1;
const FIELD_SNPP_PLAYER_PATH: u32 = 1;

/// Fallback player id a client uses when it never explicitly designates one
/// via `SetNowPlayingPlayerMessage` (pyatv's `player_state.DEFAULT_PLAYER_ID`).
const DEFAULT_PLAYER_ID: &str = "MediaRemote-DefaultPlayer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStateKind {
    #[default]
    Unknown,
    Playing,
    Paused,
    Stopped,
    Interrupted,
    Seeking,
}

impl PlaybackStateKind {
    fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Playing,
            2 => Self::Paused,
            3 => Self::Stopped,
            4 => Self::Interrupted,
            5 => Self::Seeking,
            _ => Self::Unknown,
        }
    }
}

/// One player's state, keyed by `PlayerKey` in `PlaybackState::players`.
#[derive(Debug, Clone, Default)]
struct PlayerSnapshot {
    title: Option<String>,
    duration: Option<f64>,
    elapsed_time: Option<f64>,
    /// Cocoa-epoch seconds at which `elapsed_time` was accurate.
    elapsed_timestamp: Option<f64>,
    playback_rate: Option<f32>,
    playback_state: PlaybackStateKind,
}

impl PlayerSnapshot {
    /// Apply an incoming `SetStateMessage`'s fields. Only fields actually
    /// present in this particular update are touched — a message that
    /// changes just `playbackState` (no `playbackQueue`) shouldn't reset
    /// timing fields we already know, matching pyatv's `handle_set_state`.
    fn apply_set_state(&mut self, set_state_fields: &[(u32, WireValue)]) {
        if let Some(v) = last_field(set_state_fields, FIELD_PLAYBACK_STATE).and_then(WireValue::as_i32) {
            self.playback_state = PlaybackStateKind::from_i32(v);
        }

        let Some(queue_bytes) = last_field(set_state_fields, FIELD_PLAYBACK_QUEUE).and_then(WireValue::as_bytes)
        else {
            return;
        };
        let Ok(queue_fields) = decode(queue_bytes) else {
            return;
        };
        let location = last_field(&queue_fields, FIELD_QUEUE_LOCATION)
            .and_then(WireValue::as_i64)
            .unwrap_or(0) as usize;
        let items: Vec<&[u8]> = all_fields(&queue_fields, FIELD_QUEUE_CONTENT_ITEMS)
            .filter_map(WireValue::as_bytes)
            .collect();
        let Some(item_bytes) = items.get(location) else {
            return;
        };
        let Ok(item_fields) = decode(item_bytes) else {
            return;
        };
        let Some(meta_bytes) = last_field(&item_fields, FIELD_ITEM_METADATA).and_then(WireValue::as_bytes) else {
            return;
        };
        let Ok(meta_fields) = decode(meta_bytes) else {
            return;
        };

        if let Some(v) = last_field(&meta_fields, FIELD_META_TITLE).and_then(WireValue::as_string) {
            self.title = Some(v);
        }
        if let Some(v) = last_field(&meta_fields, FIELD_META_DURATION).and_then(WireValue::as_f64) {
            self.duration = Some(v);
        }
        if let Some(v) = last_field(&meta_fields, FIELD_META_ELAPSED_TIME).and_then(WireValue::as_f64) {
            self.elapsed_time = Some(v);
        }
        if let Some(v) = last_field(&meta_fields, FIELD_META_PLAYBACK_RATE).and_then(WireValue::as_f32) {
            self.playback_rate = Some(v);
        }
        if let Some(v) = last_field(&meta_fields, FIELD_META_ELAPSED_TIMESTAMP).and_then(WireValue::as_f64) {
            self.elapsed_timestamp = Some(v);
        }
    }

    /// Current playback position, extrapolated locally for 1s-and-better
    /// accuracy without asking the device again — mirrors pyatv's
    /// `build_playing_instance.position()`.
    fn position_now(&self) -> Option<f64> {
        let elapsed_timestamp = self.elapsed_timestamp?;
        let elapsed_time = self.elapsed_time.unwrap_or(0.0);
        let playback_rate = self.playback_rate.unwrap_or(0.0);

        if self.playback_state == PlaybackStateKind::Playing && playback_rate.abs() > f32::EPSILON {
            let event_unix = elapsed_timestamp + COCOA_EPOCH_OFFSET;
            let now_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            Some(elapsed_time + (now_unix - event_unix))
        } else {
            Some(elapsed_time)
        }
    }
}

/// Identifies one player: the app's bundle id plus its player id within that
/// app (most apps just use `DEFAULT_PLAYER_ID`, but some register several,
/// e.g. distinct audio/video players). Empty strings stand in for "field
/// absent" so a `SetStateMessage` that never carried a `playerPath` at all
/// still gets a stable (if anonymous) slot instead of being dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
struct PlayerKey {
    bundle_id: String,
    player_id: String,
}

fn parse_player_path(fields: &[(u32, WireValue)]) -> PlayerKey {
    let bundle_id = last_field(fields, FIELD_PLAYER_PATH_CLIENT)
        .and_then(WireValue::as_bytes)
        .and_then(|b| decode(b).ok())
        .and_then(|cf| last_field(&cf, FIELD_NOW_PLAYING_CLIENT_BUNDLE_ID).and_then(WireValue::as_string))
        .unwrap_or_default();
    let player_id = last_field(fields, FIELD_PLAYER_PATH_PLAYER)
        .and_then(WireValue::as_bytes)
        .and_then(|b| decode(b).ok())
        .and_then(|pf| last_field(&pf, FIELD_NOW_PLAYING_PLAYER_IDENTIFIER).and_then(WireValue::as_string))
        .unwrap_or_default();
    PlayerKey { bundle_id, player_id }
}

/// Tracks every player the device has told us about and which one is
/// currently "now playing" — mirrors pyatv's `PlayerStateManager`. Only the
/// active player's state is exposed to callers (`title`/`duration`/
/// `playback_state`/`position_now`); everything else is maintained silently
/// so it's ready the moment it *does* become active.
#[derive(Debug, Clone, Default)]
pub struct PlaybackState {
    players: HashMap<PlayerKey, PlayerSnapshot>,
    /// Bundle id of the active client, set by `SetNowPlayingClientMessage`.
    /// `None` until the device tells us — matches pyatv, which reports no
    /// now-playing info at all until this arrives, rather than guessing.
    active_client: Option<String>,
    /// Per-client active player id, set by `SetNowPlayingPlayerMessage`.
    /// Falls back to `DEFAULT_PLAYER_ID` for a client that hasn't
    /// explicitly picked one yet.
    active_player_per_client: HashMap<String, String>,
}

impl PlaybackState {
    fn active_key(&self) -> Option<PlayerKey> {
        let bundle_id = self.active_client.clone()?;
        let player_id = self
            .active_player_per_client
            .get(&bundle_id)
            .cloned()
            .unwrap_or_else(|| DEFAULT_PLAYER_ID.to_string());
        Some(PlayerKey { bundle_id, player_id })
    }

    fn active(&self) -> Option<&PlayerSnapshot> {
        self.players.get(&self.active_key()?)
    }

    pub fn title(&self) -> Option<&str> {
        self.active().and_then(|p| p.title.as_deref())
    }

    pub fn duration(&self) -> Option<f64> {
        self.active().and_then(|p| p.duration)
    }

    pub fn playback_state(&self) -> PlaybackStateKind {
        self.active().map(|p| p.playback_state).unwrap_or_default()
    }

    pub fn position_now(&self) -> Option<f64> {
        self.active().and_then(PlayerSnapshot::position_now)
    }

    /// Feed one received `ProtocolMessage` envelope in. Irrelevant types are
    /// ignored (safe to call unconditionally on every push received).
    /// Returns whether this was one of the three types that can change what
    /// `active()` reports, so callers driving a UI refresh loop know whether
    /// this push is worth redisplaying for.
    pub fn apply_envelope(&mut self, msg_type: i64, envelope_fields: &[(u32, WireValue)]) -> bool {
        match msg_type {
            messages::TYPE_SET_STATE_MESSAGE => {
                if let Some(inner) = decode_extension(envelope_fields, messages::FIELD_SET_STATE_MESSAGE) {
                    let key = last_field(&inner, FIELD_SET_STATE_PLAYER_PATH)
                        .and_then(WireValue::as_bytes)
                        .and_then(|b| decode(b).ok())
                        .map(|pp| parse_player_path(&pp))
                        .unwrap_or_default();
                    // Diagnostic: a SetStateMessage only reaches the display
                    // if its playerPath matches the currently-active
                    // (bundle_id, player_id) — a hardware-remote-originated
                    // pause routed through a different playerPath (e.g. a
                    // system player instead of the app's own) would silently
                    // update the wrong entry and never surface.
                    let is_active = self.active_key().as_ref() == Some(&key);
                    let has_playback_state = last_field(&inner, FIELD_PLAYBACK_STATE).is_some();
                    let has_queue = last_field(&inner, FIELD_PLAYBACK_QUEUE).is_some();
                    println!(
                        "MRP: SetStateMessage playerPath=({:?}, {:?}) active={is_active} hasPlaybackState={has_playback_state} hasQueue={has_queue}",
                        key.bundle_id, key.player_id,
                    );
                    self.players.entry(key).or_default().apply_set_state(&inner);
                }
                true
            }
            messages::TYPE_SET_NOW_PLAYING_CLIENT_MESSAGE => {
                if let Some(inner) = decode_extension(envelope_fields, messages::FIELD_SET_NOW_PLAYING_CLIENT_MESSAGE) {
                    if let Some(bundle_id) = last_field(&inner, FIELD_SNPC_CLIENT)
                        .and_then(WireValue::as_bytes)
                        .and_then(|b| decode(b).ok())
                        .and_then(|cf| last_field(&cf, FIELD_NOW_PLAYING_CLIENT_BUNDLE_ID).and_then(WireValue::as_string))
                    {
                        println!("MRP: SetNowPlayingClientMessage active_client={bundle_id:?} (was {:?})", self.active_client);
                        self.active_client = Some(bundle_id);
                    }
                }
                true
            }
            messages::TYPE_SET_NOW_PLAYING_PLAYER_MESSAGE => {
                if let Some(inner) = decode_extension(envelope_fields, messages::FIELD_SET_NOW_PLAYING_PLAYER_MESSAGE) {
                    if let Some(pp) = last_field(&inner, FIELD_SNPP_PLAYER_PATH)
                        .and_then(WireValue::as_bytes)
                        .and_then(|b| decode(b).ok())
                    {
                        let key = parse_player_path(&pp);
                        println!(
                            "MRP: SetNowPlayingPlayerMessage client={:?} active_player={:?} (was {:?})",
                            key.bundle_id,
                            key.player_id,
                            self.active_player_per_client.get(&key.bundle_id)
                        );
                        self.active_player_per_client.insert(key.bundle_id, key.player_id);
                    }
                }
                true
            }
            _ => false,
        }
    }
}

fn decode_extension(envelope_fields: &[(u32, WireValue)], field: u32) -> Option<Vec<(u32, WireValue)>> {
    last_field(envelope_fields, field).and_then(WireValue::as_bytes).and_then(|b| decode(b).ok())
}

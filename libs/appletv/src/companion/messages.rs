//! OPACK message builders for Companion's media-control protocol
//! (`_systemInfo`, `_touchStart`, `_sessionStart`, `TVRCSessionStart`,
//! `_tiStart`, `_interest`, `_mcc`).

use std::collections::HashMap;

use opack::Value;

pub const MSG_EVENT: i64 = 1;
pub const MSG_REQUEST: i64 = 2;
pub const MSG_RESPONSE: i64 = 3;

const MCC_SKIP_BY: i64 = 7;

pub fn map_of(entries: &[(&str, Value)]) -> HashMap<String, Value> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

pub fn system_info_message(pairing_id: &str) -> HashMap<String, Value> {
    map_of(&[
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
    ])
}

/// Real Apple TVs require a touch session before `_sessionStart` will
/// succeed; fake_device doesn't enforce this ordering (pyatv api.py calls
/// `_touch_start()` between `system_info()` and `_session_start()`).
pub fn touch_start_message() -> HashMap<String, Value> {
    map_of(&[
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
    ])
}

pub fn session_start_message(local_sid: i64) -> HashMap<String, Value> {
    map_of(&[
        ("_i", Value::Str("_sessionStart".into())),
        ("_t", Value::Int(MSG_REQUEST)),
        (
            "_c",
            Value::Dict(map_of(&[
                ("_srvT", Value::Str("com.apple.tvremoteservices".into())),
                ("_sid", Value::Int(local_sid)),
            ])),
        ),
    ])
}

/// Best-effort: not all devices support a TV Remote Client session.
pub fn tv_rc_session_start_message() -> HashMap<String, Value> {
    map_of(&[
        ("_i", Value::Str("TVRCSessionStart".into())),
        ("_t", Value::Int(MSG_REQUEST)),
        (
            "_c",
            Value::Dict(map_of(&[(
                "ProtocolVersionKey",
                Value::Str("1.2".into()),
            )])),
        ),
    ])
}

pub fn text_input_start_message() -> HashMap<String, Value> {
    map_of(&[
        ("_i", Value::Str("_tiStart".into())),
        ("_t", Value::Int(MSG_REQUEST)),
        ("_c", Value::Dict(HashMap::new())),
    ])
}

/// Event (`_t`=1), not a request; no response is expected.
pub fn interest_message() -> HashMap<String, Value> {
    map_of(&[
        ("_i", Value::Str("_interest".into())),
        ("_t", Value::Int(MSG_EVENT)),
        (
            "_c",
            Value::Dict(map_of(&[(
                "_regEvents",
                Value::List(vec![Value::Str("_iMC".into())]),
            )])),
        ),
    ])
}

pub fn skip_by_message(seconds: f64) -> HashMap<String, Value> {
    map_of(&[
        ("_i", Value::Str("_mcc".into())),
        ("_t", Value::Int(MSG_REQUEST)),
        (
            "_c",
            Value::Dict(map_of(&[
                ("_mcc", Value::Int(MCC_SKIP_BY)),
                ("_skpS", Value::Float(seconds)),
            ])),
        ),
    ])
}

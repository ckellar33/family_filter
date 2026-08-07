//! Minimal schema-less protobuf (proto2) wire codec.
//!
//! This is not a general protobuf library: there is no `.proto` compiler and no
//! generated structs. It just knows how to write/read the wire-format primitives
//! (varint, fixed32, fixed64, length-delimited) tagged by field number, which is
//! all that's needed to build and parse the handful of Apple MRP messages this
//! project cares about. proto2 `extend` blocks (how MRP attaches a specific
//! message to the `ProtocolMessage` envelope) are purely a schema-level concept —
//! on the wire an extension field is indistinguishable from a regular field, so
//! callers just pick the right field number.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("truncated varint")]
    TruncatedVarint,
    #[error("varint too long (not a valid protobuf varint)")]
    VarintTooLong,
    #[error("truncated field body (need {need} bytes, have {have})")]
    TruncatedField { need: usize, have: usize },
    #[error("unknown wire type {0}")]
    UnknownWireType(u8),
    #[error("field is not valid UTF-8")]
    InvalidUtf8,
}

type Result<T> = std::result::Result<T, Error>;

const WT_VARINT: u8 = 0;
const WT_FIXED64: u8 = 1;
const WT_LEN: u8 = 2;
const WT_FIXED32: u8 = 5;

pub fn write_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// Returns (value, bytes consumed).
pub fn read_varint(data: &[u8]) -> Result<(u64, usize)> {
    let mut value: u64 = 0;
    for (i, &byte) in data.iter().enumerate() {
        if i >= 10 {
            return Err(Error::VarintTooLong);
        }
        value |= ((byte & 0x7F) as u64) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err(Error::TruncatedVarint)
}

#[derive(Debug, Clone)]
pub enum WireValue {
    Varint(u64),
    Fixed64(u64),
    LenDelim(Vec<u8>),
    Fixed32(u32),
}

impl WireValue {
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            WireValue::LenDelim(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<String> {
        self.as_bytes()
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(str::to_string)
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            WireValue::Varint(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_u64().map(|v| v as i64)
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_u64().map(|v| v as i32)
    }

    pub fn as_bool(&self) -> Option<bool> {
        self.as_u64().map(|v| v != 0)
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            WireValue::Fixed64(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            WireValue::Fixed32(bits) => Some(f32::from_bits(*bits)),
            _ => None,
        }
    }
}

/// Decode a message into an ordered list of (field_number, value) pairs.
///
/// Repeated / unknown fields are all preserved in order; for singular fields
/// (the only kind this project uses) proto semantics say the *last* occurrence
/// wins, so use [`last_field`] to look one up.
pub fn decode(mut data: &[u8]) -> Result<Vec<(u32, WireValue)>> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let (tag, consumed) = read_varint(data)?;
        data = &data[consumed..];

        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u8;

        let value = match wire_type {
            WT_VARINT => {
                let (v, n) = read_varint(data)?;
                data = &data[n..];
                WireValue::Varint(v)
            }
            WT_FIXED64 => {
                if data.len() < 8 {
                    return Err(Error::TruncatedField {
                        need: 8,
                        have: data.len(),
                    });
                }
                let bits = u64::from_le_bytes(data[..8].try_into().unwrap());
                data = &data[8..];
                WireValue::Fixed64(bits)
            }
            WT_LEN => {
                let (len, n) = read_varint(data)?;
                data = &data[n..];
                let len = len as usize;
                if data.len() < len {
                    return Err(Error::TruncatedField {
                        need: len,
                        have: data.len(),
                    });
                }
                let bytes = data[..len].to_vec();
                data = &data[len..];
                WireValue::LenDelim(bytes)
            }
            WT_FIXED32 => {
                if data.len() < 4 {
                    return Err(Error::TruncatedField {
                        need: 4,
                        have: data.len(),
                    });
                }
                let bits = u32::from_le_bytes(data[..4].try_into().unwrap());
                data = &data[4..];
                WireValue::Fixed32(bits)
            }
            other => return Err(Error::UnknownWireType(other)),
        };

        out.push((field_num, value));
    }
    Ok(out)
}

/// Last value for `field_num` (proto2 semantics: later occurrences of a
/// singular field overwrite earlier ones).
pub fn last_field(fields: &[(u32, WireValue)], field_num: u32) -> Option<&WireValue> {
    fields.iter().rev().find(|(n, _)| *n == field_num).map(|(_, v)| v)
}

/// All values for `field_num`, in order (for repeated fields).
pub fn all_fields<'a>(
    fields: &'a [(u32, WireValue)],
    field_num: u32,
) -> impl Iterator<Item = &'a WireValue> {
    fields.iter().filter(move |(n, _)| *n == field_num).map(|(_, v)| v)
}

fn write_tag(out: &mut Vec<u8>, field_num: u32, wire_type: u8) {
    out.extend(write_varint(((field_num as u64) << 3) | wire_type as u64));
}

#[derive(Default)]
pub struct MessageBuilder(Vec<u8>);

impl MessageBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn varint(mut self, field_num: u32, value: i64) -> Self {
        write_tag(&mut self.0, field_num, WT_VARINT);
        self.0.extend(write_varint(value as u64));
        self
    }

    pub fn bool(self, field_num: u32, value: bool) -> Self {
        self.varint(field_num, value as i64)
    }

    pub fn string(mut self, field_num: u32, value: &str) -> Self {
        write_tag(&mut self.0, field_num, WT_LEN);
        self.0.extend(write_varint(value.len() as u64));
        self.0.extend_from_slice(value.as_bytes());
        self
    }

    pub fn bytes(mut self, field_num: u32, value: &[u8]) -> Self {
        write_tag(&mut self.0, field_num, WT_LEN);
        self.0.extend(write_varint(value.len() as u64));
        self.0.extend_from_slice(value);
        self
    }

    /// Embed an already-encoded sub-message as a length-delimited field.
    pub fn submessage(self, field_num: u32, encoded: &[u8]) -> Self {
        self.bytes(field_num, encoded)
    }

    pub fn double(mut self, field_num: u32, value: f64) -> Self {
        write_tag(&mut self.0, field_num, WT_FIXED64);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn float(mut self, field_num: u32, value: f32) -> Self {
        write_tag(&mut self.0, field_num, WT_FIXED32);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn encode(self) -> Vec<u8> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            let encoded = write_varint(v);
            let (decoded, consumed) = read_varint(&encoded).unwrap();
            assert_eq!(decoded, v);
            assert_eq!(consumed, encoded.len());
        }
    }

    #[test]
    fn message_roundtrip() {
        let encoded = MessageBuilder::new()
            .varint(1, 42)
            .string(2, "hello")
            .double(3, 1.5)
            .bytes(4, &[1, 2, 3])
            .encode();

        let fields = decode(&encoded).unwrap();
        assert_eq!(last_field(&fields, 1).unwrap().as_i64(), Some(42));
        assert_eq!(last_field(&fields, 2).unwrap().as_string(), Some("hello".into()));
        assert_eq!(last_field(&fields, 3).unwrap().as_f64(), Some(1.5));
        assert_eq!(last_field(&fields, 4).unwrap().as_bytes(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn nested_submessage() {
        let inner = MessageBuilder::new().string(1, "child").encode();
        let outer = MessageBuilder::new().submessage(9, &inner).encode();

        let fields = decode(&outer).unwrap();
        let inner_bytes = last_field(&fields, 9).unwrap().as_bytes().unwrap();
        let inner_fields = decode(inner_bytes).unwrap();
        assert_eq!(last_field(&inner_fields, 1).unwrap().as_string(), Some("child".into()));
    }
}

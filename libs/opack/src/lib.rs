//! opack_rs — Apple-compatible OPACK serializer/deserializer
//!
//! Supported value types:
//! - Nil (Null)
//! - Bool
//! - Integer (i64)
//! - Float (f64)
//! - Decimal (small unsigned 0–39; encoded as Apple small-int)
//! - String (UTF-8)
//! - Bytes (opaque binary)
//! - List (Vec<Value>)
//! - Dict (HashMap<String, Value>)
//!
//! Encoding follows the Apple OPACK subset used by Companion / pyatv.

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(u8),
    Str(String),
    Bytes(Vec<u8>),
    ByteArray(Vec<u8>),
    List(Vec<Value>),
    Dict(HashMap<String, Value>),
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid data: {0}")]
    InvalidData(String),
    #[error("unexpected end of input")]
    Eof,
}

mod tag {
    pub const BOOL_TRUE: u8 = 0x01;
    pub const BOOL_FALSE: u8 = 0x02;
    pub const NIL: u8 = 0x04;
    pub const SMALL_INT: u8 = 0x08;
    pub const INT_1: u8 = 0x30;
    pub const INT_2: u8 = 0x31;
    pub const INT_4: u8 = 0x32;
    pub const INT_8: u8 = 0x33;
    pub const FLOAT64: u8 = 0x36;
    pub const STR: u8 = 0x40;
    pub const BYTES: u8 = 0x70;
    pub const BYTE_ARRAY: u8 = 0x90;
    pub const LIST: u8 = 0xd0;
    pub const DICT: u8 = 0xe0;
}

/// Encode a Value into OPACK bytes
pub fn encode(v: &Value) -> Result<Vec<u8>, Error> {
    let mut buf = Vec::new();
    encode_into(&mut buf, v)?;
    Ok(buf)
}

fn encode_int<W: Write>(w: &mut W, i: i64) -> Result<(), Error> {
    if (0..0x28).contains(&i) {
        w.write_u8(tag::SMALL_INT + i as u8)?;
    } else if (0..=0xFF).contains(&i) {
        w.write_u8(tag::INT_1)?;
        w.write_u8(i as u8)?;
    } else if (0..=0xFFFF).contains(&i) {
        w.write_u8(tag::INT_2)?;
        w.write_u16::<LittleEndian>(i as u16)?;
    } else if (0..=0xFFFF_FFFF).contains(&i) {
        w.write_u8(tag::INT_4)?;
        w.write_u32::<LittleEndian>(i as u32)?;
    } else {
        w.write_u8(tag::INT_8)?;
        w.write_i64::<LittleEndian>(i)?;
    }
    Ok(())
}

fn encode_into<W: Write>(w: &mut W, v: &Value) -> Result<(), Error> {
    match v {
        Value::Nil => {
            w.write_u8(tag::NIL)?;
        }
        Value::Bool(true) => {
            w.write_u8(tag::BOOL_TRUE)?;
        }
        Value::Bool(false) => {
            w.write_u8(tag::BOOL_FALSE)?;
        }
        Value::Int(i) => encode_int(w, *i)?,
        Value::Float(f) => {
            w.write_u8(tag::FLOAT64)?;
            w.write_f64::<LittleEndian>(*f)?;
        }
        Value::Decimal(d) => {
            if *d > 39 {
                return Err(Error::InvalidData("Decimal too large".to_string()));
            }
            w.write_u8(tag::SMALL_INT + *d)?;
        }
        Value::Str(s) => {
            let b = s.as_bytes();
            if b.len() <= 0x20 {
                w.write_u8(tag::STR + b.len() as u8)?;
                w.write_all(b)?;
            } else if b.len() <= 0xFF {
                w.write_u8(0x61)?;
                w.write_u8(b.len() as u8)?;
                w.write_all(b)?;
            } else if b.len() <= 0xFFFF {
                w.write_u8(0x62)?;
                w.write_u16::<LittleEndian>(b.len() as u16)?;
                w.write_all(b)?;
            } else {
                return Err(Error::InvalidData("string too long".to_string()));
            }
        }
        Value::Bytes(bv) | Value::ByteArray(bv) => {
            let bv_len = bv.len();
            if bv_len <= 32 {
                w.write_u8(tag::BYTES + bv_len as u8)?;
            } else if bv_len <= u8::MAX as usize {
                w.write_u8(tag::BYTE_ARRAY | 0x01)?;
                w.write_u8(bv_len as u8)?;
            } else if bv_len <= u16::MAX as usize {
                w.write_u8(tag::BYTE_ARRAY | 0x02)?;
                w.write_u16::<LittleEndian>(bv_len as u16)?;
            } else if bv_len <= u32::MAX as usize {
                w.write_u8(tag::BYTE_ARRAY | 0x04)?;
                w.write_u32::<LittleEndian>(bv_len as u32)?;
            } else {
                return Err(Error::InvalidData("byte array too long".to_string()));
            }
            w.write_all(bv.as_slice())?;
        }
        Value::List(arr) => {
            if arr.len() >= 0x0F {
                return Err(Error::InvalidData("list too long (max 14)".to_string()));
            }
            w.write_u8(tag::LIST + arr.len() as u8)?;
            for item in arr {
                encode_into(w, item)?;
            }
        }
        Value::Dict(map) => {
            if map.len() >= 0x0F {
                return Err(Error::InvalidData("dict too long (max 14)".to_string()));
            }
            w.write_u8(tag::DICT + map.len() as u8)?;
            for (k, v) in map {
                encode_into(w, &Value::Str(k.clone()))?;
                encode_into(w, v)?;
            }
        }
    }
    Ok(())
}

/// Decode OPACK bytes into a Value
pub fn decode(data: &[u8]) -> Result<(Value, usize), Error> {
    let mut cur = Cursor::new(data);
    let val = decode_from(&mut cur)?;
    let pos = cur.position() as usize;
    Ok((val, pos))
}

fn decode_from<R: Read>(r: &mut R) -> Result<Value, Error> {
    let mut tag_buf = [0u8; 1];
    if r.read_exact(&mut tag_buf).is_err() {
        return Err(Error::Eof);
    }
    let tag = tag_buf[0];
    match tag {
        tag::NIL => Ok(Value::Nil),
        tag::BOOL_TRUE => Ok(Value::Bool(true)),
        tag::BOOL_FALSE => Ok(Value::Bool(false)),
        0x08..=0x2F => Ok(Value::Int((tag - tag::SMALL_INT) as i64)),
        tag::INT_1 => Ok(Value::Int(r.read_u8()? as i64)),
        tag::INT_2 => Ok(Value::Int(r.read_u16::<LittleEndian>()? as i64)),
        tag::INT_4 => Ok(Value::Int(r.read_u32::<LittleEndian>()? as i64)),
        tag::INT_8 => Ok(Value::Int(r.read_i64::<LittleEndian>()?)),
        tag::FLOAT64 => Ok(Value::Float(r.read_f64::<LittleEndian>()?)),
        0x40..=0x60 => {
            let len = (tag - tag::STR) as usize;
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            let s = String::from_utf8(buf)
                .map_err(|e| Error::InvalidData(format!("invalid utf8 string: {}", e)))?;
            Ok(Value::Str(s))
        }
        0x61..=0x64 => {
            let nbytes = (tag & 0x0F) as usize;
            let mut len_buf = vec![0u8; nbytes];
            r.read_exact(&mut len_buf)?;
            let mut padded = [0u8; 8];
            padded[..nbytes].copy_from_slice(&len_buf);
            let len = u64::from_le_bytes(padded) as usize;
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            let s = String::from_utf8(buf)
                .map_err(|e| Error::InvalidData(format!("invalid utf8 string: {}", e)))?;
            Ok(Value::Str(s))
        }
        0x70..=0x90 => {
            let len = (tag - tag::BYTES) as usize;
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            Ok(Value::Bytes(buf))
        }
        0x91..=0x94 => {
            let nbytes = 1usize << ((tag & 0x0F) - 1);
            let mut len_buf = vec![0u8; nbytes];
            r.read_exact(&mut len_buf)?;
            let mut padded = [0u8; 8];
            padded[..nbytes].copy_from_slice(&len_buf);
            let len = u64::from_le_bytes(padded) as usize;
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            Ok(Value::ByteArray(buf))
        }
        0xD0..=0xDE => {
            let count = (tag - tag::LIST) as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(decode_from(r)?);
            }
            Ok(Value::List(items))
        }
        0xE0..=0xEE => {
            let count = (tag - tag::DICT) as usize;
            let mut map = HashMap::with_capacity(count);
            for _ in 0..count {
                let key = decode_from(r)?;
                let key_str = match key {
                    Value::Str(key_str) => key_str,
                    _ => {
                        return Err(Error::InvalidData(
                            "Failed to get dictionary key".to_string(),
                        ))
                    }
                };
                let value = decode_from(r)?;
                map.insert(key_str, value);
            }
            Ok(Value::Dict(map))
        }
        other => Err(Error::InvalidData(format!("unknown tag byte: 0x{:02x}", other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::FromIterator;

    #[test]
    fn roundtrip_primitive() {
        let v = Value::Int(42);
        let enc = encode(&v).unwrap();
        assert_eq!(enc, vec![0x30, 42]);
        let (dec, used) = decode(&enc).unwrap();
        assert_eq!(dec, v);
        assert_eq!(used, enc.len());
    }

    #[test]
    fn roundtrip_small_int() {
        let v = Value::Int(7);
        let enc = encode(&v).unwrap();
        assert_eq!(enc, vec![0x0F]);
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn roundtrip_float() {
        let v = Value::Float(10.0);
        let enc = encode(&v).unwrap();
        assert_eq!(enc[0], 0x36);
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn roundtrip_complex() {
        use Value::*;
        let mut dict = HashMap::new();
        dict.insert("name".to_string(), Str("alice".into()));
        dict.insert("age".to_string(), Int(30));
        dict.insert("bytes".to_string(), Bytes(vec![1, 2, 3]));
        let v = Dict(dict);
        let enc = encode(&v).unwrap();
        let (dec, used) = decode(&enc).unwrap();
        assert_eq!(dec, v);
        assert_eq!(used, enc.len());
    }

    #[test]
    fn list_and_bool_nil() {
        let v = Value::List(vec![Value::Nil, Value::Bool(true), Value::Bool(false)]);
        let enc = encode(&v).unwrap();
        assert_eq!(enc[0], 0xD3);
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn nested() {
        use Value::*;
        let nested = List(vec![
            Int(1),
            Str("x".into()),
            Dict(HashMap::from_iter(vec![("k".into(), Bytes(vec![9, 9]))])),
        ]);
        let enc = encode(&nested).unwrap();
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, nested);
    }

    #[test]
    fn decimal_encodes_as_small_int() {
        let enc = encode(&Value::Decimal(1)).unwrap();
        assert_eq!(enc, vec![0x09]);
    }
}

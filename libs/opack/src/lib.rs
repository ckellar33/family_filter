//! opack_rs — best-effort OPACK serializer/deserializer
//!
//! Supported value types:
//! - Nil (Null)
//! - Bool
//! - Integer (i64)
//! - String (UTF-8)
//! - Bytes (opaque binary)
//! - List (Vec<Value>)
//! - Dict (HashMap<String, Value>)
//!
//! Notes:
//! * Implementation follows the common subset of OPACK described in public docs.
//! * Uses little-endian for multi-byte integers, length-prefix for strings/bytes/lists/dicts.
//! * Not guaranteed to be compatible with every Apple internal variant — recommended to test
//!   against the data you're seeing (tools like pyatv / go-ios influenced this design).
//!
//! Sources: pyatv OPACK summary and go-ios opack package. :contentReference[oaicite:1]{index=1}

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
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

/// OPACK "type tags" used in this implementation (best-effort)
/// Chosen as small, understandable markers; real Apple tags may differ.
/// These values are internal to this crate and used for the on-wire encoding here.
mod tag {
    pub const NIL: u8 = 0x00;
    pub const BOOL_TRUE: u8 = 0x01;
    pub const BOOL_FALSE: u8 = 0x02;
    pub const INT: u8 = 0x10;
    pub const DEC: u8 = 0x08;
    pub const STR: u8 = 0x40;
    pub const BYTES: u8 = 0x70;
    pub const BYTE_ARRAY: u8 = 0x90;
    pub const LIST: u8 = 0x30;
    pub const DICT: u8 = 0xe0;
}

/// Encode a Value into OPACK-like bytes
pub fn encode(v: &Value) -> Result<Vec<u8>, Error> {
    let mut buf = Vec::new();
    encode_into(&mut buf, v)?;
    Ok(buf)
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
        Value::Int(i) => {
            w.write_u8(tag::INT)?;
            // write as little-endian 64-bit integer
            w.write_i64::<LittleEndian>(*i)?;
        }
        Value::Decimal(d) => {
            if *d > 39 {
                return Err(Error::InvalidData("Decimal too large".to_string()))
            }
            let tag = tag::DEC + *d;
            println!("{:?}", tag);
            w.write_u8(tag)?;
        }
        Value::Str(s) => {
            let tag = tag::STR + s.len() as u8;
            w.write_u8(tag)?;
            let b = s.as_bytes();
            w.write_all(b)?;
        }
        Value::Bytes(bv) => {
            let bv_len = bv.len();
            if bv_len <= 32 {
                w.write_u8(tag::BYTES + bv_len as u8)?;
            } else if bv_len <= u8::MAX as usize {
                let _ = w.write_u8(tag::BYTE_ARRAY | 0x01);
                let _ = w.write_u8(bv_len as u8);
            } else if bv_len <= u16::MAX as usize {
                let _ = w.write_u8(tag::BYTE_ARRAY | 0x02);
                let _ = w.write_u16::<LittleEndian>(bv_len as u16);
            } else if bv_len <= 0xFF_FFFF {
                let le_bytes = bv_len.to_le_bytes();
                w.write_u8(tag::BYTE_ARRAY | 0x03)?;
                w.write_all(&le_bytes[0..3])?;
            } else if bv_len <= u32::MAX as usize {
                w.write_u8(tag::BYTE_ARRAY | 0x04)?;
                w.write_u32::<LittleEndian>(bv_len as u32)?;
            }
            w.write_all(bv.as_slice())?;
        },
        Value::ByteArray(byte_array) => {

        },
        Value::List(arr) => {
            w.write_u8(tag::LIST)?;
            w.write_u32::<LittleEndian>(arr.len() as u32)?;
            for item in arr {
                encode_into(w, item)?;
            }
        }
        Value::Dict(map) => {
            let tag = tag::DICT + map.len() as u8;
            w.write_u8(tag)?;
            for (k, v) in map {
                encode_into(w, &Value::Str(k.clone()))?;
                encode_into(w, v)?;
            }
            // let mut keys: Vec<_> = map.keys().cloned().collect();
            // keys.sort();
            // for k in keys {
            //     let v2 = &map[&k];
            //     // encode key as string (length-prefixed)
            //     let key_bytes = k.as_bytes();
            //     w.write_u32::<LittleEndian>(key_bytes.len() as u32)?;
            //     w.write_all(key_bytes)?;
            //     // encode value recursively
            //     encode_into(w, v2)?;
            // }
        }
    }
    Ok(())
}

/// Decode bytes produced by `encode` into a Value
pub fn decode(data: &[u8]) -> Result<(Value, usize), Error> {
    let mut cur = Cursor::new(data);
    let val = decode_from(&mut cur)?;
    let pos = cur.position() as usize;
    Ok((val, pos))
}

fn decode_from<R: Read>(r: &mut R) -> Result<Value, Error> {
    // read tag
    let mut tag_buf = [0u8; 1];
    if r.read_exact(&mut tag_buf).is_err() {
        return Err(Error::Eof);
    }
    let tag = tag_buf[0];
    println!("tag: {}", tag);
    match tag {
        tag::NIL => Ok(Value::Nil),
        tag::BOOL_TRUE => Ok(Value::Bool(true)),
        tag::BOOL_FALSE => Ok(Value::Bool(false)),
        tag::DEC..0x2F => {
            let value = 39 - tag;
            Ok(Value::Decimal(value))
        },
        tag::INT => {
            let i = r.read_i64::<LittleEndian>()?;
            Ok(Value::Int(i))
        },
        tag::STR..0x4F => {
            let len = tag - tag::STR;
            let mut buf = vec![0u8; len as usize];
            r.read_exact(&mut buf)?;
            let s = String::from_utf8(buf)
                .map_err(|e| Error::InvalidData(format!("invalid utf8 string: {}", e)))?;
            Ok(Value::Str(s))
        }
        tag::BYTES..0x7F => {
            let len = tag - tag::BYTES;
            let mut buf = vec![0u8; len as usize];
            r.read_exact(&mut buf)?;
            Ok(Value::Bytes(buf))
        },
        tag::BYTE_ARRAY..0x94 => {
            let data_byte_len = tag - tag::BYTE_ARRAY;
            let mut buf = vec![0u8; data_byte_len as usize];
            r.read_exact(&mut buf)?;
            let len = match data_byte_len {
                1 => buf[0] as u32,
                2 => u16::from_le_bytes([buf[0], buf[1]]) as u32,
                3 => u32::from_le_bytes([0, buf[0], buf[1], buf[2]]),
                4 => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
                _=> 0
            };
            let mut final_buf = vec![0u8; len as usize];
            println!("reading data len");
            r.read_exact(&mut final_buf)?;
            // process the buf size to get actual length of byte array
            Ok(Value::ByteArray(final_buf))
        },
        tag::LIST => {
            let count = r.read_u32::<LittleEndian>()?;
            let mut items = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let it = decode_from(r)?;
                items.push(it);
            }
            Ok(Value::List(items))
        }
        tag::DICT..0xEF => {
            let count = tag - tag::DICT;
            let mut map = HashMap::with_capacity(count as usize);
            for _ in 0..count {
                // let mut kb = vec![0u8; klen as usize];
                // r.read_exact(&mut kb)?;
                let key = decode_from(r)?;
                let key_str = match key {
                    Value::Str(key_str) => {
                        key_str
                    },
                    _ => return Err(Error::InvalidData("Failed to get dictionary key".to_string()))
                };
                let value = decode_from(r)?;
                // // read key length, key bytes
                // let klen = r.read_u32::<LittleEndian>()?;
                // let key = String::from_utf8(kb)
                //     .map_err(|e| Error::InvalidData(format!("invalid utf8 key: {}", e)))?;
                // let val = decode_from(r)?;
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
        let (dec, used) = decode(&enc).unwrap();
        assert_eq!(dec, v);
        assert_eq!(used, enc.len());
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
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn nested() {
        use Value::*;
        let nested = List(vec![
            Int(-1),
            Str("x".into()),
            Dict(HashMap::from_iter(vec![
                ("k".into(), Bytes(vec![9, 9])),
            ])),
        ]);
        let enc = encode(&nested).unwrap();
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, nested);
    }

    #[test]
    fn dictionary() {
        let vec = vec![("bool".to_string(), Value::Bool(false)), ("str".to_string(), Value::Str("abc".to_string())), ("int".to_string(), Value::Int(1234))];
        let v = Value::Dict(vec.into_iter().collect());
        let enc = encode(&v).unwrap();
        println!("{:x?}", enc);
        let (dec, _) = decode(&enc).unwrap();
        println!("{:?}", dec);
    }
}

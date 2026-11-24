//! Value kinds and their byte layouts.
//!
//! A hub entry is `(id, kind, bytes)`. The hub itself is payload-agnostic; the kind only tells
//! consumers how to decode the bytes. Scalar layouts follow the message-rs wire conventions:
//! little-endian, `bool` one byte (0 = false), `String` raw UTF-8 (the length is the payload
//! length), `Bytes` raw. `Message` payloads are message-rs encodings and the id is the message
//! key. The u32 discriminants are stable (they cross the FFI boundary as u8).

use std::fmt;

/// How the payload bytes of a data item are decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ValueKind {
    Bool = 0,
    U8 = 1,
    I8 = 2,
    U16 = 3,
    I16 = 4,
    U32 = 5,
    I32 = 6,
    U64 = 7,
    I64 = 8,
    F32 = 9,
    F64 = 10,
    String = 11,
    Bytes = 12,
    /// A message-rs encoding; the item id is the message key.
    Message = 13,
}

impl ValueKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => ValueKind::Bool,
            1 => ValueKind::U8,
            2 => ValueKind::I8,
            3 => ValueKind::U16,
            4 => ValueKind::I16,
            5 => ValueKind::U32,
            6 => ValueKind::I32,
            7 => ValueKind::U64,
            8 => ValueKind::I64,
            9 => ValueKind::F32,
            10 => ValueKind::F64,
            11 => ValueKind::String,
            12 => ValueKind::Bytes,
            13 => ValueKind::Message,
            _ => return None,
        })
    }

    /// Expected payload length for fixed-size kinds (None for variable-length ones).
    pub fn fixed_len(self) -> Option<usize> {
        Some(match self {
            ValueKind::Bool | ValueKind::U8 | ValueKind::I8 => 1,
            ValueKind::U16 | ValueKind::I16 => 2,
            ValueKind::U32 | ValueKind::I32 | ValueKind::F32 => 4,
            ValueKind::U64 | ValueKind::I64 | ValueKind::F64 => 8,
            _ => return None,
        })
    }
}

/// A decoded data item.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    /// A message-rs payload; the key is carried by the item id.
    Message(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueError(pub &'static str);

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ValueError {}

fn le<const N: usize>(b: &[u8]) -> Result<[u8; N], ValueError> {
    b.try_into().map_err(|_| ValueError("payload length does not match the kind"))
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Bool(_) => ValueKind::Bool,
            Value::U8(_) => ValueKind::U8,
            Value::I8(_) => ValueKind::I8,
            Value::U16(_) => ValueKind::U16,
            Value::I16(_) => ValueKind::I16,
            Value::U32(_) => ValueKind::U32,
            Value::I32(_) => ValueKind::I32,
            Value::U64(_) => ValueKind::U64,
            Value::I64(_) => ValueKind::I64,
            Value::F32(_) => ValueKind::F32,
            Value::F64(_) => ValueKind::F64,
            Value::String(_) => ValueKind::String,
            Value::Bytes(_) => ValueKind::Bytes,
            Value::Message(_) => ValueKind::Message,
        }
    }

    /// Encode into the hub's byte layout (message-rs scalar conventions).
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Value::Bool(v) => vec![*v as u8],
            Value::U8(v) => vec![*v],
            Value::I8(v) => vec![*v as u8],
            Value::U16(v) => v.to_le_bytes().to_vec(),
            Value::I16(v) => v.to_le_bytes().to_vec(),
            Value::U32(v) => v.to_le_bytes().to_vec(),
            Value::I32(v) => v.to_le_bytes().to_vec(),
            Value::U64(v) => v.to_le_bytes().to_vec(),
            Value::I64(v) => v.to_le_bytes().to_vec(),
            Value::F32(v) => v.to_le_bytes().to_vec(),
            Value::F64(v) => v.to_le_bytes().to_vec(),
            Value::String(s) => s.as_bytes().to_vec(),
            Value::Bytes(b) | Value::Message(b) => b.clone(),
        }
    }

    /// Decode a payload of the given kind. Fixed-size kinds reject wrong lengths; `String`
    /// requires valid UTF-8. `Message` bytes are returned as-is (decoding is the schema's job).
    pub fn decode(kind: ValueKind, payload: &[u8]) -> Result<Value, ValueError> {
        Ok(match kind {
            ValueKind::Bool => Value::Bool(!payload.is_empty() && payload[0] != 0),
            ValueKind::U8 => Value::U8(le::<1>(payload)?[0]),
            ValueKind::I8 => Value::I8(le::<1>(payload)?[0] as i8),
            ValueKind::U16 => Value::U16(u16::from_le_bytes(le::<2>(payload)?)),
            ValueKind::I16 => Value::I16(i16::from_le_bytes(le::<2>(payload)?)),
            ValueKind::U32 => Value::U32(u32::from_le_bytes(le::<4>(payload)?)),
            ValueKind::I32 => Value::I32(i32::from_le_bytes(le::<4>(payload)?)),
            ValueKind::U64 => Value::U64(u64::from_le_bytes(le::<8>(payload)?)),
            ValueKind::I64 => Value::I64(i64::from_le_bytes(le::<8>(payload)?)),
            ValueKind::F32 => Value::F32(f32::from_le_bytes(le::<4>(payload)?)),
            ValueKind::F64 => Value::F64(f64::from_le_bytes(le::<8>(payload)?)),
            ValueKind::String => Value::String(
                std::str::from_utf8(payload).map_err(|_| ValueError("string payload is not valid utf-8"))?.to_owned(),
            ),
            ValueKind::Bytes => Value::Bytes(payload.to_vec()),
            ValueKind::Message => Value::Message(payload.to_vec()),
        })
    }
}

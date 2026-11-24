//! Codec for the basic value kinds: little-endian scalars, raw UTF-8, raw bytes.

use std::fmt;

/// Stable u8 discriminants (they cross FFI); the order matches `message_core::SimpleType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ValueKind {
    Bool = 0,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
    String = 11,
    Bytes = 12,
    Message = 13,
}

impl ValueKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Bool,
            1 => Self::U8,
            2 => Self::I8,
            3 => Self::U16,
            4 => Self::I16,
            5 => Self::U32,
            6 => Self::I32,
            7 => Self::U64,
            8 => Self::I64,
            9 => Self::F32,
            10 => Self::F64,
            11 => Self::String,
            12 => Self::Bytes,
            13 => Self::Message,
            _ => return None,
        })
    }

    /// Payload width of the scalar kinds; `None` for string, bytes and message.
    pub fn fixed_len(self) -> Option<usize> {
        Some(match self {
            Self::Bool | Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
            Self::String | Self::Bytes | Self::Message => return None,
        })
    }
}

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
    Message(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueError(pub &'static str);

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ValueError {}

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

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Value::Bool(v) => vec![u8::from(*v)],
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

    pub fn decode(kind: ValueKind, payload: &[u8]) -> Result<Value, ValueError> {
        fn fixed<const N: usize>(payload: &[u8]) -> Result<[u8; N], ValueError> {
            payload
                .try_into()
                .map_err(|_| ValueError("wrong payload length"))
        }
        Ok(match kind {
            ValueKind::Bool => Value::Bool(fixed::<1>(payload)?[0] != 0),
            ValueKind::U8 => Value::U8(fixed::<1>(payload)?[0]),
            ValueKind::I8 => Value::I8(fixed::<1>(payload)?[0] as i8),
            ValueKind::U16 => Value::U16(u16::from_le_bytes(fixed(payload)?)),
            ValueKind::I16 => Value::I16(i16::from_le_bytes(fixed(payload)?)),
            ValueKind::U32 => Value::U32(u32::from_le_bytes(fixed(payload)?)),
            ValueKind::I32 => Value::I32(i32::from_le_bytes(fixed(payload)?)),
            ValueKind::U64 => Value::U64(u64::from_le_bytes(fixed(payload)?)),
            ValueKind::I64 => Value::I64(i64::from_le_bytes(fixed(payload)?)),
            ValueKind::F32 => Value::F32(f32::from_le_bytes(fixed(payload)?)),
            ValueKind::F64 => Value::F64(f64::from_le_bytes(fixed(payload)?)),
            ValueKind::String => Value::String(
                String::from_utf8(payload.to_vec()).map_err(|_| ValueError("invalid utf-8"))?,
            ),
            ValueKind::Bytes => Value::Bytes(payload.to_vec()),
            ValueKind::Message => Value::Message(payload.to_vec()),
        })
    }
}

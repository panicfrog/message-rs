//! Strict-validation primitives called by derive-generated `validate` code.
//!
//! Every helper returns a leaf [`ValidationError`]; the generated code wraps it with the field
//! path (`ValidationError::nested`), so the leaves carry no field name themselves. After `Ok`,
//! no accessor on the reader can panic.

use crate::errors::ValidationError;
use crate::rt::{find_ext_block, read_i32, read_slot, read_u16, read_u32};
use crate::traits::EnumType;
use crate::HEADER_LEN;

pub type VResult = Result<(), ValidationError>;

/// Header checks shared by every message: length, key, and the fixed_len this reader was built for.
pub fn validate_header(buf: &[u8], key: u32, fixed_len: usize) -> VResult {
    let need = HEADER_LEN + fixed_len;
    if buf.len() < need {
        return Err(ValidationError::TooShort { need, got: buf.len() });
    }
    let got = read_u32(buf, 0);
    if got != key {
        return Err(ValidationError::KeyMismatch { expected: key, got });
    }
    let got = read_u16(buf, 4) as usize;
    if got != fixed_len {
        return Err(ValidationError::FixedLenMismatch { expected: fixed_len, got });
    }
    Ok(())
}

/// Read the slot at `pos` and check it stays inside the payload; returns `(offset, length)`.
pub fn check_bytes_slot(payload: &[u8], pos: usize) -> Result<(usize, usize), ValidationError> {
    let (o, l) = read_slot(payload, pos);
    let (o, l) = (o as usize, l as usize);
    match o.checked_add(l) {
        Some(end) if end <= payload.len() => Ok((o, l)),
        _ => Err(ValidationError::SlotOutOfBounds { off: o, len: l, end: payload.len() }),
    }
}

pub fn check_str_slot(payload: &[u8], pos: usize) -> VResult {
    let (o, l) = check_bytes_slot(payload, pos)?;
    match std::str::from_utf8(&payload[o..o + l]) {
        Ok(_) => Ok(()),
        Err(_) => Err(ValidationError::InvalidUtf8),
    }
}

/// Read the slot at `pos` whose length is an element count of `stride`-sized elements; returns
/// `(offset, count)`.
pub fn check_region(payload: &[u8], pos: usize, stride: usize) -> Result<(usize, usize), ValidationError> {
    let (o, l) = read_slot(payload, pos);
    let (o, l) = (o as usize, l as usize);
    let end = l.checked_mul(stride).and_then(|n| n.checked_add(o));
    match end {
        Some(end) if end <= payload.len() => Ok((o, l)),
        _ => Err(ValidationError::SlotOutOfBounds { off: o, len: l, end: payload.len() }),
    }
}

/// The i32 at `pos` must be a known variant of `E`.
pub fn check_enum<E: EnumType>(payload: &[u8], pos: usize) -> VResult {
    let v = read_i32(payload, pos);
    if E::__try_from_i32(v).is_none() {
        return Err(ValidationError::UnknownEnumValue { enum_name: E::NAME, value: v });
    }
    Ok(())
}

/// Locate an extension block the writer must have written (version <= header version).
pub fn check_ext_block(payload: &[u8], versions: &[u16], header_version: u8, target: u16, min_len: usize) -> Result<(usize, usize), ValidationError> {
    find_ext_block(payload, versions, header_version, target, min_len)
        .ok_or(ValidationError::ExtBlock { version: target })
}

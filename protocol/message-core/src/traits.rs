//! Trait contracts implemented by the derive macros.

use crate::descriptor::{EnumDescriptor, MessageDescriptor};
use crate::rt::EncBuf;

/// Fixed layout constants. Both structs and enums implement this (enums are pinned to 4/4) so
/// parents can do symbolic constant layout math via `<T as Fixed>::FIXED_LEN`.
pub trait Fixed {
    const FIXED_LEN: usize;
    const MAX_ALIGN: usize;
}

/// Struct encoding: write the fixed region into `[origin, origin + FIXED_LEN)` (including bitmap
/// and slot back-patching); var data is appended at the buffer end in declaration-order DFS. Slot
/// values are always payload-relative.
/// Enums implement this too (4-byte i32).
pub trait StructEncode: Fixed {
    fn __write_fixed(&self, buf: &mut EncBuf, origin: usize);
}

/// Struct/enum decoding: build a zero-copy Reader at the given payload origin (or return the
/// enum value directly). `__owned` converts a Reader back into an owned value.
pub trait StructDecode: Fixed + Sized {
    type Reader<'a>;
    fn __reader<'a>(payload: &'a [u8], origin: usize, header_version: u8) -> Self::Reader<'a>;
    fn __owned(reader: &Self::Reader<'_>) -> Self;
}

/// Top-level message type.
pub trait Message: Fixed + StructEncode + StructDecode {
    const KEY: u32;
    /// All extension versions of this schema (ascending; empty slice when there are none).
    const EXT_VERSIONS: &'static [u16];
    fn __descriptor() -> &'static MessageDescriptor;
}

/// Enum types (i32-backed).
pub trait EnumType: Sized + Copy {
    const NAME: &'static str;
    fn __to_i32(self) -> i32;
    /// Panics on unknown values (validate untrusted data before reading).
    fn __from_i32(v: i32) -> Self;
    fn __descriptor() -> &'static EnumDescriptor;
}

/// Link-time registry entry: the export binary collects all descriptors via
/// `inventory::iter::<SchemaEntry>()`. Stores fn pointers to satisfy submit!'s const requirement;
/// iteration calls them to obtain descriptors.
pub enum SchemaEntry {
    Message(fn() -> &'static MessageDescriptor),
    Enum(fn() -> &'static EnumDescriptor),
}

inventory::collect!(SchemaEntry);

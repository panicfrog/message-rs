//! Trait contracts implemented by the derive macros.

use crate::descriptor::{EnumDescriptor, MessageDescriptor};
use crate::rt::EncBuf;
use crate::ValidationError;

/// Fixed layout constants. Both structs and enums implement this (enums are pinned to 4/4) so
/// parents can do symbolic constant layout math via `<T as Fixed>::FIXED_LEN`. The derive macro
/// cannot tell a struct path from an enum path at expansion time; generated code branches on the
/// `IS_ENUM` / `HAS_EXT` consts instead.
pub trait Fixed {
    const FIXED_LEN: usize;
    const MAX_ALIGN: usize;
    /// True for i32-backed enums (4-byte inline), false for structs (slot-shaped instances).
    const IS_ENUM: bool = false;
    /// True when this struct has versioned (since >= 2) fields, i.e. array/map containers of it
    /// are written in instance mode (shape bit set).
    const HAS_EXT: bool = false;
}

/// Struct encoding: write the fixed region into `[origin, origin + FIXED_LEN)` (including bitmap
/// and slot back-patching); var data is appended at the buffer end in declaration-order DFS. Slot
/// values are always payload-relative.
/// Enums implement this too (4-byte i32).
pub trait StructEncode: Fixed {
    fn __write_fixed(&self, buf: &mut EncBuf, origin: usize);

    /// Self-contained instance bytes: fixed region, own var data and the extension-block chain.
    /// Returns the absolute start; the caller back-patches the referencing slot with
    /// `{ payload_rel(start), payload_len() - payload_rel(start) }`. Only structs implement it.
    fn __write_instance(&self, buf: &mut EncBuf) -> usize {
        let _ = buf;
        unreachable!("__write_instance is only implemented for structs")
    }
}

/// Struct/enum decoding: build a zero-copy Reader spanning `[origin, end)` of the payload (or
/// return the enum value directly). `end` is the instance extent for self-contained readers and
/// the payload length for top-level / open-element readers. `__owned` converts a Reader back
/// into an owned value.
pub trait StructDecode: Fixed + Sized {
    type Reader<'a>;
    fn __reader<'a>(payload: &'a [u8], origin: usize, end: usize, header_version: u8) -> Self::Reader<'a>;
    fn __owned(reader: &Self::Reader<'_>) -> Self;
    /// Strictly check the value within `[origin, end)` (enums: known discriminant; structs: every
    /// slot, string and nested value reachable from the fixed region, plus the instance's
    /// extension chain when present). Leaf errors are returned un-nested; the caller wraps them
    /// with the field path.
    fn __validate(payload: &[u8], origin: usize, end: usize, header_version: u8) -> Result<(), ValidationError>;
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
    /// Non-panicking counterpart of `__from_i32`.
    fn __try_from_i32(v: i32) -> Option<Self>;
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

//! message-core: IR data model, version-aware layout planner, wire-format constants and the runtime base.
//!
//! Single source of truth for the protocol: descriptors emitted by the derive macro,
//! the exported JSON IR, and the layout data consumed by every language backend.
//!
//! Layout invariants (see SPEC.md at the repository root):
//! - the key lives only in the header; alignment and layout never depend on it;
//! - payload is little-endian; the fixed region holds scalars/enums inline and an 8-byte slot for
//!   everything else (strings, bytes, arrays, maps, structs); offsets are compile-time constants;
//! - a struct field is a slot referencing a self-contained instance
//!   `[ bitmap | fixed fields | var data | ext blocks... ]`; instance slots' len covers the whole
//!   instance including its extension chain, so every struct can gain `since >= 2` fields;
//! - array/map region slots carry a shape bit in `len`'s high bit (uniform footprints vs
//!   per-element instance slots);
//! - fields with `since >= 2` belong to the extension block of that version; the base layout is frozen forever;
//! - an extension block is `[ bitmap | fixed fields | var data | since:u16 | block_len:u32 ]`
//!   with the trailer at the tail; readers locate blocks by walking back from the region end.

mod descriptor;
mod errors;
mod layout;
mod registry;
mod rt;
mod traits;
mod types;
mod validate;

pub use descriptor::{
    EnumDescriptor, EnumVariant, ExtBlockLayout, FieldDescriptor, MessageDescriptor,
    MessageLayout, SchemaSet, SINCE_BASE,
};
pub use errors::{SchemaError, ValidationError};
pub use layout::{plan_schema, set_to_json_string};
pub use registry::{check_consistency, collect_raw, collect_schema};
pub use rt::{
    find_ext_block, find_ext_block_bounded, read_bitmap_bit, read_bool, read_f32, read_f64,
    read_i16, read_i32, read_i64, read_slot, read_u16, read_u32, read_u64, read_u8, slot_slice,
    BytesListView, EncBuf, ScalarListView, SlotListView, StrideView, StringListView,
    StructEvoListView,
};
pub use traits::{EnumType, Fixed, Message, SchemaEntry, StructDecode, StructEncode};
pub use types::{
    align_up_usize, element_size, field_size, map_entry_layouts, map_entry_size, type_align,
    value_has_ext, ArrayBody, MapBody, MapEntryLayouts, SimpleType, Type,
};
pub use validate::{
    check_bytes_slot, check_enum, check_ext_block, check_extent, check_instance_ext_block,
    check_instance_slot, check_region, check_str_slot, read_region_slot, validate_header, VResult,
};

/// Re-exported for derive-generated code (avoids a direct inventory dep in user crates).
pub use inventory as __inventory;

/// Envelope header length (bytes): `key:u32 | fixed_len:u16 | version:u8 | flags:u8`, little-endian.
/// version = the writer's highest extension version (0 = base only).
pub const HEADER_LEN: usize = 8;

/// Length of a var-length reference slot `{ offset: u32, len: u32 }`.
pub const SLOT_LEN: usize = 8;

/// Length of the extension-block trailer `since:u16 | block_len:u32`; block_len covers the
/// whole block (including the trailer itself).
pub const EXT_TRAILER_LEN: usize = 6;

/// High bit of a slot's `len`: the element shape flag of array/map regions (1 = elements are
/// self-contained instances referenced by per-element slots).
pub const SHAPE_BIT: u32 = 1 << 31;

/// Mask of the count bits of a slot's `len` (the complement of [`SHAPE_BIT`]).
pub const LEN_MASK: u32 = 0x7FFF_FFFF;

/// Header version sentinel passed by instance readers: no extension version is promised inside
/// an instance, so the promise check (`since > header_version` ⇒ invalid) is always disabled.
pub const INSTANCE_HV: u8 = u8::MAX;

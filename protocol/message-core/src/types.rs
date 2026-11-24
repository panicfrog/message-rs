//! Type-system model: the full V1 type set.

use crate::descriptor::SchemaSet;
use crate::errors::SchemaError;
use crate::SLOT_LEN;
use serde::{Deserialize, Serialize};

/// Simple types: fixed-size scalars plus the string/bytes slot-shaped primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SimpleType {
    Bool,
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
    String,
    Bytes,
}

impl SimpleType {
    /// Fixed footprint (bool..=f64 use their own width; string/bytes contribute one slot as array elements).
    pub const fn fixed_size(self) -> usize {
        match self {
            SimpleType::Bool | SimpleType::U8 | SimpleType::I8 => 1,
            SimpleType::U16 | SimpleType::I16 => 2,
            SimpleType::U32 | SimpleType::I32 | SimpleType::F32 => 4,
            SimpleType::U64 | SimpleType::I64 | SimpleType::F64 => 8,
            SimpleType::String | SimpleType::Bytes => SLOT_LEN,
        }
    }

    pub const fn is_scalar(self) -> bool {
        !matches!(self, SimpleType::String | SimpleType::Bytes)
    }
}

/// Array body: `elem` is the element type; `fixed = Some(n)` means a fixed-length array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArrayBody {
    pub elem: Box<Type>,
    pub fixed: Option<u32>,
}

/// Map body: KV pairs expressed as an array of a synthetic struct.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapBody {
    pub key: Box<Type>,
    pub value: Box<Type>,
}

/// Field types. JSON shape: scalars are strings (`"u64"`), composites are objects
/// (`{"enum":"Color"}` / `{"struct":"Profile"}` / `{"array":{..}}` / `{"map":{..}}`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Type {
    Simple(SimpleType),
    Enum {
        #[serde(rename = "enum")]
        name: String,
    },
    Struct {
        #[serde(rename = "struct")]
        name: String,
    },
    Array {
        #[serde(rename = "array")]
        body: ArrayBody,
    },
    Map {
        #[serde(rename = "map")]
        body: MapBody,
    },
}

impl From<SimpleType> for Type {
    fn from(s: SimpleType) -> Self {
        Type::Simple(s)
    }
}

impl Type {
    pub fn enumeration(name: impl Into<String>) -> Type {
        Type::Enum { name: name.into() }
    }

    pub fn structure(name: impl Into<String>) -> Type {
        Type::Struct { name: name.into() }
    }

    pub fn array(elem: Type, fixed: Option<u32>) -> Type {
        Type::Array {
            body: ArrayBody { elem: Box::new(elem), fixed },
        }
    }

    pub fn vec(elem: Type) -> Type {
        Self::array(elem, None)
    }

    pub fn map(key: Type, value: Type) -> Type {
        Type::Map {
            body: MapBody { key: Box::new(key), value: Box::new(value) },
        }
    }

    /// Whether the element shape is "slot-shaped" (string/bytes/map/dynamic array/struct): such
    /// elements contribute a reference slot inside the array data region instead of an inline value.
    /// Enum fields are 4-byte inline, but fixed arrays of enums are still slot-shaped: the derive
    /// macro cannot tell an enum path from a struct path, so both are treated uniformly as
    /// slot-shaped elements (dynamic-array encoding with the element count as the length).
    pub fn is_slot_shaped(&self) -> bool {
        match self {
            Type::Simple(s) => !s.is_scalar(),
            Type::Map { .. } | Type::Struct { .. } => true,
            Type::Array { body } => {
                body.fixed.is_none()
                    || body.elem.is_slot_shaped()
                    || matches!(&*body.elem, Type::Enum { .. })
            }
            Type::Enum { .. } => false,
        }
    }
}

/// Per-element footprint inside the array data region (the stride).
///
/// Convention: the array data region is the contiguous repetition of the element footprint;
/// string/bytes/nested dynamic arrays contribute an 8-byte slot, structs contribute their fixed
/// region (bitmap and slots included). Maps cannot be array elements in V1.
pub fn element_size(ty: &Type, set: &SchemaSet) -> Result<usize, SchemaError> {
    match ty {
        Type::Simple(s) => Ok(s.fixed_size()),
        Type::Enum { .. } => Ok(4),
        Type::Struct { name } => set.planned_fixed_len(name),
        Type::Array { body } => match body.fixed {
            Some(n) => Ok(n as usize * element_size(&body.elem, set)?),
            None => Ok(SLOT_LEN),
        },
        Type::Map { .. } => Err(SchemaError::InvalidElement(ty_debug(ty))),
    }
}

/// Both layouts of a synthetic KV entry: the open (shape=0) pair and the instance (shape=1)
/// pair. For non-struct values the two pairs are identical; for struct values the open pair
/// inlines the struct's fixed region while the instance pair holds one 8-byte reference slot.
pub struct MapEntryLayouts {
    pub open_value_offset: usize,
    pub open_stride: usize,
    pub inst_value_offset: usize,
    pub inst_stride: usize,
}

/// Both map entry layouts for a key/value pair. Requires value structs to be planned
/// (the open pair references their fixed length).
pub fn map_entry_layouts(key: &Type, value: &Type, set: &SchemaSet) -> Result<MapEntryLayouts, SchemaError> {
    validate_map_key(key)?;
    let k_sz = k_size(key);
    let k_align = type_align(key, set)?;
    let inst_v_off = align_up_usize(k_sz, 4);
    let inst_stride = align_up_usize(inst_v_off + SLOT_LEN, k_align.max(4));
    let (open_v_off, open_stride) = match value {
        Type::Struct { name } => {
            let fixed = set.planned_fixed_len(name)?;
            let max_align = set.planned_max_align(name)?.max(1);
            let v_off = align_up_usize(k_sz, max_align);
            (v_off, align_up_usize(v_off + fixed, k_align.max(max_align)))
        }
        _ => {
            let v_align = type_align(value, set)?;
            let v_inline = field_size(value, set)?;
            let v_off = align_up_usize(k_sz, v_align);
            (v_off, align_up_usize(v_off + v_inline, k_align.max(v_align)))
        }
    };
    Ok(MapEntryLayouts {
        open_value_offset: open_v_off,
        open_stride,
        inst_value_offset: inst_v_off,
        inst_stride,
    })
}

/// Whether `value` is a struct that has extension blocks (instance-shaped when written today).
pub fn value_has_ext(value: &Type, set: &SchemaSet) -> bool {
    match value {
        Type::Struct { name } => {
            set.message(name).is_some_and(|m| !m.layout.ext_blocks.is_empty())
        }
        _ => false,
    }
}

/// Writer-current stride of a synthetic KV entry: instance pair for evolved struct values,
/// open pair otherwise.
pub fn map_entry_size(key: &Type, value: &Type, set: &SchemaSet) -> Result<usize, SchemaError> {
    let l = map_entry_layouts(key, value, set)?;
    Ok(if value_has_ext(value, set) { l.inst_stride } else { l.open_stride })
}

fn k_size(key: &Type) -> usize {
    match key {
        Type::Simple(s) => s.fixed_size(),
        Type::Enum { .. } => 4,
        _ => unreachable!("validated by validate_map_key"),
    }
}

/// Field footprint in the fixed region: inline byte size, or 8 for slot shapes.
/// Structs are slot-shaped in v2 (self-contained instances referenced by a slot).
pub fn field_size(ty: &Type, set: &SchemaSet) -> Result<usize, SchemaError> {
    match ty {
        Type::Simple(s) if s.is_scalar() => Ok(s.fixed_size()),
        Type::Simple(_) => Ok(SLOT_LEN),
        Type::Enum { .. } => Ok(4),
        Type::Struct { .. } => Ok(SLOT_LEN),
        Type::Array { body } => match body.fixed {
            Some(n) if !body.elem.is_slot_shaped() => {
                Ok(n as usize * element_size(&body.elem, set)?)
            }
            _ => Ok(SLOT_LEN),
        },
        Type::Map { .. } => Ok(SLOT_LEN),
    }
}

/// Alignment requirement of a type.
pub fn type_align(ty: &Type, set: &SchemaSet) -> Result<usize, SchemaError> {
    match ty {
        Type::Simple(s) => Ok(if s.is_scalar() { s.fixed_size() } else { 4 }),
        Type::Enum { .. } => Ok(4),
        Type::Struct { .. } => Ok(4),
        Type::Array { body } if body.fixed.is_some() && !body.elem.is_slot_shaped() => {
            type_align(&body.elem, set)
        }
        _ => Ok(4),
    }
}

/// Valid map key types: fixed-size scalars, enums, or string (a string key occupies one slot in the KV entry).
pub fn validate_map_key(key: &Type) -> Result<(), SchemaError> {
    match key {
        Type::Simple(s) if s.is_scalar() || *s == SimpleType::String => Ok(()),
        Type::Enum { .. } => Ok(()),
        _ => Err(SchemaError::InvalidMapKey(type_debug(key))),
    }
}

pub const fn align_up_usize(v: usize, align: usize) -> usize {
    if align <= 1 {
        v
    } else {
        (v + align - 1) / align * align
    }
}

fn ty_debug(ty: &Type) -> String {
    format!("{ty:?}")
}

fn type_debug(ty: &Type) -> String {
    format!("{ty:?}")
}

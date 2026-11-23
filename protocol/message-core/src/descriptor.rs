//! Descriptor (IR) data model: produced by the derive macro, filled in by the layout
//! planner, serialized as JSON IR, and consumed by every language backend.
//!
//! JSON shape (camelCase, field order = declaration order, stable output): a
//! `MessageDescriptor` carries both its inputs (name/type/since/optional/...) and the
//! computed results (offset/size/slot/elementSize plus the layout summary).

use crate::types::Type;
use serde::{Deserialize, Serialize};

/// Base version: fields without a `since` marker belong to the base (layout frozen forever).
pub const SINCE_BASE: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldDescriptor {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    /// Schema version this field belongs to; 1 = base, >= 2 = the extension block of that version.
    #[serde(default = "default_since")]
    pub since: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// u64 precision flag: maps to bigint on the TS side (default number, 2^53 limit).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub precise: bool,
    /// Reserved placeholder: participates in layout but no accessors are generated (the equivalent of "deletion" during evolution).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reserved: bool,

    // ---- Computed results below are filled in by the layout planner ----
    /// Offset within the fixed region of its version block.
    #[serde(default)]
    pub offset: usize,
    /// Footprint in the fixed region (inline size or an 8-byte slot).
    #[serde(default)]
    pub size: usize,
    /// Bit index in the owning version block's bitmap (optional fields only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional_bit: Option<usize>,
    /// Whether this field is a var-length reference slot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub slot: bool,
    /// Element stride for slot-based arrays (per-element footprint inside the array data region).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_size: Option<usize>,
    /// Offset of the value inside a KV entry (map fields only; consumers must never recompute layout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_value_offset: Option<usize>,
}

fn default_since() -> u16 {
    SINCE_BASE
}

impl FieldDescriptor {
    pub fn new(name: impl Into<String>, ty: Type) -> Self {
        FieldDescriptor {
            name: name.into(),
            ty,
            optional: false,
            since: SINCE_BASE,
            doc: None,
            precise: false,
            reserved: false,
            offset: 0,
            size: 0,
            optional_bit: None,
            slot: false,
            element_size: None,
            map_value_offset: None,
        }
    }

    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    pub fn since(mut self, since: u16) -> Self {
        self.since = since;
        self
    }

    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = Some(doc.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtBlockLayout {
    pub version: u16,
    pub bitmap_len: usize,
    /// Length of the extension block's header area (bitmap + fixed fields, excluding var data).
    pub fixed_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageLayout {
    /// Base bitmap length in bytes (multiple of 4; 0 when there are no optional fields).
    #[serde(default)]
    pub bitmap_len: usize,
    /// Base fixed-region length (= fixed_len in the header).
    #[serde(default)]
    pub fixed_len: usize,
    /// Base max alignment (used as the overall alignment when inlined).
    #[serde(default)]
    pub max_align: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ext_blocks: Vec<ExtBlockLayout>,
}

impl Default for MessageLayout {
    fn default() -> Self {
        MessageLayout { bitmap_len: 0, fixed_len: 0, max_align: 1, ext_blocks: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDescriptor {
    pub key: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub fields: Vec<FieldDescriptor>,
    #[serde(default)]
    pub layout: MessageLayout,
}

impl MessageDescriptor {
    pub fn new(key: u32, name: impl Into<String>, fields: Vec<FieldDescriptor>) -> Self {
        MessageDescriptor {
            key,
            name: name.into(),
            doc: None,
            fields,
            layout: MessageLayout::default(),
        }
    }

    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = Some(doc.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnumVariant {
    pub name: String,
    /// Enums are backed by i32.
    pub value: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnumDescriptor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub variants: Vec<EnumVariant>,
}

/// A set of message and enum definitions (one crate or one export).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaSet {
    pub messages: Vec<MessageDescriptor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<EnumDescriptor>,
}

impl SchemaSet {
    pub fn message(&self, name: &str) -> Option<&MessageDescriptor> {
        self.messages.iter().find(|m| m.name == name)
    }

    /// Base fixed-region length of an already-planned struct.
    pub fn planned_fixed_len(&self, name: &str) -> Result<usize, crate::SchemaError> {
        self.message(name)
            .map(|m| m.layout.fixed_len)
            .ok_or_else(|| crate::SchemaError::UnknownType(name.to_string()))
    }

    /// Base max alignment of an already-planned struct.
    pub fn planned_max_align(&self, name: &str) -> Result<usize, crate::SchemaError> {
        self.message(name)
            .map(|m| m.layout.max_align)
            .ok_or_else(|| crate::SchemaError::UnknownType(name.to_string()))
    }
}

//! Version-aware layout planner.
//!
//! Two phases:
//! 1. check that every referenced type exists, then plan each message's per-version blocks
//!    (bitmap / field offsets / fixed length); the base and the extension blocks are independent
//!    of each other and the base is frozen forever. Structs are slot-shaped in v2, so planning a
//!    message never depends on another message's layout and reference cycles are legal;
//! 2. after everything is planned, back-fill element_size for slot fields (array strides and map
//!    entry strides, writer-current values). This is deferred because strides reference the
//!    planned fixed lengths of element/value structs.

use crate::descriptor::{ExtBlockLayout, MessageDescriptor, SchemaSet, SINCE_BASE};
use crate::errors::SchemaError;
use crate::types::{align_up_usize, element_size, field_size, map_entry_layouts, type_align, Type};
use std::collections::{BTreeMap, HashMap};

/// Plan the whole schema set: fill in every message's layout and run full validation.
pub fn plan_schema(set: &mut SchemaSet) -> Result<(), SchemaError> {
    validate_uniqueness(set)?;
    check_references(set)?;
    let planned = set
        .messages
        .iter()
        .map(|m| {
            let mut msg = m.clone();
            plan_message(&mut msg, set).map(|_| msg)
        })
        .collect::<Result<Vec<MessageDescriptor>, SchemaError>>()?;
    set.messages = planned;
    fill_strides(set)?;
    Ok(())
}

fn validate_uniqueness(set: &SchemaSet) -> Result<(), SchemaError> {
    let mut keys = HashMap::new();
    for m in &set.messages {
        if m.key == 0 {
            return Err(SchemaError::ZeroKey);
        }
        if keys.insert(m.key, m.name.clone()).is_some() {
            return Err(SchemaError::DuplicateKey(m.key));
        }
        for f in &m.fields {
            if f.since < SINCE_BASE {
                return Err(SchemaError::InvalidSince(f.name.clone(), SINCE_BASE));
            }
        }
    }
    let mut names: HashMap<&str, ()> = HashMap::new();
    for m in &set.messages {
        if names.insert(m.name.as_str(), ()).is_some() {
            return Err(SchemaError::DuplicateName(m.name.clone()));
        }
    }
    for e in &set.enums {
        if names.insert(e.name.as_str(), ()).is_some() {
            return Err(SchemaError::DuplicateName(e.name.clone()));
        }
    }
    Ok(())
}

/// Every struct/enum referenced by any field must be defined in this schema set.
fn check_references(set: &SchemaSet) -> Result<(), SchemaError> {
    for m in &set.messages {
        for f in &m.fields {
            check_type_exists(&f.ty, set)?;
        }
    }
    Ok(())
}

fn check_type_exists(ty: &Type, set: &SchemaSet) -> Result<(), SchemaError> {
    match ty {
        Type::Struct { name } | Type::Enum { name } => {
            if set.message(name).is_none() && !set.enums.iter().any(|e| &e.name == name) {
                return Err(SchemaError::UnknownType(name.clone()));
            }
            Ok(())
        }
        Type::Array { body } => check_type_exists(&body.elem, set),
        Type::Map { body } => {
            check_type_exists(&body.key, set)?;
            check_type_exists(&body.value, set)
        }
        _ => Ok(()),
    }
}

fn bitmap_bytes(optional_count: usize) -> usize {
    if optional_count == 0 {
        0
    } else {
        let bytes = (optional_count + 7) / 8;
        (bytes + 3) / 4 * 4
    }
}

fn plan_message(msg: &mut MessageDescriptor, set: &SchemaSet) -> Result<(), SchemaError> {
    let mut blocks: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
    for (i, f) in msg.fields.iter().enumerate() {
        blocks.entry(f.since).or_default().push(i);
    }

    let mut ext = Vec::new();
    for (&version, idxs) in &blocks {
        let optional_count = idxs.iter().filter(|i| msg.fields[**i].optional).count();
        let bitmap_len = bitmap_bytes(optional_count);
        let mut cursor = bitmap_len;
        let mut max_align = if bitmap_len > 0 { 4 } else { 1 };
        let mut bit = 0usize;
        for &i in idxs {
            let f = &mut msg.fields[i];
            if f.optional {
                f.optional_bit = Some(bit);
                bit += 1;
            }
            let size = field_size(&f.ty, set)?;
            let align = type_align(&f.ty, set)?;
            f.offset = align_up_usize(cursor, align);
            f.size = size;
            f.slot = f.ty.is_slot_shaped();
            cursor = f.offset + size;
            max_align = max_align.max(align);
        }
        let fixed_len = align_up_usize(cursor, max_align);
        if version == SINCE_BASE {
            msg.layout.bitmap_len = bitmap_len;
            msg.layout.fixed_len = fixed_len;
            msg.layout.max_align = max_align;
        } else {
            ext.push(ExtBlockLayout { version, bitmap_len, fixed_len });
        }
    }
    msg.layout.ext_blocks = ext;
    Ok(())
}

fn fill_strides(set: &mut SchemaSet) -> Result<(), SchemaError> {
    for mi in 0..set.messages.len() {
        for fi in 0..set.messages[mi].fields.len() {
            let ty = set.messages[mi].fields[fi].ty.clone();
            match &ty {
                Type::Array { body } => {
                    let s = element_size(&body.elem, set)?;
                    set.messages[mi].fields[fi].element_size = Some(s);
                }
                Type::Map { body } => {
                    let layouts = map_entry_layouts(&body.key, &body.value, set)?;
                    // Writer-current values: an evolved struct value is written as an instance slot.
                    let use_inst = crate::types::value_has_ext(&body.value, set);
                    set.messages[mi].fields[fi].element_size = Some(if use_inst {
                        layouts.inst_stride
                    } else {
                        layouts.open_stride
                    });
                    set.messages[mi].fields[fi].map_value_offset = Some(if use_inst {
                        layouts.inst_value_offset
                    } else {
                        layouts.open_value_offset
                    });
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Serialize into stable, human-readable JSON IR (pretty, 2-space indent).
pub fn set_to_json_string(set: &SchemaSet) -> String {
    serde_json::to_string_pretty(set).expect("schema set must serialize")
}

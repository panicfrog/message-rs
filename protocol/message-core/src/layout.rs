//! Version-aware layout planner.
//!
//! Two phases:
//! 1. topologically sort by inline dependencies (struct fields, fixed-array elements), then plan
//!    each message's per-version blocks (bitmap / field offsets / fixed length); the base and the
//!    extension blocks are independent of each other and the base is frozen forever;
//! 2. after everything is planned, back-fill element_size for slot fields (array strides and map
//!    entry strides). This is deferred because dynamic arrays allow recursion
//!    (`Node { children: Vec<Node> }`), so stride references impose no topological constraint.
//!
//! V1 restriction: a struct with extension blocks (since >= 2 fields) cannot be referenced by
//! other types (inline, array, or map). An inlined child only contributes its base part, so nested
//! evolution would silently drop data; reject it explicitly. To evolve, append versioned fields
//! on the containing message.

use crate::descriptor::{ExtBlockLayout, MessageDescriptor, SchemaSet, SINCE_BASE};
use crate::errors::SchemaError;
use crate::types::{align_up_usize, element_size, field_size, map_entry_size, type_align, Type};
use std::collections::{BTreeMap, HashMap};

/// Plan the whole schema set: fill in every message's layout and run full validation.
pub fn plan_schema(set: &mut SchemaSet) -> Result<(), SchemaError> {
    validate_uniqueness(set)?;
    let order = topo_order(set)?;
    for name in order {
        let idx = set
            .messages
            .iter()
            .position(|m| m.name == name)
            .expect("topo order only contains known messages");
        let mut msg = set.messages[idx].clone();
        plan_message(&mut msg, set)?;
        set.messages[idx] = msg;
    }
    fill_strides(set)?;
    check_referenced_structs_frozen(set)?;
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

/// Collect a type's inline dependencies: struct references whose data lands in the parent's
/// fixed region. (Map values and dynamic-array elements live in the var region; they only create
/// stride references, not inline dependencies.)
fn inline_deps_of(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Struct { name } => out.push(name.clone()),
        Type::Array { body } => {
            if body.fixed.is_some() && !body.elem.is_slot_shaped() {
                inline_deps_of(&body.elem, out);
            }
        }
        _ => {}
    }
}

fn topo_order(set: &SchemaSet) -> Result<Vec<String>, SchemaError> {
    let mut state: HashMap<String, u8> = HashMap::new(); // 1 = visiting, 2 = done
    let mut order = Vec::new();
    for m in &set.messages {
        visit(m.name.as_str(), set, &mut state, &mut order)?;
    }
    Ok(order)
}

fn visit(
    name: &str,
    set: &SchemaSet,
    state: &mut HashMap<String, u8>,
    order: &mut Vec<String>,
) -> Result<(), SchemaError> {
    match state.get(name) {
        Some(2) => return Ok(()),
        Some(1) => return Err(SchemaError::InlineCycle(vec![name.to_string()])),
        _ => {}
    }
    state.insert(name.to_string(), 1);
    if let Some(m) = set.message(name) {
        let mut deps = Vec::new();
        for f in &m.fields {
            inline_deps_of(&f.ty, &mut deps);
        }
        for dep in deps {
            if dep == name || state.get(&dep) == Some(&1) {
                return Err(SchemaError::InlineCycle(vec![name.to_string(), dep]));
            }
            if set.message(&dep).is_none() {
                return Err(SchemaError::UnknownType(dep));
            }
            visit(&dep, set, state, order)?;
        }
    }
    state.insert(name.to_string(), 2);
    order.push(name.to_string());
    Ok(())
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
                    let stride = map_entry_size(&body.key, &body.value, set)?;
                    let v_off = map_value_offset(&body.key, &body.value, set)?;
                    set.messages[mi].fields[fi].element_size = Some(stride);
                    set.messages[mi].fields[fi].map_value_offset = Some(v_off);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Offset of the value inside a map entry.
fn map_value_offset(key: &Type, value: &Type, set: &SchemaSet) -> Result<usize, SchemaError> {
    crate::types::validate_map_key(key)?;
    let k_sz = match key {
        Type::Simple(s) => s.fixed_size(),
        Type::Enum { .. } => 4,
        _ => unreachable!(),
    };
    let v_align = type_align(value, set)?;
    Ok(align_up_usize(k_sz, v_align))
}

fn check_referenced_structs_frozen(set: &SchemaSet) -> Result<(), SchemaError> {
    for m in &set.messages {
        for f in &m.fields {
            check_type_frozen(&f.ty, set)?;
        }
    }
    Ok(())
}

fn check_type_frozen(ty: &Type, set: &SchemaSet) -> Result<(), SchemaError> {
    match ty {
        Type::Struct { name } => {
            if let Some(m) = set.message(name) {
                if !m.layout.ext_blocks.is_empty() {
                    return Err(SchemaError::EvolvedStructInline(name.clone()));
                }
            }
            Ok(())
        }
        Type::Array { body } => check_type_frozen(&body.elem, set),
        Type::Map { body } => check_type_frozen(&body.value, set),
        _ => Ok(()),
    }
}

/// Serialize into stable, human-readable JSON IR (pretty, 2-space indent).
pub fn set_to_json_string(set: &SchemaSet) -> String {
    serde_json::to_string_pretty(set).expect("schema set must serialize")
}

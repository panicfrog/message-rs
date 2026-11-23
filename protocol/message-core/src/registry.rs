//! Link-time registry collection: inventory iteration into a SchemaSet.
//!
//! The derive macro cannot tell struct field types from enums (both are bare named paths), so
//! collection relabels them using the registered enum names; plan_schema can then rebuild the
//! layout and cross-check it against the derive-side symbolic constant layout.

use crate::descriptor::SchemaSet;
use crate::{plan_schema, SchemaEntry, SchemaError, Type};
use std::collections::BTreeSet;

/// Collect every descriptor linked into this binary (keeping the derive-computed layout).
pub fn collect_raw() -> SchemaSet {
    let mut set = SchemaSet::default();
    let mut enums = BTreeSet::new();
    for entry in inventory::iter::<SchemaEntry> {
        match entry {
            SchemaEntry::Message(f) => set.messages.push(f().clone()),
            SchemaEntry::Enum(f) => {
                let d = f();
                enums.insert(d.name.clone());
                set.enums.push(d.clone());
            }
        }
    }
    set.messages.sort_by(|a, b| a.name.cmp(&b.name));
    set.enums.sort_by(|a, b| a.name.cmp(&b.name));
    for m in &mut set.messages {
        for f in &mut m.fields {
            relabel(&mut f.ty, &enums);
        }
    }
    set
}

fn relabel(ty: &mut Type, enums: &BTreeSet<String>) {
    match ty {
        Type::Struct { name } if enums.contains(name) => {
            let n = name.clone();
            *ty = Type::Enum { name: n };
        }
        Type::Array { body } => relabel(&mut body.elem, enums),
        Type::Map { body } => {
            relabel(&mut body.key, enums);
            relabel(&mut body.value, enums);
        }
        _ => {}
    }
}

/// Collect and rebuild layout via the planner (for export/validation).
pub fn collect_schema() -> Result<SchemaSet, SchemaError> {
    let mut set = collect_raw();
    plan_schema(&mut set)?;
    Ok(set)
}

/// Cross-check: the derive-generated symbolic constant layout must equal the planner's pure
/// functional layout. The two implementations must agree field by field, otherwise the layout
/// algorithm has drifted.
pub fn check_consistency() -> Result<(), String> {
    let raw = collect_raw();
    let mut replanned = raw.clone();
    plan_schema(&mut replanned).map_err(|e| format!("replan failed: {e}"))?;
    if raw.messages.len() != replanned.messages.len() {
        return Err("message count mismatch".to_string());
    }
    for (a, b) in raw.messages.iter().zip(replanned.messages.iter()) {
        if a.name != b.name || a.key != b.key || a.fields.len() != b.fields.len() {
            return Err(format!("{}: message-level mismatch", a.name));
        }
        for (fa, fb) in a.fields.iter().zip(b.fields.iter()) {
            if fa.offset != fb.offset
                || fa.size != fb.size
                || fa.slot != fb.slot
                || fa.element_size != fb.element_size
                || fa.map_value_offset != fb.map_value_offset
                || fa.optional_bit != fb.optional_bit
                || fa.since != fb.since
            {
                return Err(format!(
                    "{}.{}: derive(offset={}, size={}, slot={}, es={:?}, bit={:?}) != planner(offset={}, size={}, slot={}, es={:?}, bit={:?})",
                    a.name, fa.name, fa.offset, fa.size, fa.slot, fa.element_size, fa.optional_bit,
                    fb.offset, fb.size, fb.slot, fb.element_size, fb.optional_bit
                ));
            }
        }
        if a.layout != b.layout {
            return Err(format!(
                "{}: layout mismatch derive={:?} planner={:?}",
                a.name, a.layout, b.layout
            ));
        }
    }
    Ok(())
}

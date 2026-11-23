//! message-gen: export pipeline and evolution gate.
//!
//! Usage: inside a crate that defines messages,
//!
//! ```ignore
//! // src/bin/export-schema.rs
//! message_gen::export_main!();
//! ```
//!
//! then `cargo run --bin export-schema [command]`:
//! - `export [path]`: collect the link-time registry into a JSON IR snapshot (default
//!   `./message-snapshot.json`);
//! - `check [path]`: compare against the committed snapshot and enforce the append-only
//!   evolution rules, exiting non-zero on violations (CI gate);
//! - `ts <out.ts>`: generate the TypeScript schema (readers, encoders, validators with literal
//!   offsets; the runtime needs no IR);
//! - `ir --key <KEY>`: dump the JSON IR of a single type for debugging.

use message_core::{collect_schema, set_to_json_string, SchemaSet};
use std::path::Path;

mod backend_ts;
pub use backend_ts::generate_ts;

pub const DEFAULT_SNAPSHOT: &str = "message-snapshot.json";

/// Generates the export binary's main. Call it from `src/bin/*.rs` in the user crate.
#[macro_export]
macro_rules! export_main {
    () => {
        fn main() {
            let args: Vec<String> = std::env::args().collect();
            let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("export");
            let rest: Vec<String> = args.iter().skip(2).cloned().collect();
            std::process::exit($crate::run(cmd, &rest));
        }
    };
}

pub fn run(cmd: &str, args: &[String]) -> i32 {
    match cmd {
        "export" => {
            let path = args.first().map(|s| s.as_str()).unwrap_or(DEFAULT_SNAPSHOT);
            match export_to(path) {
                Ok(n) => {
                    println!("exported {n} message types -> {path}");
                    0
                }
                Err(e) => {
                    eprintln!("export failed: {e}");
                    1
                }
            }
        }
        "ts" => {
            let path = args
                .iter()
                .find(|a| !a.starts_with("--"))
                .map(|s| s.as_str())
                .unwrap_or("schema.ts");
            match ts_to(path) {
                Ok(n) => {
                    println!("generated TS code ({n} message types) -> {path}");
                    0
                }
                Err(e) => {
                    eprintln!("TS generation failed: {e}");
                    1
                }
            }
        }
        "check" => {
            let path = args.first().map(|s| s.as_str()).unwrap_or(DEFAULT_SNAPSHOT);
            match check_against(path) {
                Ok(()) => {
                    println!("evolution check passed: compatible with {path}");
                    0
                }
                Err(errs) => {
                    eprintln!("evolution check failed ({} violations):", errs.len());
                    for e in errs {
                        eprintln!("  - {e}");
                    }
                    1
                }
            }
        }
        "ir" => {
            let key = args
                .iter()
                .position(|a| a == "--key")
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse::<u32>().ok());
            match (key, ir_dump(key)) {
                (Some(_), Ok(Some(json))) => {
                    println!("{json}");
                    0
                }
                (Some(k), Ok(None)) => {
                    eprintln!("no message type with key={k}");
                    2
                }
                (_, Err(e)) => {
                    eprintln!("collection failed: {e}");
                    1
                }
                (None, _) => {
                    eprintln!("usage: ir --key <KEY>");
                    2
                }
            }
        }
        other => {
            eprintln!("unknown command `{other}` (supported: export / check / ir --key <KEY> / ts <out.ts>)");
            2
        }
    }
}

pub fn export_to(path: &str) -> Result<usize, String> {
    let set = collect_schema().map_err(|e| e.to_string())?;
    let n = set.messages.len();
    std::fs::write(Path::new(path), set_to_json_string(&set)).map_err(|e| e.to_string())?;
    Ok(n)
}

/// Generate the TypeScript schema (readers, encoders and validators with literal offsets).
pub fn ts_to(path: &str) -> Result<usize, String> {
    let set = collect_schema().map_err(|e| e.to_string())?;
    let n = set.messages.len();
    std::fs::write(Path::new(path), generate_ts(&set)).map_err(|e| e.to_string())?;
    Ok(n)
}

pub fn check_against(path: &str) -> Result<(), Vec<String>> {
    let old: SchemaSet = serde_json::from_str(
        &std::fs::read_to_string(Path::new(path)).map_err(|e| vec![format!("failed to read snapshot: {e}")])?,
    )
    .map_err(|e| vec![format!("failed to parse snapshot: {e}")])?;
    let new = collect_schema().map_err(|e| vec![e.to_string()])?;
    let errs = check_evolution(&old, &new);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

pub fn ir_dump(key: Option<u32>) -> Result<Option<String>, String> {
    let set = collect_schema().map_err(|e| e.to_string())?;
    match key {
        None => Ok(Some(set_to_json_string(&set))),
        Some(k) => Ok(set
            .messages
            .iter()
            .find(|m| m.key == k)
            .map(|m| serde_json::to_string_pretty(m).expect("descriptor serializes"))),
    }
}

/// Append-only evolution rules. The base and existing extension blocks of a known message must
/// stay identical field by field; new fields must declare a strictly larger since; deletion and
/// reordering are rejected. Marking an existing field `reserved` is the sanctioned way to delete
/// it and passes; un-reserving one or flipping `precise` (number <-> bigint on the TS side) does not.
pub fn check_evolution(old: &SchemaSet, new: &SchemaSet) -> Vec<String> {
    let mut errs = Vec::new();
    for om in &old.messages {
        let Some(nm) = new.messages.iter().find(|m| m.name == om.name) else {
            errs.push(format!("message `{}` was removed (published messages cannot be deleted; keep it registered, or reserve its fields)", om.name));
            continue;
        };
        if nm.key != om.key {
            errs.push(format!("`{}`: key changed ({} -> {})", om.name, om.key, nm.key));
        }
        if nm.layout.bitmap_len != om.layout.bitmap_len
            || nm.layout.fixed_len != om.layout.fixed_len
            || nm.layout.max_align != om.layout.max_align
        {
            errs.push(format!(
                "`{}`: base layout changed (bitmap {}/{}, fixed {}/{}, align {}/{})",
                om.name,
                om.layout.bitmap_len,
                nm.layout.bitmap_len,
                om.layout.fixed_len,
                nm.layout.fixed_len,
                om.layout.max_align,
                nm.layout.max_align
            ));
        }
        if nm.layout.ext_blocks.len() < om.layout.ext_blocks.len() {
            errs.push(format!(
                "`{}`: extension block count shrank ({} -> {})",
                om.name,
                om.layout.ext_blocks.len(),
                nm.layout.ext_blocks.len()
            ));
        }
        for (i, ob) in om.layout.ext_blocks.iter().enumerate() {
            if nm.layout.ext_blocks.get(i) != Some(ob) {
                errs.push(format!(
                    "`{}`: extension block #{} changed (old {ob:?} vs new {:?}); existing blocks are immutable, only larger versions may be appended",
                    om.name,
                    i,
                    nm.layout.ext_blocks.get(i)
                ));
            }
        }
        if nm.fields.len() < om.fields.len() {
            errs.push(format!(
                "`{}`: field count shrank ({} -> {})",
                om.name,
                om.fields.len(),
                nm.fields.len()
            ));
            continue;
        }
        for (i, of) in om.fields.iter().enumerate() {
            let nf = &nm.fields[i];
            if nf.name != of.name {
                errs.push(format!(
                    "`{}`: field #{} changed from `{}` to `{}` (reordering/insertion is not allowed)",
                    om.name, i, of.name, nf.name
                ));
            } else if nf.ty != of.ty
                || nf.optional != of.optional
                || nf.since != of.since
                || nf.offset != of.offset
                || nf.size != of.size
                || nf.slot != of.slot
                || nf.optional_bit != of.optional_bit
                || nf.precise != of.precise
            {
                errs.push(format!(
                    "`{}`.`{}`: field definition changed (type/optional/since/precise/offset/slot mismatch)",
                    om.name, of.name
                ));
            } else if of.reserved && !nf.reserved {
                errs.push(format!(
                    "`{}`.`{}`: reserved field was un-reserved (a reserved field is deleted from the API; add a new field with a larger `since` instead)",
                    om.name, of.name
                ));
            }
        }
        let old_max = om.fields.iter().map(|f| f.since).max().unwrap_or(1);
        for nf in nm.fields.iter().skip(om.fields.len()) {
            if nf.since <= old_max {
                errs.push(format!(
                    "`{}`.`{}`: appended field has since={} which must exceed the existing max version {}",
                    om.name, nf.name, nf.since, old_max
                ));
            }
        }
    }
    for oe in &old.enums {
        match new.enums.iter().find(|e| e.name == oe.name) {
            Some(ne) if ne.variants != oe.variants => {
                errs.push(format!("enum `{}` variants changed (values cannot be added/removed/changed)", oe.name));
            }
            None => errs.push(format!("enum `{}` was removed", oe.name)),
            _ => {}
        }
    }
    errs
}

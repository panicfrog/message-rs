//! `hub-ffi`: the Swift (C++ interop) surface of `message-hub`. See `bridge.rs` for the
//! conventions the bridge follows and hub/DESIGN.md for the design.

pub mod bridge;

/// Force-link anchor. An aggregate library (a React Native app's superlib) that depends on this
/// crate must reference this symbol, or the linker drops every object here: nothing in Rust
/// calls the bridge functions, they are only reached from Swift/C++. See example/PLAN.md.
#[unsafe(no_mangle)]
pub extern "C" fn message_hub_ffi_force_link() {}

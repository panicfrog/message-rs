//! message-derive: `#[derive(Message)]` for structs and `#[derive(MessageEnum)]` for enums.
//!
//! User crates must depend on `message-core` (generated code references `::message_core::`).
//!
//! ```ignore
//! #[derive(Message)]
//! #[msg(key = 0x0001)]
//! pub struct User {
//!     pub id: u64,
//!     pub name: String,
//!     pub profile: Profile,               // inline nesting (base part)
//!     #[msg(since = 2)]
//!     pub tags: Vec<String>,              // v2 extension block
//!     #[msg(since = 2)]
//!     pub avatar: Option<Vec<u8>>,        // v2 extension block with its own bitmap
//! }
//! ```

mod gen;
mod model;

#[proc_macro_derive(Message, attributes(msg))]
pub fn derive_message(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let derive_input = syn::parse_macro_input!(input as syn::DeriveInput);
    match gen::expand_struct(&derive_input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[proc_macro_derive(MessageEnum, attributes(msg))]
pub fn derive_message_enum(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let derive_input = syn::parse_macro_input!(input as syn::DeriveInput);
    match gen::expand_enum(&derive_input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

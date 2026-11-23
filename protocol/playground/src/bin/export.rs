message_gen::export_main!();

// Force-link the playground crate so inventory registrations take effect
#[allow(unused_imports)]
use playground as _link;

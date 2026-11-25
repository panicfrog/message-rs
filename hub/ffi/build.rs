fn main() {
    cxx_build::bridge("src/bridge.rs")
        .std("c++17")
        .compile("hub-ffi");
    println!("cargo:rerun-if-changed=src/bridge.rs");
}

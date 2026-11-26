//! Drives the Java conformance suite against the host dylib. Requires `javac`/`java` on PATH
//! (any JDK 17+); skipped gracefully when they are missing.

use std::path::PathBuf;
use std::process::Command;

fn tool(name: &str) -> Option<PathBuf> {
    let out = Command::new("which").arg(name).output().ok()?;
    if out.status.success() && !out.stdout.is_empty() {
        Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
    } else {
        None
    }
}

#[test]
fn java_conformance() {
    let (Some(java), Some(javac)) = (tool("java"), tool("javac")) else {
        eprintln!("skipping: java/javac not found");
        return;
    };
    let manifest = env!("CARGO_MANIFEST_DIR");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jni-java");
    std::fs::create_dir_all(&out).unwrap();
    // Build the dylib first: cdylib artifacts live next to the lib output.
    let dylib = build_dylib();
    std::fs::copy(&dylib, out.join(dylib.file_name().unwrap())).unwrap();

    let sources = ["tests/java/message/rs/hub/Hub.java", "tests/java/Conformance.java"];
    let status = Command::new(&javac)
        .current_dir(manifest)
        .args(&sources)
        .arg("-d")
        .arg(&out)
        .status()
        .expect("javac");
    assert!(status.success(), "javac failed");

    let run = Command::new(&java)
        .arg("-cp")
        .arg(&out)
        .arg("-Djava.library.path=".to_string() + out.to_str().unwrap())
        .arg("--enable-native-access=ALL-UNNAMED")
        .arg("Conformance")
        .output()
        .expect("java");
    let stdout = String::from_utf8_lossy(&run.stdout);
    print!("{stdout}");
    assert!(run.status.success(), "java conformance failed");
    assert!(stdout.contains("ALL OK"), "unexpected output: {stdout}");
}

fn build_dylib() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    // The test profile does not produce cdylib artifacts; build the dev profile explicitly.
    let status = Command::new("cargo")
        .current_dir(manifest)
        .args(["build", "-p", "hub-jni"])
        .status()
        .expect("cargo build");
    assert!(status.success(), "cargo build failed");
    // CARGO_TARGET_DIR may point elsewhere; resolve relative to the workspace.
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(manifest).join("../../target"));
    let name = if cfg!(target_os = "macos") {
        "libhub_jni.dylib"
    } else if cfg!(target_os = "linux") {
        "libhub_jni.so"
    } else {
        "hub_jni.dll"
    };
    target.join("debug").join(name)
}

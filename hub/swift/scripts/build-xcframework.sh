#!/bin/sh
# Builds the hub FFI static library for every Apple slice and wraps them in one xcframework.
# macOS is included so `swift test` can run the package's tests on a development machine.
# The old xcframework is removed first: SwiftPM caches a binaryTarget by path and would
# otherwise keep linking the stale library.
set -eu
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT="$ROOT/hub/swift/Artifacts/MessageHubRust.xcframework"
INC="$ROOT/hub/swift/Sources/MessageHubCxx/include"

cd "$ROOT"
"$ROOT/hub/swift/export-headers.sh"
for t in aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin; do
  cargo build --release -p hub-ffi --target "$t"
done

rm -rf "$OUT"
xcodebuild -create-xcframework \
  -library "target/aarch64-apple-ios/release/libhub_ffi.a" -headers "$INC" \
  -library "target/aarch64-apple-ios-sim/release/libhub_ffi.a" -headers "$INC" \
  -library "target/aarch64-apple-darwin/release/libhub_ffi.a" -headers "$INC" \
  -output "$OUT"
echo "xcframework written to $OUT"

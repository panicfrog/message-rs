#!/bin/sh
# Builds the hub FFI static library for every Apple slice and wraps them in one xcframework.
# macOS is included so `swift test` can run the package's tests on a development machine.
# The old xcframework is removed first: SwiftPM caches a binaryTarget by path and would
# otherwise keep linking the stale library.
set -eu
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT="$ROOT/hub/apple/Artifacts/MessageHubRust.xcframework"
INC="$ROOT/hub/apple/Sources/MessageHubCxx/include"

cd "$ROOT"
"$ROOT/hub/apple/export-headers.sh"
# cargo's target dir may be redirected (CARGO_TARGET_DIR on CI runners)
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
for t in aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin; do
  cargo build --release -p hub-ffi --target "$t"
done

rm -rf "$OUT"
xcodebuild -create-xcframework \
  -library "$TARGET_DIR/aarch64-apple-ios/release/libhub_ffi.a" -headers "$INC" \
  -library "$TARGET_DIR/aarch64-apple-ios-sim/release/libhub_ffi.a" -headers "$INC" \
  -library "$TARGET_DIR/aarch64-apple-darwin/release/libhub_ffi.a" -headers "$INC" \
  -output "$OUT"
echo "xcframework written to $OUT"

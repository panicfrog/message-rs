#!/bin/sh
# Copies the cxx-generated bridge header and the cxx runtime header into the Swift package's
# Clang target. Run after changing hub/ffi/src/bridge.rs; CI regenerates and diffs, like every
# other generated artifact in this repository.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INC="$ROOT/hub/swift/Sources/MessageHubCxx/include"
cd "$ROOT"
cargo build -p hub-ffi >/dev/null
OUT=$(ls -d target/debug/build/hub-ffi-*/out/cxxbridge/include | head -1)
mkdir -p "$INC/rust"
cp "$OUT/hub-ffi/src/bridge.rs.h" "$INC/message_hub.h"
cp "$OUT/rust/cxx.h" "$INC/rust/cxx.h"
echo "headers exported to $INC"

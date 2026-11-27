#!/bin/sh
# Copies the cxx-generated bridge header and the cxx runtime header into the Swift package's
# Clang target. Run after changing hub/ffi/src/bridge.rs; CI regenerates and diffs, like every
# other generated artifact in this repository.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INC="$ROOT/hub/apple/Sources/MessageHubCxx/include"
cd "$ROOT"

# Ask cargo where it put things instead of guessing target/: CARGO_TARGET_DIR may point
# elsewhere (CI runners do this), and the build-script hash directory is not predictable.
cargo build -p hub-ffi --message-format=json-render-diagnostics >"$ROOT/.hub-ffi-build.json"
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
OUT=$(python3 - "$ROOT/.hub-ffi-build.json" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "build-script-executed" and m.get("package_id", "").find("hub-ffi") >= 0:
        print(m["out_dir"]); break
PY
)
rm -f "$ROOT/.hub-ffi-build.json"
if [ -z "$OUT" ]; then
  echo "could not locate the hub-ffi build script output under $TARGET_DIR" >&2
  exit 1
fi

mkdir -p "$INC/rust"
cp "$OUT/cxxbridge/include/hub-ffi/src/bridge.rs.h" "$INC/message_hub.h"
cp "$OUT/cxxbridge/include/rust/cxx.h" "$INC/rust/cxx.h"
echo "headers exported to $INC"

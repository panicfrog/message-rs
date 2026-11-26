# example/: the message-rs demo app — implementation plan

**Both platforms pass (2026-11-26)** — `docs/ios-all-pass.png`, `docs/android-all-pass.png`.
Five scenarios run on launch: JS→JS, JS→native, native→JS, shared pull (both ends read one
store), Closed across languages (exactly once, pull empty afterwards). `native` is Swift on iOS
and Kotlin on Android. One Rust registry per process, verified at runtime on both.
See README.md for how to run it.

Scope (settled with the user): a React Native app inside this repository that verifies **all
three surfaces share one Rust registry**. Steps 0–3 are done on both platforms; what remains is
step 4 (automating the run) and step 5 (whether any of it belongs in CI).

## Why our own demo (not jsi-rs's example)

Borrowing another repo's example leaves no artifact here, cannot run in our CI, and breaks
when they change it. The cross-language claim ("one core per process") is the data layer's
central promise and the only thing host-side tests cannot prove — it needs a real app.

## The blocking problem: one Rust copy per process

Today each surface ships its own copy of the core:

| Surface | Rust comes from |
|---|---|
| hub-turbo (JS) | the app's superlib `librust_app.a` / `librust_app.so` |
| hub-swift | `MessageHubRust.xcframework` (its own `libhub_ffi.a`) |
| hub-kotlin | `libhub_jni.so` |

Linking two of them into one app gives **two `Hub::shared()` instances** (each static copy has
its own `OnceLock`), so cross-language delivery silently fails. This is the link-mode question
deferred in hub/DESIGN.md M2/M5 — the demo forces it to be answered first.

### Confirmed by symbol inspection (2026-11-26)

Each artifact carries its own `message_hub::Hub::shared::SHARED` static, and the crate hashes
differ, so the mangled names differ and the linker will **not** coalesce them:

```
libhub_ffi.a   __RNvNvMs_Csb7H7fYwxfei_11message_hubNtB6_3Hub6shared6SHARED
libhub_jni     __RNvNvMs_Cs518BnrOXvhc_11message_hubNtB6_3Hub6shared6SHARED
librust_app.a  __RNvNvMs_CsdMojIPzNB87_11message_hubNtB6_3Hub6shared6SHARED
```

Three independent `OnceLock`s = three registries. This is not a "duplicate symbol might
clash" risk — sharing is *guaranteed impossible* while each surface builds its own copy.
(Each build also gets a different hash because the dependency graphs differ.)

### Resolution to implement (step 0, before any app code)

One Rust build per app, every surface links against it:

- **iOS**: the superlib carries `message-hub` + `hub-ffi` + `hub-turbo` (add `hub-ffi` as a
  dependency of the demo's aggregate crate, or make hub-turbo depend on it). The Swift package
  gains a **superlib mode**: `MessageHubCxx` headers as today, but **no** `MessageHubRust`
  binaryTarget — instead `linkerSettings: [.linkedLibrary(...)]` / relying on the app already
  linking the superlib. Two products (`MessageHub` standalone, `MessageHubShared` for apps)
  or a compile-time flag; decide by what SwiftPM allows cleanly.
- **Android**: `hub-jni` and `hub-turbo` go into **one** `.so`. Either the superlib depends on
  `hub-jni` (so its JNI symbols land in `librust_app.so`) or a small aggregate cdylib depends
  on both. The Kotlin wrapper then loads that single library name.
- Verify by symbol inspection first (`nm`: exactly one `Hub::shared()` / one `OnceLock`), then
  by the runtime test below.

**Chosen: plan A** (lower cognitive load — installing one package is enough, no hand-written
aggregate crate). Implemented as platform-conditional dependencies in `hub/turbo/Cargo.toml`:
iOS/macOS pull in `hub-ffi`, Android pulls in `hub-jni`.

Verified with a throwaway aggregate crate in this repo's scope (core + hub-ffi, staticlib):
the artifact contains **exactly one** `Hub::shared::SHARED` static and 171 Swift-bridge
symbols — one build carries both the core and the native surface.

Both surfaces now have their superlib mode, verified locally:

- **Swift**: `MESSAGE_HUB_SUPERLIB=1 swift build` compiles the wrapper with **headers only**
  (no binaryTarget) — the symbols resolve at the app's final link. Default mode unchanged
  (10 tests pass with the xcframework).
- **Kotlin**: `-Dhub.libName=rust_app` loads the aggregate superlib instead of `hub_jni`;
  default unchanged (9 tests pass).

Two link-level traps found and fixed while building the demo's superlib (both would have
shown up as "Swift finds no symbols" at app link time):

1. **Dead-code elimination**: nothing in Rust calls the bridge/JNI entry points (Swift and the
   JVM do), so the linker dropped those objects. Each surface now exports a force-link anchor
   (`message_hub_ffi_force_link` / `message_hub_jni_force_link`) that `hub-turbo` references via
   a `#[used]` static — the same trick jsi-turbo's macros use for module anchors.
2. **cxx glue lives in the aggregate's build**: the Rust symbols survived but the
   `message_hub::` C++ entry points did not. `hub-ffi` needs `links = "hub-ffi"` in
   `[package]` so cxx-build inside the superlib discovers the bridge and compiles its C++ glue
   there (jsi-sys does the same with `links = "jsi"`).

Verified on the demo app's iOS-simulator superlib: exactly one `Hub::shared::SHARED`, 21
`message_hub` C++ symbols (shared_ptr/subscribe/publish/get all present), and the Hub turbo
force anchor defined.

**The iOS app builds** (`BUILD SUCCEEDED`, simulator): the Swift surface compiles in superlib
mode — headers only, no xcframework — and links against the superlib's symbols. Three
configuration findings on the way:

- The pod must use **C++20**, not C++17: RN 0.88's headers use concepts (`requires`), and a
  c++17 setting on our pod broke the React Clang module for the whole target.
- CocoaPods ignores `source_files` pointing outside the pod directory; the wrapper sources are
  **symlinked** into `ios/HubNative/wrapper/` so there is still one source of truth.
- The wrapper does `import MessageHubCxx`, so the pod ships a `module.modulemap` giving that
  name to the cxxbridge header, with `SWIFT_INCLUDE_PATHS` at the pod root.

Remaining for step 0: prove it at runtime (Swift publishes, JS pulls the same bytes).

This step is the real work; the app around it is scaffolding.

## Runtime findings (iOS)

- **Metro cannot resolve a `file:` symlinked package by default**: `metro.config.js` needs
  `watchFolders: [repoRoot]` and both `nodeModulesPaths`, or it reports "Unable to resolve
  module hub-turbo" although npm linked it.
- The package's `main` must point at `index.ts` (the emitter layer), not the generated
  `Hub.ts` — otherwise consumers get only the raw native interface.
- RN 0.88 ships a **prebuilt** React framework with the Metro port compiled in, so
  `RCT_METRO_PORT` cannot be changed by rebuilding. To use another port (another project's
  Metro may own 8081), set the runtime override:
  `simctl spawn <dev> defaults write <bundle-id> RCT_jsLocation -string 'localhost:8088'`.
- Hermes has no `TextEncoder`; the scenarios use ASCII byte literals.
- Reading results: `console.log` did not surface in Metro's terminal or the simulator log on
  this setup — the UI (and a screenshot) is the reliable channel, which is why the app renders
  every scenario's verdict.

## Android findings

- The framework links `librust_app.a` **statically into `librustturbo.so`**, so the Kotlin
  wrapper must load `rustturbo` — not `hub_jni`, not `rust_app`. `MainApplication.onCreate`
  sets `System.setProperty("hub.libName", "rustturbo")` before anything touches `Hub`.
- The RN template sets `reactNativeArchitectures` to four ABIs; building all of them needs four
  Rust targets installed. For the demo, `-PreactNativeArchitectures=arm64-v8a` is enough.
- Verified in `librustturbo.so` (arm64, unstripped): exactly one `Hub::shared::SHARED`, 28
  `Java_message_rs_hub_*` symbols and the Hub turbo anchor — one core, both surfaces.
- Runtime: `HUB_DEMO_RESULT 5/5`. Like iOS, the app defaults to port 8081 (another project's
  Metro here); point it at ours by writing `debug_http_host` into the app's SharedPreferences
  (`adb shell run-as com.messagersdemo ...`) plus `adb reverse tcp:8088 tcp:8088`. Unlike iOS,
  Android's JS console does reach `adb logcat -s ReactNativeJS`, which makes automation easy.

## App shape (after step 0)

```
example/
  package.json         RN 0.88 + react-native-rust-turbo + file:../hub/turbo
  App.tsx              the conformance screen: buttons per scenario, results inline
  ios/                 Podfile; a Swift native module (HubBridge) exposing the Swift surface to JS
  android/             gradle; a Kotlin native module doing the same
  src/scenarios.ts     the cross-language checks, shared by the UI and any automation
```

The native modules are thin: each exposes `publishFromNative(id, kind, bytes)` and
`subscribeFromNative(id)` (forwarding events to JS via RN's event emitter), so a JS test can
drive "native publishes → JS receives" and "JS publishes → native receives" without UI taps.

## Scenarios the demo must prove

1. JS → JS: publish/pull/Closed through hub-turbo (already covered by unit tests, re-checked here).
2. **JS → Swift**: JS publishes id 101; the Swift module's subscription fires; it reports back to JS.
3. **Swift → JS**: the Swift module publishes id 102; the JS channel receives it.
4. **JS → Kotlin** and **Kotlin → JS**: the same pair on Android.
5. **Pull across languages**: Swift publishes, JS `pull` reads the same bytes (and vice versa).
6. **Closed across languages**: the last publisher closes on one side, subscribers on the other
   side get Closed exactly once.
7. **Schema decode**: a `MESSAGE` payload published natively decodes in JS with `decodeBy(key)`
   against the committed vector (byte equality first, then field values).

Each scenario asserts on bytes, not just "an event arrived".

## Steps

0. **One-copy linking** (above). Gate: `nm` shows a single core; a two-line smoke test (Swift
   publishes, JS pulls) passes on a simulator.
1. Scaffold the RN app (`npx @react-native-community/cli init` pinned to 0.88, then strip the
   template UI), wire `file:../hub/turbo`, confirm the superlib builds with our module.
2. iOS: add the Swift native module using `MessageHub` in superlib mode; run scenarios 2/3/5/6/7.
3. Android: the Kotlin module against the single `.so`; the same scenarios.
4. Automate: an in-app test runner (a screen that runs all scenarios and prints PASS/FAIL) so
   the check is repeatable without manual taps; optionally drive it from a Detox/Maestro script.
5. CI: keep the app build out of CI initially (pods + NDK + simulator are heavy); add a
   `example` job later if the runtime proves stable. Document how to run it locally.

## Open points

- Whether `hub-ffi` should become a dependency of `hub-turbo` (simplest single-build story) or
  the demo's aggregate crate lists both — affects the published packages, not just the demo.
- SwiftPM's exact mechanism for "headers only, symbols from the app" (product split vs flag).
- RN version pinning: match jsi-rs's supported 0.88 to avoid framework drift.

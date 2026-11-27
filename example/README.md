# message-rs demo app

A React Native app that proves the data hub's central claim: **JS, Swift and Kotlin share one
Rust registry per process**. Six scenarios run on launch and render their verdicts; screenshots
of both platforms passing are in `docs/`.

| Scenario | What it proves |
|---|---|
| JS → JS | the baseline push/pull path in a real runtime |
| JS → native | JS publishes, the native subscription receives the same bytes |
| native → JS | the native side publishes, the JS channel receives |
| shared pull | both ends read one store (identical bytes) |
| Closed across | the last native publisher closes; JS sees Closed exactly once, pull goes empty |
| struct across | native publishes a real generated struct (`Profile`) through the unified protocol — no manual encode step — and JS decodes it with its own `Profile.codec`, fields matching |

`native` is Swift on iOS (`ios/HubNative`) and Kotlin on Android
(`android/app/src/main/java/com/messagersdemo/hub`). Both link the **same** core the Rust turbo
module uses — see `PLAN.md` for why that takes work (one copy per process).

## Run it

```sh
npm install
npx react-native start --port 8088          # 8081 may be taken by another project

# iOS
cd ios && pod install && cd ..
xcodebuild -workspace ios/MessageRsDemo.xcworkspace -scheme MessageRsDemo \
  -configuration Debug -sdk iphonesimulator -destination 'id=<simulator-udid>' \
  -derivedDataPath ios/build build
xcrun simctl install <udid> ios/build/Build/Products/Debug-iphonesimulator/MessageRsDemo.app
# RN 0.88 bakes the Metro port into its prebuilt framework; override it at runtime:
xcrun simctl spawn <udid> defaults write org.reactjs.native.example.MessageRsDemo \
  RCT_jsLocation -string 'localhost:8088'
xcrun simctl launch <udid> org.reactjs.native.example.MessageRsDemo

# Android (arm64 only unless more Rust targets are installed)
cd android && ./gradlew :app:assembleDebug -PreactNativeArchitectures=arm64-v8a && cd ..
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb reverse tcp:8088 tcp:8088
adb shell run-as com.messagersdemo mkdir -p /data/data/com.messagersdemo/shared_prefs
# point the app at our Metro (debug_http_host), then launch
adb shell am start -n com.messagersdemo/.MainActivity
adb logcat -s ReactNativeJS:V | grep HUB_DEMO_RESULT
```

The app also prints one machine-readable line: `HUB_DEMO_RESULT <passed>/<total> [...]`.

## Notes

- `hub-turbo` is a `file:` symlink into `../hub/turbo`; `metro.config.js` watches the repo root
  so Metro can resolve it. `@message/runtime` (the generated `Profile.codec` used by "struct
  across") is a `file:` dependency the same way.
- The wrapper sources are symlinked from `hub/apple` and `hub/kotlin` — one source of truth.
  Since both surfaces now depend on their runtime's codec protocol (`MessageCodec` /
  `MessageEncodable`), `runtime/swift`'s `Runtime.swift` + `Schema.swift` and `runtime/kotlin`'s
  `Runtime.kt` + `Schema.kt` are symlinked in too, compiled directly into the pod/app target
  (no separate module — `#if canImport(MessageRuntime)` guards the import for this flat build).
- Neither surface ships its own Rust library here: the symbols come from the app's superlib
  (`librust_app.a` on iOS, linked into `librustturbo.so` on Android).

# Kotlin surface (M4) — plan

Status: hub-jni (Rust) builds for Android; this Kotlin wrapper is the next step.

## JNI binding contract

All `external fun` declarations live in `companion object` blocks of their classes, so the
JVM symbol names are stable (`Java_message_rs_hub_Hub_00024Companion_...`). The Rust side
(hub/jni/src/lib.rs) exports exactly those. The dispatch of instance-level convenience
methods goes through the companion functions with the handle passed explicitly.

- `Hub.shared` / `Hub.create()` / `release(handle)` — the process hub is never freed.
- `createPublisher(handle, id, kind)`, `publishNative(handle, bytes)`, `closeNative(handle)`,
  `releasePublisher(handle)`.
- `subscribeNative(handle, id, callback)` (inline) and
  `subscribeOnNative(handle, id, callback, scheduler)` — the callback object declares
  `onEvent(id: Long, kind: Int, ptr: Long, len: Int)`; the scheduler object declares
  `schedule(jobCtx: Long)` and must call `Hub.runJob(jobCtx)` (a companion external) exactly
  once, from any thread; `ptr == 0` delivers Closed.
- `getNative(handle, id): LongArray?` = `[kind, ptr, end, len]`; `copyBytes(ptr, len)` reads
  the payload into a direct `ByteBuffer` before the next publish replaces it.

## Wrapper shape (to write)

`Hub` (shared/isolated, AutoCloseable), `Publisher`, `Subscription` (RAII + finalizer safety
net, explicit close from `ViewModel.onCleared`), `ValueKind`/`Value` mirroring the core codec,
`events(id, dispatcher): Flow<HubEvent>` over a `MutableSharedFlow` fed by a scheduled
subscription, and `HubState` (StateFlow mirror + pull-first `refresh()`).

## Verification

1. Host JVM test: build hub-jni as a macOS dylib (`crate-type = ["cdylib", "rlib"]` already
   supports it), `System.load` it from a JUnit test, drive the same conformance scenarios as
   the Swift suite (push/pull/Closed/scheduler + the Rust vector byte-for-byte).
2. `cargo ndk` Android build stays green (already verified: 15 exported symbols).
3. A later Gradle module wraps this into an Android library; the wrapper sources stay plain
   Kotlin so they compile under both.

## Open points

- `events()` flow buffering semantics (backpressure policy) — start with a large buffer and
  drop-oldest? Needs the RN wake-up semantics decided first (M5 shares it).
- Whether `DispatcherScheduler` should hold a dispatcher or an executor — start with
  CoroutineDispatcher, the jsi-turbo side will reuse the same shape.

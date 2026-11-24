# Data Hub Design (数据管理层)

Status: design agreed, core implementation in progress. Decisions below were settled in
discussion on 2026-11-24; change them here first, then in code.

## 1. Role

An in-process pub/sub data layer on top of message-rs encoding. One Rust core (the "总控") owns
the subscription registry and the fan-out / last-value logic; every end embeds the same core via
FFI instead of reimplementing dispatch. Ends wrap it in their own idioms:

| End | Integration |
|---|---|
| Swift | ObservableObject / AsyncSequence-style wrapper, decode via the generated `Schema.swift` |
| Kotlin | SharedFlow/StateFlow-style wrapper, decode via the generated `Schema.kt` |
| RN / JS | native module over the same core + event-emitter ("mbox"-style) API; TS already has `decodeBy(key)` |

The core is payload-agnostic: it moves `(id, kind, bytes)` and knows no schema, mirroring the
schema-free runtimes of the three languages.

## 2. Data model

A data item is `(id: u32, kind: ValueKind, payload: bytes)`.

- `ValueKind` (stable u8 discriminants, they cross FFI):
  `Bool=0, U8, I8, U16, I16, U32, I32, U64, I64, F32, F64, String=11, Bytes=12, Message=13`
  — order matches `message_core::SimpleType`, plus `Message`.
- Byte layouts follow message-rs wire conventions: little-endian scalars, `bool` one byte,
  `String` raw UTF-8 (payload length is the length), `Bytes` raw.
- `Message` payloads are message-rs encodings; the item id is then the message key, so ends can
  decode by key with their generated code. Core does not enforce the id/key match.
- Basic kinds exist so simple values (flags, counters, strings) do not need a struct per value.

## 3. Semantics (settled)

- **Push**: `subscribe(id, cb)` is pure forwarding. Subscribing never replays the last value.
- **Pull**: the hub keeps the last published `(kind, payload)` per id; `get(id)` returns it
  (None if never published). Push and pull are two views of the same store: `publish` updates
  the last value *before* dispatching, so `get` inside a callback returns the value being
  delivered.
- **No subscribers**: `publish` silently stores and returns (pull still works).
- **Delivery order**: a single internal lock serializes publishes → per-id FIFO; callbacks run
  outside the lock on a snapshot of the subscriber list:
  - unsubscribing inside a callback: the current delivery still completes, the next one misses;
  - subscribing inside a callback: does not receive the current delivery;
  - publishing another id inside a callback is allowed and dispatches recursively (re-entrancy
    is the publisher's responsibility; no cycle protection).
- **Failures**: callbacks cannot fail (fire-and-forget). In the default synchronous mode a slow
  callback blocks its publisher — the escape hatch is the scheduler (below).

## 4. Threading model

- `Hub: Send + Sync`; interior `Mutex<HashMap<DataId, Entry>>`, dispatch outside the lock.
- `Entry { kind, last: Arc<[u8]>, subs: Vec<Sub> }`; the `Arc<[u8]>` avoids copying the payload
  per subscriber and gives scheduled deliveries owned data.
- **Scheduler hook**: `set_scheduler(Option<Scheduler>)` where
  `Scheduler = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>`. With none (default),
  callbacks run synchronously on the publisher's thread. Platforms register one to land
  callbacks on their own loop (Swift main queue / dispatch queue, Kotlin handler or coroutine
  dispatcher, RN JS thread via JSI call invoker). In scheduler mode `publish` returns before
  callbacks run; ordering becomes the scheduler's property.
- `Subscription`: RAII (`Drop` unsubscribes) plus an idempotent explicit `unsubscribe()`. It
  holds a `Weak` reference to the hub, so dropping subscriptions after the hub is harmless.

## 5. Core API (crate `message-hub`, dir `hub/`, zero dependencies)

```rust
pub type DataId = u32;
pub type Callback = Arc<dyn Fn(DataId, ValueKind, &[u8]) + Send + Sync>;

impl Hub {
    pub fn new() -> Self;
    pub fn subscribe(&self, id: DataId, cb: Callback) -> Subscription;
    pub fn publish(&self, id: DataId, kind: ValueKind, payload: &[u8]);
    pub fn get(&self, id: DataId) -> Option<(ValueKind, Arc<[u8]>)>;   // pull
    pub fn set_scheduler(&self, s: Option<Scheduler>);
    // convenience over Value (encode/decode in value.rs)
    pub fn publish_value(&self, id: DataId, v: &Value);
    pub fn get_value(&self, id: DataId) -> Option<Result<Value, ValueError>>;
}
```

`value.rs` (already written) provides `ValueKind`, `Value` and `encode`/`decode` with
length/UTF-8 validation for basic kinds; `Message` bytes pass through untouched.

## 6. Milestones

- **M1 (current)** — Rust core: `hub/src/value.rs` (done), `hub/src/lib.rs` (`Hub`,
  `Subscription`, `Scheduler`; TODO), `hub/tests/hub.rs` (TODO), add `hub` to workspace members.
- **M2** — C-ABI surface (`hub_ffi.rs`, cdylib): opaque handles,
  `hub_publish(h, id, kind_u8, ptr, len)`, `hub_subscribe(h, id, trampoline(ctx, id, kind, ptr,
  len), ctx, ctx_drop)`, `hub_get` + buffer release, `hub_set_scheduler(fn, ctx)`; round-trip
  tests from C or via `cbindgen`/UniFFI decision.
- **M3** — Swift binding + Observable-style wrapper; conformance: publish message vectors from
  Rust, decode in Swift via `Schema.swift`.
- **M4** — Kotlin binding + Flow-style wrapper; same conformance via `Schema.kt`.
- **M5** — RN native module (iOS/Android) over the same core; JS emitter API + `decodeBy`
  integration.
- **M6 (later)** — per-id retention/limits, metrics, cross-process transport (only if needed).

## 7. M1 test list

1. subscribe → publish → callback receives `(id, kind, bytes)`;
2. publish with no subscribers is silent, `get` still returns the last value;
3. multiple subscribers all receive, in registration order;
4. dropping `Subscription` stops deliveries;
5. explicit `unsubscribe()` is idempotent;
6. `get` returns the latest after several publishes;
7. subscribing does not replay the last value;
8. unsubscribing inside a callback: current delivery completes, the next misses;
9. publishing another id inside a callback works;
10. `get` inside a callback returns the value being delivered;
11. concurrent publish/subscribe/unsubscribe across threads: no deadlock, everything delivered;
12. scheduler mode: deliveries run on the scheduler's executor, not the publisher thread;
13. `Value` encode/decode round-trips for every kind; wrong lengths and invalid UTF-8 rejected;
14. dropping subscriptions after the hub does not panic;
15. the kind travels with each publish (re-published id can change kind; core does not police it).

## 8. State on disk (for the next device)

- `hub/Cargo.toml`, `hub/src/value.rs` — written (uncommitted at the time of this note).
- `hub/src/lib.rs`, `hub/tests/hub.rs` — not started.
- Root `Cargo.toml` workspace `members` does not include `hub` yet.
- Repo conventions: English comments/docs/errors; commits carry no attribution lines.

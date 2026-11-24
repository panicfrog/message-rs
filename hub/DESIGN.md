# Data Hub Design (数据管理层)

Status: design agreed, M1 (Rust core) implemented; M2 (C-ABI) next. Decisions below were settled
in discussion on 2026-11-24; change them here first, then in code.

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

A data item is `(id: u32, kind: ValueKind, payload: bytes)`, owned by a publisher.

- `ValueKind` (stable u8 discriminants, they cross FFI):
  `Bool=0, U8, I8, U16, I16, U32, I32, U64, I64, F32, F64, String=11, Bytes=12, Message=13`
  — order matches `message_core::SimpleType`, plus `Message`.
- Byte layouts follow message-rs wire conventions: little-endian scalars, `bool` one byte,
  `String` raw UTF-8 (payload length is the length), `Bytes` raw.
- `Message` payloads are message-rs encodings; the item id is then the message key, so ends can
  decode by key with their generated code. Core does not enforce the id/key match.
- Basic kinds exist so simple values (flags, counters, strings) do not need a struct per value.
- A **publisher** is an explicit RAII handle bound to one `(id, kind)`; the kind is fixed for its
  lifetime (a publisher is a source of exactly one type, so ends can expose `Publisher<T>`).
  Several publishers may share an id. All publishing goes through a handle — there is no one-shot
  `hub.publish(id, ...)`, because an ephemeral publisher would make "the last publisher closed"
  meaningless.

## 3. Semantics (settled)

- **Push**: `subscribe(id, cb)` is pure forwarding. Subscribing never replays the last value.
- **Pull**: the hub stores nothing. Each publisher keeps the payload of its own last publish;
  `get(id)` returns the newest of those among the id's *live* publishers, `None` when none ever
  published or none is left. Pull runs no user code. (A lazy provider — `get` calling into the
  publisher for a value newer than its last push — was considered and deferred: it would put
  user code inside `get`, with the same re-entrancy, threading and FFI ctx-lifetime issues as
  dispatch. It can be added later without changing anything else here.)
- **Lifecycle**: closing the last live publisher of an id delivers `Event::Closed` to its
  subscribers — exactly once, after every value in synchronous mode — and `get` returns `None`
  from then on. Subscriptions survive `Closed`: a new publisher on the same id reaches them again.
  Publishing after `close` is a silent no-op; `close` is idempotent (`Drop` closes as well).
- **No subscribers**: `publish` silently records the value and returns (pull still works).
- **Delivery order**: a single internal lock serializes the registry update of every publish, so
  publishes from one thread arrive per id in order and the newest value always wins in `get`;
  callbacks run outside the lock on a snapshot of the subscriber list, so across publisher
  threads every delivery happens but their relative order is unspecified:
  - unsubscribing inside a callback: the callback already running completes, the next delivery
    misses; a subscriber unsubscribed earlier in the same dispatch is skipped;
  - subscribing inside a callback: does not receive the current delivery;
  - publishing or closing inside a callback is allowed and dispatches recursively (re-entrancy
    is the caller's responsibility; no cycle protection).
- **Failures**: callbacks cannot fail (fire-and-forget). In the default synchronous mode a slow
  callback blocks its publisher — the escape hatch is the scheduler (below).

## 4. Threading model

- `Hub: Send + Sync` over an `Arc<Inner>` (so `Hub` is cheap to clone and shares one registry);
  `Inner` holds `Mutex<State>`. The lock guards the registry — which ids exist, their
  subscribers, their live publishers and each one's last value — and never user code: dispatch
  and `Closed` notifications run outside it.
- `State { ids: HashMap<DataId, Entry>, seq, scheduler }`,
  `Entry { subs: Vec<Arc<SubState>>, pubs: Vec<PubRec> }`, `PubRec { kind, seq, last }`. `seq` is
  a global counter stamped at each publish; `get` picks the live record with the greatest `seq`
  that has published. An entry is removed once it has neither subscribers nor live publishers.
- **Why the lock stays**: explicit handles solve lifetime (`Arc` + RAII), not mutual exclusion —
  handles are created and dropped from any thread and every publish still needs a snapshot of
  the subscribers. Critical sections are a map lookup plus one `Arc` clone per subscriber;
  per-id locks are a later option only if contention is ever measured.
- **Snapshot**: everything handed to user code outside the lock is an immutable owned value —
  the payload (`Arc<[u8]>`, replaced per publish, so a running callback never sees a torn or
  newer buffer) and the subscriber list (cloned per publish). A delivery skips subscribers whose
  `alive` flag is clear; that makes unsubscribe take effect as early as possible and, under FFI,
  lets a trampoline ctx be freed with its last `Arc` instead of at unsubscribe time.
- **Scheduler hook**: `set_scheduler(Option<Scheduler>)` where
  `Scheduler = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>`. With none (default),
  callbacks run synchronously on the publisher's thread. Platforms register one to land
  callbacks on their own loop (Swift main queue / dispatch queue, Kotlin handler or coroutine
  dispatcher, RN JS thread via JSI call invoker). In scheduler mode `publish` returns before
  callbacks run; ordering becomes the scheduler's property, and `get` inside a callback may
  already return a value newer than the one being delivered — callbacks must use their
  arguments.
- **Subscription**: RAII (`Drop` unsubscribes) plus an idempotent explicit `unsubscribe()`.
- **Publisher**: RAII (`Drop` closes) plus an explicit idempotent `close()`. Both handles hold a
  `Weak` reference to the hub, so using or dropping them after the hub is harmless. Ends with
  nondeterministic destruction (Kotlin) must expose `close()` and call it from their lifecycle
  (`ViewModel.onCleared`), otherwise pull outlives the object.

## 5. Core API (crate `message-hub`, dir `hub/`, zero dependencies)

```rust
pub type DataId = u32;

/// What a subscriber receives: a published value, or the end of every publisher of its id.
pub enum Event<'a> { Value(ValueKind, &'a [u8]), Closed }

pub type Callback = Arc<dyn Fn(DataId, Event<'_>) + Send + Sync>;

impl Hub {
    pub fn new() -> Self;
    pub fn publisher(&self, id: DataId, kind: ValueKind) -> Publisher;
    pub fn subscribe(&self, id: DataId, cb: Callback) -> Subscription;
    pub fn get(&self, id: DataId) -> Option<(ValueKind, Arc<[u8]>)>;   // pull
    pub fn set_scheduler(&self, s: Option<Scheduler>);
    pub fn get_value(&self, id: DataId) -> Option<Result<Value, ValueError>>;
}

impl Publisher {                       // RAII: Drop closes
    pub fn id(&self) -> DataId;
    pub fn kind(&self) -> ValueKind;
    pub fn publish(&self, payload: &[u8]);
    pub fn publish_value(&self, v: &Value) -> Result<(), ValueError>;  // Err on kind mismatch
    pub fn close(&self);               // idempotent
}

impl Subscription {
    pub fn id(&self) -> DataId;
    pub fn unsubscribe(&self);         // idempotent
}
```

`hub/src/value.rs` — the value codec module:

- `ValueKind`: `Copy`, `#[repr(u8)]`, stable discriminants per section 2, plus `from_u8(u8)`
  and `fixed_len() -> Option<usize>` (fixed-size kinds only);
- `Value`: one variant per kind (`Bool(bool)` … `F64(f64)`, `String(String)`, `Bytes(Vec<u8>)`,
  `Message(Vec<u8>)`), with `kind()` and `encode() -> Vec<u8>` (little-endian scalars, raw
  UTF-8 / raw bytes, `Message` passthrough);
- `decode(kind, &[u8]) -> Result<Value, ValueError>`: fixed-size kinds reject wrong lengths,
  `String` rejects invalid UTF-8, `Bytes` / `Message` pass through; `ValueError(&'static str)`.

## 6. Milestones

- **M1 (done)** — Rust core: `hub/Cargo.toml` (crate `message-hub`, zero dependencies),
  `hub/src/value.rs`, `hub/src/lib.rs` (`Hub`, `Publisher`, `Subscription`, `Event`,
  `Scheduler`), `hub/tests/hub.rs` (section 7); `hub` is a root workspace member.
- **M2** — C-ABI surface (`hub_ffi.rs`, cdylib): opaque handles,
  `hub_publisher_create(h, id, kind_u8)`, `hub_publisher_publish(p, ptr, len)`,
  `hub_publisher_close(p)` / `hub_publisher_release(p)`,
  `hub_subscribe(h, id, trampoline(ctx, id, kind_u8, ptr, len), ctx, ctx_drop)` with a reserved
  `kind_u8` delivering `Closed` as `ptr = null, len = 0`, `hub_get` + buffer release,
  `hub_set_scheduler(fn, ctx)`; headers via `cbindgen`, round-trip tests from C. UniFFI is
  ruled out: the core stays dependency-free, all three ends share one hand-written ABI, and the
  RN layer goes through C++/JSI anyway.
- **M3** — Swift binding + Observable-style wrapper; conformance: publish message vectors from
  Rust, decode in Swift via `Schema.swift`.
- **M4** — Kotlin binding + Flow-style wrapper; same conformance via `Schema.kt`. The wrapper
  owns the publisher handles and must close them from its lifecycle, not rely on GC.
- **M5** — RN native module (iOS/Android) over the same core; JS emitter API + `decodeBy`
  integration.
- **M6 (later)** — metrics, cross-process transport (only if needed). Retention is no longer a
  topic: nothing is stored once the publishers are gone.

## 7. M1 test list

1. subscribe → publish → callback receives `(id, kind, bytes)`;
2. publish with no subscribers is silent, `get` still returns the publisher's last value;
3. multiple subscribers all receive, in registration order;
4. dropping `Subscription` stops deliveries;
5. explicit `unsubscribe()` is idempotent;
6. `get` returns the latest after several publishes;
7. subscribing does not replay the last value;
8. unsubscribing inside a callback: the callback completes, the next delivery misses; a
   subscriber unsubscribed earlier in the same dispatch is skipped;
9. publishing another id inside a callback works;
10. `get` inside a callback returns the value being delivered (through a cloned `Hub`);
11. concurrent publish/subscribe/unsubscribe/close across threads: no deadlock, everything
    delivered to the subscribers that stayed;
12. scheduler mode: deliveries run on the scheduler's executor, not the publisher thread, while
    `get` is updated synchronously;
13. `Value` encode/decode round-trips for every kind; wrong lengths and invalid UTF-8 rejected;
14. using and dropping handles after the hub does not panic;
15. the kind is fixed per publisher: `publish_value` rejects a mismatched kind, and `get` reports
    the kind of the publisher that answered;
16. `get` is `None` before any publish and once the last publisher is closed; a second live
    publisher keeps pull working and `get` answers with the newest publish;
17. closing the last publisher delivers `Closed` to every subscriber exactly once, after every
    value; closing a non-last publisher delivers nothing;
18. publishing after `close` is a silent no-op; `close` is idempotent across explicit calls and
    `Drop`;
19. closing inside a callback is allowed and dispatches the other id's `Closed` recursively;
20. subscriptions survive `Closed` and receive values again from a new publisher.

## 8. State on disk (for the next device)

- M1 is in place under `hub/` (`Cargo.toml`, `src/value.rs`, `src/lib.rs`, `tests/hub.rs`) and
  `hub` is a root workspace member; `cargo test -p message-hub` covers section 7. Start at M2.
- Repo conventions: English comments/docs/errors; commits carry no attribution lines.

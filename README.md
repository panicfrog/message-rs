# message-rs

Define your data once in Rust; read and write the same bytes from Rust, TypeScript, Swift and
Kotlin — and share live values between them inside one app.

Two things ship here. An **encoding** layer: `#[derive(Message)]` computes the layout at compile
time and generates TS/Swift/Kotlin code producing **byte-identical** buffers, with zero-copy
readers and an append-only evolution contract enforced in CI. And a **data hub**: one Rust
registry per process that JS, Swift and Kotlin all publish and subscribe through, so a value
written in Kotlin is readable from JS with no serialization boundary in between.

```
ENCODING                              DATA HUB (independent)

your Rust structs                     message-hub (Rust)
      │                               one registry per process
      │ #[derive(Message)]                  │
      ▼                             ┌───────┼───────┐
   IR at build time                 ▼       ▼       ▼
      │                          hub-turbo hub-ffi hub-jni
      ├──────────┬──────────┐       (JS)   (Swift) (Kotlin)
      ▼          ▼          ▼
  schema.ts  Schema.swift  Schema.kt   ← generated, literal offsets
      │          │          │
  TS runtime  MessageRuntime  message.rs  ← hand-written, schema-free
      └──────────┴──────────┘
                 │
        one wire format (SPEC.md)
```

The two halves meet at one codec protocol per language: a generated type conforms to it, and that
is what makes it publishable through the hub. Neither half depends on the other.

## What problem this solves

Cross-language mobile apps normally pay for every boundary: a React Native screen, a Swift
feature and a Kotlin feature each keep their own copy of the same data, and keeping them in sync
means JSON over a bridge, hand-written DTOs on both sides, and a schema that drifts.

message-rs removes the drift and the copies:

- **Drift** — three languages are generated from the one Rust definition, and CI checks that a
  committed byte vector still decodes identically everywhere. Changing a field in a
  backward-incompatible way fails the build instead of failing in production.
- **Copies** — the hub is one Rust registry embedded by every end. Publishing is forwarding, not
  broadcasting serialized copies; pulling returns the newest value. `example/` is a real RN app
  that proves this on both platforms.

## Two independent pieces

Use either one alone. They compose, but neither depends on the other.

| | What it is | Docs |
|---|---|---|
| **Encoding** (`protocol/`, `runtime/`) | The wire format, the derive macro, the code generator and three schema-free runtimes | [SPEC.md](SPEC.md) |
| **Data hub** (`hub/`) | In-process pub/sub over `(id, kind, bytes)`; one Rust core, three language surfaces | [hub/DESIGN.md](hub/DESIGN.md) |

The hub moves bytes and knows no schema; supporting a new encodable type means implementing the
codec protocol, never editing the hub.

## Repository layout

```
protocol/
  message-core      IR data model, layout planner, read/validate primitives, traits
  message-derive    #[derive(Message)] / #[derive(MessageEnum)]
  message-gen       export / check / ts / swift / kotlin  (the export binary and evolution gate)
  message-fixtures  test messages + cross-language conformance vectors
  playground        scratch schema for trying things out
runtime/ts/         zero-dependency TS runtime + generated schema.ts
runtime/swift/      SwiftPM package: MessageRuntime + generated Schema.swift + conformance tests
runtime/kotlin/     Gradle project: message-runtime + generated Schema.kt + conformance tests
hub/                message-hub: the Rust registry (crate root) + DESIGN.md
  ffi/              hub-ffi: cxx bridge for Swift
  jni/              hub-jni: JNI surface for Kotlin
  apple/            SwiftPM package MessageHub (the Swift surface)
  kotlin/           Gradle module hub-kotlin (the Kotlin surface)
  turbo/            hub-turbo: npm package + Rust turbo module (the JS surface)
example/            React Native demo app proving one registry across JS/Swift/Kotlin
vectors/            committed IR snapshot and conformance vectors (generated)
```

---

# Part 1 — Encoding

## Define a message

```rust
use message_derive::{Message, MessageEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MessageEnum)]
pub enum Color { Red, Green, Blue }

#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0001)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub color: Color,
    pub profile: Profile,                   // nested struct (defined below)
    pub tags: Vec<String>,
    pub counts: HashMap<Color, u32>,        // enum keys: entries sorted by value
    pub avatar: Option<Vec<u8>>,
    #[msg(precise)]
    pub precise_id: u64,                    // bigint on the TS side
    #[msg(reserved)]
    pub legacy_flag: bool,                  // deleted from the API, layout kept
    #[msg(since = 2)]
    pub email: Option<String>,              // v2 extension block
}
```

Every type needs a unique non-zero `key`. Supported field types: scalars, `String`, `Vec<u8>`,
enums, nested structs, fixed and dynamic arrays (nested), `HashMap`/`BTreeMap` with scalar /
`String` / enum keys, and `Option<T>` of any of these.

## Read and write in Rust

```rust
let buf = user.encode();                   // canonical bytes
User::validate(&buf)?;                     // untrusted input: check first, then nothing can panic
let r = User::decode(&buf).unwrap();       // zero-copy
let name = r.name();                       // &str borrowed from buf
let tags = r.tags();                       // lazy view
let user2 = r.to_owned();                  // plain struct
```

Two layers, deliberately: `UserReader` is a **view** over the buffer (nothing is copied or
allocated until you ask for an owned value), and `User` is a plain struct. `decode` is lazy — it
only checks the header length and key. Call `validate` first on untrusted bytes; after it returns
no error, no accessor can panic.

## Generate the other languages

Add an export binary to your crate:

```rust
// src/bin/export-schema.rs
message_gen::export_main!();
use your_crate as _link;   // so the derive registrations are linked in
```

Then:

```sh
cargo run --bin export-schema -- export message-snapshot.json   # JSON IR snapshot (commit this)
cargo run --bin export-schema -- check  message-snapshot.json   # evolution gate (run in CI)
cargo run --bin export-schema -- ts     path/to/schema.ts
cargo run --bin export-schema -- swift  path/to/Schema.swift
cargo run --bin export-schema -- kotlin path/to/Schema.kt [--package com.example.schema]
cargo run --bin export-schema -- ir --key 1                     # debug one type's layout
```

The generated code has **literal offsets** — no schema object ships to the runtime, and nothing
recomputes layout at runtime. Each runtime holds only read/write primitives and views.

This repo's own fixtures use the same commands via `-p message-fixtures --bin export-schema`;
`protocol/playground` is a scratch schema you can edit freely to try things out.

## Evolution: append-only, enforced

The base layout is frozen forever. To add a field, give it `#[msg(since = N)]` with `N` above the
current maximum: it lands in a new extension block, older readers ignore it, newer readers read
old buffers with defaults.

**This works for nested structs too.** A struct referenced by another type is stored as a
self-contained instance with its own extension chain, so it can gain fields at any time without
its containers changing:

```rust
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0002)]
pub struct Profile {
    pub age: u32,
    pub bio: String,
    #[msg(since = 2)]
    pub verified: Option<bool>,             // added later; User.profile keeps working unchanged
}
```

To remove a field from the API, mark it `#[msg(reserved)]` rather than deleting it — the layout
keeps its place, no accessor is generated, and the other languages encode zeros.

`check` compares against the committed snapshot and rejects everything else: reordering,
retyping, deleting, shrinking extension blocks, flipping `precise`, changing enum variants.
Allowed: appending with a larger `since`, adding types, marking a field reserved. Full rules in
[SPEC.md §7](SPEC.md#7-evolution-rules-append-only).

## TypeScript

Zero dependencies; runs on Hermes (React Native), browsers and Node.

```ts
import { User, UserReader } from './schema';

const err = User.validate(bytes);          // null when well-formed, else a field path
const u = User.fromBytes(bytes);           // plain object
const r = UserReader.decode(bytes);        // zero-copy view
const again = User.encode(u);              // byte-identical to Rust
```

`u64` fields decode to `number` unless marked `#[msg(precise)]`, which gives `bigint`. Map keys
are strings on the object layer (enum keys are their numeric value as a string).

## Swift

Add `runtime/swift` (package `message-rs`, product `MessageRuntime`) as a SwiftPM dependency and
compile the generated `Schema.swift` into your target.

```swift
let user = User(id: 42, name: "山田", color: .green, /* ... */)
let data = user.encode()                   // byte-identical to Rust and TS

if User.validate(data) == nil {            // ValidationError? with a field path
    let r = UserReader.decode(data)!       // zero-copy: r.profile.bio, r.tags[1], r.byId.key(at: 0)
    let copy = r.toOwned()                 // plain struct
}
```

## Kotlin

Add `runtime/kotlin` (Gradle project `message-rs`, module `message-runtime`) and compile the
generated `Schema.kt` into your target. `--package` picks its package; the default is the
runtime's own `message.rs`.

```kotlin
val user = User(id = 42uL, name = "山田", color = Color.GREEN, /* ... */)
val data = user.encode()                   // byte-identical to Rust, TS and Swift

if (User.validate(data) == null) {         // ValidationError? with a field path
    val r = UserReader.decode(data)!!      // zero-copy: r.profile.bio, r.tags[1], r.byId.key(0)
    val copy = r.toOwned()                 // plain data class
}
```

Kotlin's unsigned types map exactly to Rust's (`UByte`/`UShort`/`UInt`/`ULong`), so no width is
lost or guessed.

---

# Part 2 — Data hub

An in-process pub/sub layer. **One Rust registry per process**, embedded by every language
surface — not three implementations that talk to each other.

- **Push** is pure forwarding: subscribing never replays the last value.
- **Pull** answers with the newest published value of an id.
- **Closed** arrives exactly once when an id's last publisher goes away; subscriptions survive it
  and reach the next publisher.

## Value-first on every end

Callers pass values, never wire kinds or byte arrays. Basic types and generated structs go
through the same call:

```ts
// JS (hub-turbo)
import { publish, pull, channel, close, u16 } from 'hub-turbo';
import { User } from './schema';

publish(1, 'hello');                       // kind follows the JS type
publish(2, u16(65535));                    // exact width when you need it
publish(3, user, User.codec);              // a generated struct, no manual encode

const s = pull(1);                         // 'hello', already decoded
const u = pull(3, User.codec);             // User | null
const off = channel(3, User.codec, (e) => { if (!e.closed) console.log(e.value?.name); });
off();
close(3);
```

```swift
// Swift (MessageHub)
Hub.shared.publish(id: 1, "hello")
Hub.shared.publish(id: 2, UInt16(65535))
Hub.shared.publish(id: 3, user)            // any MessageCodec type

let s: String? = Hub.shared.pull(id: 1)
let u: User? = Hub.shared.pull(id: 3)
for await event in Hub.shared.events(id: 3) { /* ... */ }
Hub.shared.close(id: 3)
```

```kotlin
// Kotlin (hub-kotlin)
Hub.shared.publish(1, "hello")
Hub.shared.publish(2, 65535.toUShort())
Hub.shared.publish(3, user)                // any MessageEncodable type

val s = Hub.shared.pull(1)                 // Value.Str
val u = Hub.shared.pull(3, User.Companion) // User?
Hub.shared.events(3).collect { /* ... */ } // Flow; HubState mirrors one id as StateFlow
Hub.shared.close(3)
```

Generics on the basic-type paths are **compile-time hints only** — the value always comes from
decoding the sender's real kind. Pass a codec (TS) or companion (Kotlin) when you want an actual
checked decode into a struct.

The JS package is `@message-rs/hub-turbo`; the demo installs it under the shorter alias
`"hub-turbo": "file:../hub/turbo"`, which is what the import above uses.

## The codec protocol

Each runtime defines one contract; generated types conform to it using the `encode`/`fromBytes`
they already have. That is the whole integration — supporting a new type means implementing the
protocol, not editing the hub.

| End | Protocol | Where |
|---|---|---|
| Swift | `MessageCodec` | `MessageRuntime` |
| Kotlin | `MessageEncodable` + `MessageCodecCompanion<T>` | `message.rs` |
| TS | `MessageCodec<T>` (each namespace exports `codec`) | `runtime.ts` |

Kotlin splits it in two because `fromBytes` lives on the companion. TS takes the codec explicitly
at the call site because a plain JS object carries no reliable runtime type tag.

## Handles and lifetime

`Publisher` and `Subscription` are RAII on every end (`deinit` / finalizer / explicit `close`).
A wrapper handle **retains its owning hub**: an isolated registry is freed when its wrapper dies,
so a handle that outlived the hub variable would silently stop delivering. Use `Hub.shared` for
app-wide state; `Hub()` / `Hub.create()` gives an isolated registry for tests and tools.

On Kotlin, close handles from your lifecycle (`ViewModel.onCleared`) rather than relying on GC.

## One Rust copy per process

This is the constraint that shapes packaging. Each surface used to ship its own core; symbol
inspection showed three distinct `Hub::shared::SHARED` statics with different crate hashes, which
the linker never coalesces — three registries, so the ends could not see each other.

Now `hub-turbo` carries the platform's native surface (iOS/macOS: `hub-ffi`, Android: `hub-jni`),
so installing that one package links a single core. The Swift package has a superlib mode
(`MESSAGE_HUB_SUPERLIB=1`: headers only, no xcframework) and the Kotlin wrapper a library-name
switch (`-Dhub.libName=rust_app`). Details and the traps found on the way: `hub/DESIGN.md` and
`example/PLAN.md`.

## The demo

`example/` is a React Native app that runs six cross-language checks on launch and renders each
verdict — including publishing a real generated struct natively and decoding it in JS. Both
platforms pass; screenshots in `example/docs/`. See [example/README.md](example/README.md) for how
to run it.

---

## Development

Each line runs from the repo root (subshells, so they do not change your directory):

```sh
cargo test --workspace

(cd runtime/ts     && npm install && npm run typecheck && npm test)
(cd runtime/swift  && swift test)
(cd runtime/kotlin && ./gradlew test)

(cd hub/apple  && ./scripts/build-xcframework.sh && swift test)
cargo build -p hub-jni            # the host dylib hub/kotlin's tests load
(cd hub/kotlin && ./gradlew test)
(cd hub/turbo  && npm install && npm run typecheck && npm test)
```

CI (`.github/workflows/ci.yml`) runs all of the above, the evolution gate, and verifies that the
generated artifacts (`vectors/*`, `runtime/ts/src/schema.ts`,
`runtime/swift/Sources/MessageFixtures/Schema.swift`, `runtime/kotlin/**/Schema.kt`) are up to
date. The demo app build stays out of CI (it needs pods/NDK and a running simulator).

## Status

Not yet published. `message-hub`, `hub-ffi` and `hub-jni` are intended for crates.io (pure Rust,
no React Native coupling); `hub-turbo` as an npm package; the Swift and Kotlin surfaces are
consumed as local packages for now.

## Documentation

- [SPEC.md](SPEC.md) — the authoritative wire format, evolution rules and per-language API semantics
- [hub/DESIGN.md](hub/DESIGN.md) — the data hub's design, decisions and the findings behind them
- [example/README.md](example/README.md) / [example/PLAN.md](example/PLAN.md) — the demo and how one-copy linking was solved
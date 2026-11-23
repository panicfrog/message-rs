# message-rs

A compact binary message format with **compile-time layout**, **zero-copy readers**, an
**append-only evolution contract** and a **TypeScript runtime** that produces byte-identical
output to Rust. The wire format is specified in [SPEC.md](SPEC.md).

```
protocol/
  message-core      IR data model, layout planner, read/validate primitives, traits
  message-derive    #[derive(Message)] / #[derive(MessageEnum)]
  message-gen       export / check / ts  (the export binary and evolution gate)
  message-fixtures  test messages + cross-language conformance vectors
  playground        scratch schema for trying things out
runtime/ts/         zero-dependency TS runtime + generated schema.ts
vectors/            committed IR snapshot and conformance vectors (generated)
```

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

```rust
let buf = user.encode();                   // canonical bytes
User::validate(&buf)?;                     // untrusted input: check first, then nothing can panic
let r = User::decode(&buf).unwrap();       // zero-copy
let name = r.name();                       // &str borrowed from buf
let tags = r.tags();                       // lazy view
let user2 = r.to_owned();                  // plain struct
```

Supported field types: scalars, `String`, `Vec<u8>`, enums, inline structs, fixed and dynamic
arrays (nested), `HashMap`/`BTreeMap` with scalar / `String` / enum keys, `Option<T>` of any of
these. V1 limits are listed in the spec.

## Export, gate, generate

```sh
cargo run -p message-fixtures --bin export-schema -- export vectors/message-snapshot.json  # JSON IR snapshot
cargo run -p message-fixtures --bin export-schema -- check  vectors/message-snapshot.json  # evolution gate (CI)
cargo run -p message-fixtures --bin export-schema -- ts    runtime/ts/src/schema.ts        # TypeScript schema
cargo run -p message-fixtures --bin export-schema -- vectors vectors/user.json             # conformance vectors
```

In your own crate, add `src/bin/export-schema.rs` containing `message_gen::export_main!();`
(plus a `use your_crate as _link;` so the registrations are linked) and run the same commands.

## Evolution

The base layout is frozen forever. To add a field, give it `#[msg(since = N)]` with `N` greater
than the current maximum; it goes into a new extension block and older readers ignore it while
newer readers read old buffers with defaults. To remove a field from the API, mark it
`#[msg(reserved)]` instead of deleting it. `check` enforces all of this against the committed
snapshot; see [SPEC.md §7](SPEC.md#7-evolution-rules-append-only).

## TypeScript

```ts
import { User, UserReader } from './schema.js';

const err = User.validate(bytes);          // null when well-formed
const u = User.fromBytes(bytes!);          // plain object
const r = UserReader.decode(bytes!);       // zero-copy view
const again = User.encode(u);              // byte-identical to Rust
```

## Development

```sh
cargo test --workspace
cd runtime/ts && npm install && npm run typecheck && npm test
```

CI (`.github/workflows/ci.yml`) runs both, the evolution gate, and verifies that the generated
artifacts (`vectors/*`, `runtime/ts/src/schema.ts`) are up to date.

# message-rs wire format and evolution rules

This document is the authoritative description of the binary layout, the evolution contract and
the API semantics. The code comments in `protocol/message-core` point here; when they disagree,
this file wins.

## 1. Layers

| Layer | Crate / package | Role |
|---|---|---|
| Core | `protocol/message-core` | IR data model (`SchemaSet`, `MessageDescriptor`), pure layout planner, read primitives, validation primitives, trait contracts |
| Derive | `protocol/message-derive` | `#[derive(Message)]` / `#[derive(MessageEnum)]`: compile-time layout constants, `encode`, zero-copy `Reader`, `validate`, descriptor registration |
| Gen | `protocol/message-gen` | `export` (JSON IR snapshot), `check` (evolution gate), `ts` / `swift` / `kotlin` (language backends) |
| TS runtime | `runtime/ts` | Zero-dependency runtime (Hermes / Web / Node): read primitives, views, `Writer`, validation primitives. Schema-free |
| Swift runtime | `runtime/swift` | `MessageRuntime` (SwiftPM, iOS / macOS / Linux): `RawMessage`, views, `Writer`, validation primitives. Schema-free |
| Kotlin runtime | `runtime/kotlin` | `message.rs` package (Gradle, JVM / Android): `RawMessage`, views, `Writer`, validation primitives. Schema-free |

Every backend consumes the same IR (the derive-computed descriptor) at build time and emits code
with literal offsets; the IR itself is not shipped to the runtime. The layout planner in core
recomputes the layout from the IR and the fixtures assert that both agree field by field
(`check_consistency`); layout is never recomputed by a consumer.

## 2. Envelope

```
offset  size  field
0       4     key        u32 LE   (message key, the only place it lives)
4       2     fixed_len  u16 LE   (length of the base fixed region, bytes)
6       1     version    u8       (writer's highest extension version, 0 = base only)
7       1     flags      u8       (0)
8       ..    payload
```

`payload = [ base block ][ extension block v2 ][ extension block v3 ] ...` and each block is
`[ fixed region ][ var data ]`; an extension block additionally ends with a 6-byte trailer
(section 6). `fixed_len` in the header counts the base fixed region only.

Everything is little-endian. Alignment and layout never depend on the key.

## 3. Fixed region of a block

The fixed region holds only two kinds of things: fixed-size scalars (and enums) inline, and one
8-byte slot for everything whose size is not known at compile time. Fields are placed at natural
alignment, offsets are compile-time constants (`__Layout` in Rust, literal offsets in the
generated TS / Swift / Kotlin).

| Type | Size | Align |
|---|---|---|
| `bool`, `u8`, `i8` | 1 | 1 |
| `u16`, `i16` | 2 | 2 |
| `u32`, `i32`, `f32`, enum | 4 | 4 |
| `u64`, `i64`, `f64` | 8 | 8 |
| fixed array `[T; N]` of scalars | `N * size(T)` | `align(T)` |
| `String`, `Vec<u8>`, dynamic array, map, **struct** | 8 (slot) | 4 |
| fixed array of any other `T` (strings, enums, structs, ...) | 8 (slot) | 4 |

A struct field is never inlined: it is a slot referencing a self-contained instance (section 5),
so the parent's layout never depends on the child's and reference cycles (`A` holds `B`, `B`
holds `A`) are legal. Fixed arrays that are not pure scalars are encoded exactly like a dynamic
array of length `N`.

`bool` is one byte, `0` = false, anything else = true. Enums are `i32` (negative discriminants
allowed). Floats are IEEE-754 bit patterns. `u64`/`i64` are plain numbers unless the field is
marked `#[msg(precise)]`, which maps to `bigint` on the TS side.

### Optional fields and the bitmap

Every block whose fields include `Option<T>` starts with a bitmap: `ceil(n / 8)` bytes rounded up
to a multiple of 4, 4-byte aligned. Bit `i` of the bitmap (bit `i % 8` of byte `i / 8`) says
whether the `i`-th optional field of *that block* is present. Bits are numbered in declaration
order within the block. An absent optional field leaves its slot / storage zeroed and readers
return `None` / `null` without touching the bytes.

The fixed region ends at `cursor` rounded up to the block's max alignment (`max_align`).

## 4. Slots and var data

A slot is two little-endian u32s:

```
offset u32   payload-relative (from the start of the payload, after the header)
len    u32   string/bytes: byte count; struct instance: byte count (whole instance)
             array/map: [ shape:1 bit | count:31 bits ]
```

A slot points at one of four kinds of region in the var data:

| Region | `len` | Contents |
|---|---|---|
| byte string (`String` UTF-8, `Vec<u8>`) | bytes | raw bytes `[offset, offset + len)` |
| struct instance | bytes | `[ bitmap | fixed fields | own var data | ext v2 | ext v3 ... ]` (section 5) |
| array | shape + count | `count` element footprints, or `count` instance slots |
| map | shape + count | `count` entries `key + value`, sorted by key |

### The shape bit

The high bit of an array / map slot's `len` tells readers how the elements are laid out; the low
31 bits are the element count. The writer sets it from a write-time fact, never from a
declaration: the bit is 1 exactly when the elements (map values) are structs **whose schema has
extension blocks** at the time of writing. Readers always handle both forms, so buffers written
before and after a struct gained extension fields are equally readable.

- shape = 0 (open): every element occupies its footprint (table below) back to back;
- shape = 1 (instances): every element is an 8-byte slot pointing at a self-contained instance.

The bit is never set for strings, bytes or instance slots. A count above `2^31 - 1` cannot occur
(the payload is limited to 4 GiB by the u32 offsets).

### Arrays

Element `i` lives at `region + i * stride`:

| Element | stride |
|---|---|
| scalar / enum | its width (enum: 4) |
| `String`, `Vec<u8>`, nested dynamic array | 8 (a slot per element) |
| struct, shape = 0 | the struct's base `FIXED_LEN` (bitmap and slots included; its var data follows the region) |
| struct, shape = 1 | 8 (an instance slot per element) |

An open struct element only carries the base fields, so arrays of an unevolved struct
(`Vec<Point>`) cost nothing beyond the element footprints. Nested dynamic arrays (`Vec<Vec<T>>`)
store a slot per element pointing at the inner region.

### Maps

An entry is `key inline + value`:

```
key    at entry + 0           scalar: its width; enum: 4; String: one slot
value  at entry + v_offset
```

| Value | v_offset | stride |
|---|---|---|
| non-struct, or struct with shape = 0 (open) | `align_up(key_size, align(value))` | `align_up(v_offset + size(value), max(align(key), align(value)))` |
| struct with shape = 1 (instances) | `align_up(key_size, 4)` | `align_up(v_offset + 8, max(align(key), 4))` |

For an open struct value `size`/`align` are the struct's base `FIXED_LEN`/`MAX_ALIGN`. Both pairs
are compile-time constants; readers pick one from the shape bit (`map_entry_layouts` in core).
The IR records the pair today's writers use. Entries are sorted by key: scalars and strings by
their natural order (strings by UTF-8 bytes), enums by their `i32` value. This makes the encoding
canonical (byte-identical across languages and runs).

Valid key types: fixed-size scalars, `String`, enums. Invalid: `Vec<u8>`, arrays, maps. Valid
value types: everything a field can hold except maps and fixed arrays (V1).

### Var data order

Variable data is appended in declaration-order depth-first: base block first, then each
extension block; within a block, fields in declaration order; nested data immediately after
the slot / region that references it: an array region is followed by its elements' data in
element order (a complete instance per element in instance mode), a map region by each entry's
key data and value data, a struct field by its complete instance. Every encoder (Rust, TS,
Swift, Kotlin) follows it, which is what makes their output byte-identical. Readers do not rely
on it.

## 5. Versioned fields and struct instances

`#[msg(since = N)]` with `N >= 2` puts the field into the extension block of version `N`. All
fields with the same `since` share one block (and one bitmap). `since` must be in `1..=255`.

Every struct can evolve, including structs referenced by other types: a referenced struct is
always written as a **self-contained instance**, a headerless mini-message

```
[ bitmap | fixed fields | own var data | ext block v2 | ext block v3 ... ]
```

whose referencing slot's `len` covers the whole instance, extension chain included. Its
extension blocks have exactly the top-level format (section 6) and are located by walking back
from the instance end without crossing below its start. There is no version byte: an instance
promises nothing, so a missing block simply means an older writer and its fields degrade to
defaults. Top-level messages keep the header and the payload-tail chain.

| Reader view | extent | extension lookup |
|---|---|---|
| top-level message | the payload | walk back from the payload end, header version is the promise |
| struct field / instance-mode element or value | the instance `[lo, hi)` | walk back from `hi`, nothing promised |
| open element / value (shape = 0) | — | none (only base fields exist) |

## 6. Extension blocks

```
[ bitmap | fixed fields | var data | since: u16 | block_len: u32 ]
                                          ^-- trailer (6 bytes) at the tail
```

`block_len` covers the whole block including the trailer, so the block occupies
`[end - block_len, end)`. Blocks are appended in ascending version order after the base block
(of the message, or of the instance).

Readers locate a block by walking back from the region end: read the trailer, step over every
block newer than the one they are looking for (whether or not this reader knows that version),
stop when `since` equals the target. A block is *absent* when the chain is broken, when a block
would extend below the region start, or when the block is shorter than its fixed region. For
top-level messages `version` in the header is the writer's maximum; blocks with
`since > version` are inconsistent and make the chain invalid.

Compatibility (top-level and nested alike):

- old reader, new buffer: unknown newer blocks are stepped over, the reader's own blocks are
  still found behind them, the rest is ignored; a reader compiled before a struct evolved reads
  its instances' base fields and ignores their chains, and reads instance-mode arrays / maps
  through the shape bit;
- new reader, old buffer: missing blocks degrade to defaults (`None` / `0` / `""` / empty views),
  or throw for a required struct field written by an older schema; open elements written before
  a struct evolved read their base fields and report its newer fields as absent.

## 7. Evolution rules (append-only)

The base block and every existing extension block of a published message (or struct) are frozen.
The `check` command compares the current schema against the committed snapshot and rejects:

- removing a message, changing its key, or any change to the base layout;
- changing, reordering, inserting or deleting an existing field (type, `optional`, `since`,
  `precise`, offset, slot shape, bitmap bit);
- flipping `precise` on an existing field (changes `number` <-> `bigint` in the TS API);
- shrinking the extension block count or changing an existing block;
- un-reserving a reserved field;
- appending a field whose `since` does not exceed the current maximum;
- changing an array stride or a map entry layout (`elementSize`, `mapValueOffset`), with one
  exception: a map whose value struct gains its first extension block switches from the open
  pair to the instance pair (section 4);
- changing enum variants.

Allowed: appending fields with a strictly larger `since` (to any message or struct), adding
messages or enums, and marking an existing field `#[msg(reserved)]`.

Deployment order for nested evolution (relevant once messages ship): a struct's first extension
block makes new writers emit instance-mode containers (shape bit set) for it. Readers built from
this format handle both shapes whichever schema they were compiled against, so in a homogeneous
fleet the rollout order does not matter. Follow the readers-first order only while older-format
builds may still be reading — and because only the regenerated readers expose the new fields.

### Reserved fields

`#[msg(reserved)]` is the sanctioned deletion. The field keeps its place in the layout and is
still encoded from the Rust struct field, but:

- no accessor is generated on the Rust `Reader`;
- `to_owned()` returns `Default::default()` for it (the field type must implement `Default`);
- the TS / Swift / Kotlin side emits no getter and no field, and encodes zeros;
- `validate` ignores its bytes.

Set the Rust field to its `Default` value when Rust and the other languages must be
byte-identical.

## 8. Validation

`decode` is lazy and only checks the header length and the key; reading a corrupt buffer can
panic (Rust) or throw (TS / Swift / Kotlin). For untrusted input call `Xxx::validate(&buf)`
(Rust) or `Xxx.validate(buf)` first. After `Ok` / `null`:

- the buffer is long enough, the key and `fixed_len` match the compiled schema;
- every reachable slot (fields, array elements, map keys and values, struct instances) lies
  inside the payload, including overflow-safe element-count arithmetic under either shape;
- every string is valid UTF-8;
- every enum value is a known variant;
- every extension block the header promises is present, well-formed and at least as long as its
  fixed region;
- every struct instance is checked within its own extent: its base fields, and the fields of
  each extension block its chain offers (absent blocks are legal).

Errors carry the path to the leaf: Rust `ValidationError::Nested { field, index }` renders as
`` at `edges`[0]: at `children`[1]: invalid utf-8 ``; TS `ValidationError.path` is
`['edges', 0, 'children', 1]`; instance errors nest under the field that references them
(`` at `profile`: at `bio`: invalid utf-8 ``).

Validation guarantees safety, not canonical structure: it does not check var-data ordering or
that slots point into their own block's var region.

## 9. TypeScript API

`schema.ts` is fully generated with literal offsets; the runtime (`runtime.ts`) holds only the
read/write primitives, the views, the `Writer` buffer and the validation helpers, and knows
nothing about any schema. Each message gets:

- `XxxReader`: zero-copy view with lazy getters (`r.tags.get(1)`, `r.byId.key(0)`); `decode(buf)`
  returns `null` only for a short buffer or a key mismatch;
- `Xxx`: plain object interface (`string[]`, `Record<string, T>`, `bigint` for precise);
- `namespace Xxx`: `decode`, `fromReader`, `fromBytes`, `validate`, `encode`.

`encode(value)` is straight-line generated code (no schema walk): it reads the camelCase
properties of `value`, reserves regions with literal sizes, writes scalars through `Writer`'s
little-endian methods and back-patches slots; enum fields accept the number or a variant name.
`null` / `undefined` arrays and maps encode as empty (same as before); optional fields use the
bitmap. The `Writer` is a growable `Uint8Array` (doubling), and floats go through one reused
scratch `DataView`.

Map keys are strings on the object layer: scalars are stringified, enum keys are the **numeric
value** as a string (`{ "0": 1 }`), which is what decode produces; `encode` also accepts variant
names. Keys are resolved to their wire value (or `BigInt` for 64-bit keys) before sorting, so the
entry order matches Rust. Array-valued maps materialize as `Record<string, T[]>`.

Validation paths use the camelCase field names (`['byId', 0, 'bio']`), mirroring the API rather
than the Rust names.

## 10. Swift API

`runtime/swift` is a SwiftPM package: `MessageRuntime` is the hand-written runtime, and the
generated `Schema.swift` (one per schema, produced by `export-schema -- swift <path>`) is compiled
into the app target that depends on it. Like TypeScript, everything is generated with literal
offsets and the runtime knows nothing about any schema.

Types map to Swift idioms: `u64`/`i64` are `UInt64`/`Int64` (no `precise` distinction), floats
are `Float`/`Double`, `Vec<u8>` is `Data`, maps are `[K: V]` (enum keys are `Hashable`), fixed
arrays are `[T]`, enums are `enum Color: Int32, MessageEnum` with lowerCamelCase cases
(`Red` -> `.red`, `HTTPServer` -> `.httpServer`). Field names are lowerCamelCase (`by_id` ->
`byId`). Reserved fields are omitted entirely, as in TypeScript.

Each message gets:

- `struct Xxx: Equatable` with a public memberwise `init`, `func encode() -> Data`, and
  `static decode(_:) -> XxxReader?`, `fromBytes(_:) -> Xxx?`, `validate(_:) -> ValidationError?`;
- `struct XxxReader`: zero-copy view over a `RawMessage` (the bytes are copied once on decode
  into memory the `RawMessage` owns; scalars are plain loads, strings and `Data` are created on
  access, arrays and maps are lazy views conforming to `Sequence`);
- per map field a `XxxMapView` with `count`, `key(at:)`, `value(at:)`, `entry(at:)`;
- `toOwned()` on the reader builds the plain struct.

`encode()` sorts map keys by wire value (`String` keys by UTF-8 bytes, enum keys by
`rawValue`) so the output is byte-identical to Rust and TypeScript; `validate` behaves exactly
like the Rust and TS validators (same checks, same path semantics) and traps are impossible after
it returns `nil`. `ValidationError` is an `Equatable` indirect enum whose `description` reads
like the Rust one (``at `byId`[0]: at `bio`: slot out of bounds (...)``).

`String(validating:as:)` is used on macOS 15 / iOS 18 and later, with a lossy round-trip check
on older systems.

## 11. Kotlin API

`runtime/kotlin` is a Gradle project: `message-runtime` is the hand-written runtime (package
`message.rs`, zero dependencies, JVM / Android), and the generated `Schema.kt` (one per schema,
produced by `export-schema -- kotlin <path> [--package <pkg>]`) is compiled into the app target
that depends on it. Like TypeScript and Swift, everything is generated with literal offsets and
the runtime knows nothing about any schema.

Types map to Kotlin idioms: `u64`/`i64` are `ULong`/`Long` (unsigned types throughout; no
`precise` distinction), floats are `Float`/`Double`, `Vec<u8>` is `ByteArray`, maps are
`Map<K, V>` with typed keys, arrays are `List<T>` (nested lists for nested arrays), enums are
`enum class Color(override val value: Int) : MessageEnum` with UPPER_SNAKE_CASE cases
(`Red` -> `RED`, `HTTPServer` -> `HTTP_SERVER`). Field names are lowerCamelCase (`by_id` ->
`byId`); soft keywords stay bare (`value`, `data`, `object` as a property is fine except for
hard keywords, which are backticked). Reserved fields are omitted entirely, as in TypeScript
and Swift.

Each message gets:

- `data class Xxx` with content `equals`/`hashCode` when it holds bytes (`ByteArray` compares by
  content, also inside lists), `fun encode(): ByteArray`, and a companion with
  `decode(_): XxxReader?`, `fromBytes(_): Xxx?`, `validate(_): ValidationError?`;
- `class XxxReader(override val msg: RawMessage, override val origin: Int, override val hv: Int)`:
  zero-copy view (the bytes are copied once on decode into memory the `RawMessage` owns; scalars
  are plain `ByteBuffer` loads, strings and `ByteArray` are created on access, arrays are lazy
  read-only `AbstractList` views, map views expose `key(i)`, `value(i)`, `entry(i)` and iterate
  as `Map.Entry` lists);
- per map field a `XxxMapView` with `size`, `key(i:)`, `value(i:)`, `entry(i:)`;
- `toOwned()` on the reader builds the plain data class.

`decode(bytes)` returns null only for a short buffer or a key mismatch; reading a corrupt buffer
can throw (`IndexOutOfBoundsException` / `IllegalStateException`), so call `User.validate(bytes)`
first on untrusted input. `encode()` sorts map keys by wire value (`String` keys by UTF-8 bytes,
enum keys by `value`, scalars by their natural order) so the output is byte-identical to Rust,
TypeScript and Swift; `ValidationError` is a sealed class whose `toString()` reads like the Rust
one (``at `byId`[0]: at `bio`: slot out of bounds (...)``). Region checks use `Long` arithmetic
so element-count overflow cannot wrap around.

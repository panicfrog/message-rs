# message-rs wire format and evolution rules

This document is the authoritative description of the binary layout, the evolution contract and
the API semantics. The code comments in `protocol/message-core` point here; when they disagree,
this file wins.

## 1. Layers

| Layer | Crate / package | Role |
|---|---|---|
| Core | `protocol/message-core` | IR data model (`SchemaSet`, `MessageDescriptor`), pure layout planner, read primitives, validation primitives, trait contracts |
| Derive | `protocol/message-derive` | `#[derive(Message)]` / `#[derive(MessageEnum)]`: compile-time layout constants, `encode`, zero-copy `Reader`, `validate`, descriptor registration |
| Gen | `protocol/message-gen` | `export` (JSON IR snapshot), `check` (evolution gate), `ts` (TypeScript backend) |
| TS runtime | `runtime/ts` | Zero-dependency runtime (Hermes / Web / Node): read primitives, views, `Writer`, validation primitives. Schema-free |
| Swift runtime | `runtime/swift` | `MessageRuntime` (SwiftPM, iOS / macOS / Linux): `RawMessage`, views, `Writer`, validation primitives. Schema-free |

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

Fields are placed at natural alignment, offsets are compile-time constants (`__Layout` in Rust,
literal offsets in the generated TS).

| Type | Size | Align |
|---|---|---|
| `bool`, `u8`, `i8` | 1 | 1 |
| `u16`, `i16` | 2 | 2 |
| `u32`, `i32`, `f32`, enum | 4 | 4 |
| `u64`, `i64`, `f64` | 8 | 8 |
| `String`, `Vec<u8>`, dynamic array, map | 8 (slot) | 4 |
| inline struct | its `FIXED_LEN` | its `MAX_ALIGN` |
| fixed array `[T; N]` of inline-shaped `T` | `N * size(T)` | `align(T)` |
| fixed array of slot-shaped `T` (e.g. `[String; N]`) | 8 (slot) | 4 |

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

## 4. Var data and slots

Slot-shaped values (`String`, `Vec<u8>`, dynamic arrays, maps, fixed arrays of slot-shaped
elements) occupy one 8-byte slot in the fixed region:

```
offset u32 LE   payload-relative (from the start of the payload, after the header)
len    u32 LE   bytes for string/bytes; element count for arrays; entry count for maps
```

Strings are UTF-8. Strings and bytes are raw byte ranges `[offset, offset + len)`.

### Arrays

The slot points at the array's data region: the contiguous repetition of the element footprint.

| Element | Footprint (stride) |
|---|---|
| scalar / enum | its width (enum: 4) |
| `String`, `Vec<u8>`, nested dynamic array | 8 (a slot per element) |
| inline struct | the struct's `FIXED_LEN` (bitmap and slots included) |
| fixed array of inline-shaped `T` | `N * size(T)` (inlined) |

Element `i` lives at `region + i * stride`. Nested dynamic arrays (`Vec<Vec<T>>`) store a slot per
element pointing at the inner region.

Fixed arrays `[T; N]` with inline-shaped `T` are written inline in the fixed region (no slot);
fixed arrays of slot-shaped `T` are encoded exactly like a dynamic array of length `N`.

### Maps

The slot points at the entry region. An entry is `key inline + value`:

```
key    at entry + 0           scalar: its width; enum: 4; String: one slot
value  at entry + v_offset    v_offset = align_up(key_size, align(value))
stride = align_up(v_offset + size(value), max(align(key), align(value)))
```

The value uses the same rules as a field (inline scalar/struct, slot for string/bytes/dynamic
array). Entries are sorted by key: scalars and strings by their natural order, enums by their
`i32` value. This makes the encoding canonical (byte-identical across languages and runs).

Valid key types: fixed-size scalars, `String`, enums. Invalid: `Vec<u8>`, arrays, maps. Valid
value types: everything a field can hold except maps and fixed arrays (V1).

### Var data order

Variable data is appended in declaration-order depth-first: base block first, then each
extension block; within a block, fields in declaration order; nested data (array of structs,
map values) immediately after the slot/entry that references it. Rust `encode` and the TS
`encodeMessage` both follow it, which is what makes their output byte-identical. Readers do
not rely on it.

## 5. Versioned fields

`#[msg(since = N)]` with `N >= 2` puts the field into the extension block of version `N`. All
fields with the same `since` share one block (and one bitmap). `since` must be in `1..=255`.

A struct that has versioned fields cannot be referenced by other types in V1 (inline, array
element or map value): an inlined child only contributes its base part, so nested evolution would
silently drop data. The planner rejects it (`EvolvedStructInline`); evolve the containing
message instead.

## 6. Extension blocks

```
[ bitmap | fixed fields | var data | since: u16 | block_len: u32 ]
                                          ^-- trailer (6 bytes) at the tail
```

`block_len` covers the whole block including the trailer, so the block occupies
`[end - block_len, end)`. Blocks are appended in ascending version order after the base block.

Readers locate a block by walking back from the payload end: read the trailer, step over every
block newer than the one they are looking for (whether or not this reader knows that version),
stop when `since` equals the target. A block is *absent* when the chain is broken or when the
block is shorter than its fixed region. `version` in the header is the writer's maximum; blocks
with `since > version` are inconsistent and make the chain invalid.

Compatibility:

- old reader, new buffer: unknown newer blocks are stepped over, the reader's own blocks are
  still found behind them, the rest is ignored;
- new reader, old buffer: missing blocks degrade to defaults (`None` / `0` / `""` / empty views),
  or throw for a required struct field written by an older schema.

## 7. Evolution rules (append-only)

The base block and every existing extension block of a published message are frozen. The
`check` command compares the current schema against the committed snapshot and rejects:

- removing a message, changing its key, or any change to the base layout;
- changing, reordering, inserting or deleting an existing field (type, `optional`, `since`,
  `precise`, offset, slot shape, bitmap bit);
- flipping `precise` on an existing field (changes `number` <-> `bigint` in the TS API);
- shrinking the extension block count or changing an existing block;
- un-reserving a reserved field;
- appending a field whose `since` does not exceed the current maximum;
- changing enum variants.

Allowed: appending fields with a strictly larger `since`, adding messages or enums, and marking an
existing field `#[msg(reserved)]`.

### Reserved fields

`#[msg(reserved)]` is the sanctioned deletion. The field keeps its place in the layout and is
still encoded from the Rust struct field, but:

- no accessor is generated on the Rust `Reader`;
- `to_owned()` returns `Default::default()` for it (the field type must implement `Default`);
- the TS side emits no getter, no interface field and no `toUser()` entry, and encodes zeros;
- `validate` ignores its bytes.

Set the Rust field to its `Default` value when Rust and TS output must be byte-identical.

## 8. Validation

`decode` is lazy and only checks the header length and the key; reading a corrupt buffer can
panic (Rust) or throw (TS). For untrusted input call `Xxx::validate(&buf)` (Rust) or
`Xxx.validate(buf)` (TS) first. After `Ok` / `null`:

- the buffer is long enough, the key and `fixed_len` match the compiled schema;
- every reachable slot (fields, array elements, map keys and values, nested structs) lies inside
  the payload, including overflow-safe element-count arithmetic;
- every string is valid UTF-8;
- every enum value is a known variant;
- every extension block the header promises is present, well-formed and at least as long as its
  fixed region.

Errors carry the path to the leaf: Rust `ValidationError::Nested { field, index }` renders as
`` at `edges`[0]: at `children`[1]: invalid utf-8 ``; TS `ValidationError.path` is
`['edges', 0, 'children', 1]`.

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

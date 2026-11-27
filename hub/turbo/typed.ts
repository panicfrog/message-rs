/**
 * Value-first publishing: `publish(id, 65535)` — the kind is an implementation detail of the
 * wire format, not something callers declare.
 *
 * JS types map by a fixed rule (no value-range guessing, which would silently change the wire
 * format as values change and break cross-language decoding):
 *
 *   boolean    → BOOL
 *   string     → STRING
 *   bigint     → I64
 *   Uint8Array → BYTES
 *   number     → F64   (no precision loss, no width choice to get wrong)
 *
 * For an exact width, wrap the value: `u16(65535)`, `i32(-1)`, `f32(1.5)`, `u64(2n)`, `message(bytes)`.
 * The wrappers carry a codec (`HubCodec`), so `publish(id, u16(65535))` is still one argument.
 * Generated structs take the same idea one step further: `publish(id, user, User.codec)` — see
 * index.ts. Either way the caller never names a `ValueKind`.
 */

import { ValueKind } from './kinds';
import { encode, decode, type DecodedValue } from './value';

/**
 * The one contract every publishable/pullable value conforms to: basic JS types via [basicCodec]
 * below, and any generated struct via its own `codec` (a `MessageCodec<T>` from
 * `@message/runtime`, e.g. `User.codec`). Conforming a new type is providing a `MessageCodec` —
 * nothing here needs to change.
 */
export interface HubCodec<T> {
  readonly kind: number;
  encode(value: T): Uint8Array;
  fromBytes(bytes: Uint8Array): T | null;
}

/**
 * Basic JS types share one codec per kind: the byte layout lives in `encode`/`decode` (already
 * written for the raw wire format); each kind only picks its case, so there is no second copy
 * of the little-endian/UTF-8 rules to drift out of sync.
 */
export function basicCodec<T extends DecodedValue>(kind: number): HubCodec<T> {
  return { kind, encode: (v) => encode(kind, v), fromBytes: (bytes) => decode(kind, bytes) as T };
}

export const boolCodec = basicCodec<boolean>(ValueKind.BOOL);
export const u8Codec = basicCodec<number>(ValueKind.U8);
export const i8Codec = basicCodec<number>(ValueKind.I8);
export const u16Codec = basicCodec<number>(ValueKind.U16);
export const i16Codec = basicCodec<number>(ValueKind.I16);
export const u32Codec = basicCodec<number>(ValueKind.U32);
export const i32Codec = basicCodec<number>(ValueKind.I32);
export const u64Codec = basicCodec<bigint>(ValueKind.U64);
export const i64Codec = basicCodec<bigint>(ValueKind.I64);
export const f32Codec = basicCodec<number>(ValueKind.F32);
export const f64Codec = basicCodec<number>(ValueKind.F64);
export const strCodec = basicCodec<string>(ValueKind.STRING);
export const bytesCodec = basicCodec<Uint8Array>(ValueKind.BYTES);
/** A message-rs payload without a schema on hand; decode it yourself with the generated `decodeBy(buf)`. */
export const messageCodec = basicCodec<Uint8Array>(ValueKind.MESSAGE);

/** Wraps a value in an explicit-width codec: `publish(id, u16(65535))` instead of the f64 default. */
export interface Typed {
  readonly codec: HubCodec<unknown>;
  readonly value: unknown;
}

function typed<T>(codec: HubCodec<T>, value: T): Typed {
  return { codec: codec as HubCodec<unknown>, value };
}

export const bool = (v: boolean) => typed(boolCodec, v);
export const u8 = (v: number) => typed(u8Codec, v);
export const i8 = (v: number) => typed(i8Codec, v);
export const u16 = (v: number) => typed(u16Codec, v);
export const i16 = (v: number) => typed(i16Codec, v);
export const u32 = (v: number) => typed(u32Codec, v);
export const i32 = (v: number) => typed(i32Codec, v);
export const u64 = (v: bigint | number) => typed(u64Codec, BigInt(v));
export const i64 = (v: bigint | number) => typed(i64Codec, BigInt(v));
export const f32 = (v: number) => typed(f32Codec, v);
export const f64 = (v: number) => typed(f64Codec, v);
export const str = (v: string) => typed(strCodec, v);
export const bytes = (v: Uint8Array) => typed(bytesCodec, v);
export const message = (v: Uint8Array) => typed(messageCodec, v);

/** Anything `publish` accepts: a plain JS value or an explicitly typed one. */
export type Publishable = boolean | number | bigint | string | Uint8Array | Typed;

function isTyped(v: Publishable): v is Typed {
  return typeof v === 'object' && v !== null && 'codec' in v && 'value' in v;
}

/**
 * Resolves a plain JS value or an explicit-width wrapper to its wire kind and payload bytes.
 * Generated structs don't go through here: `publish(id, value, codec)` takes their `MessageCodec`
 * directly (see index.ts) — a plain object has no reliable runtime brand to guess a schema from,
 * so the caller names the codec once instead of this function guessing per value.
 */
export function resolve(v: Publishable): { kind: number; bytes: Uint8Array } {
  if (isTyped(v)) return { kind: v.codec.kind, bytes: v.codec.encode(v.value) };
  switch (typeof v) {
    case 'boolean':
      return { kind: ValueKind.BOOL, bytes: encode(ValueKind.BOOL, v) };
    case 'string':
      return { kind: ValueKind.STRING, bytes: encode(ValueKind.STRING, v) };
    case 'bigint':
      return { kind: ValueKind.I64, bytes: encode(ValueKind.I64, v) };
    case 'number':
      return { kind: ValueKind.F64, bytes: encode(ValueKind.F64, v) };
    default:
      if (v instanceof Uint8Array) return { kind: ValueKind.BYTES, bytes: v };
      throw new TypeError(`cannot publish a value of type ${typeof v}; pass its codec: publish(id, value, Xxx.codec)`);
  }
}

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
 * The wrappers carry the kind, so `publish(id, u16(65535))` is still one argument.
 */

import { ValueKind } from './kinds';
import { encode, type DecodedValue } from './value';

/** A value with an explicit wire kind (see [u8], [u16], ... below). */
export interface Typed {
  readonly kind: number;
  readonly value: DecodedValue;
}

function typed(kind: number, value: DecodedValue): Typed {
  return { kind, value };
}

export const bool = (v: boolean) => typed(ValueKind.BOOL, v);
export const u8 = (v: number) => typed(ValueKind.U8, v);
export const i8 = (v: number) => typed(ValueKind.I8, v);
export const u16 = (v: number) => typed(ValueKind.U16, v);
export const i16 = (v: number) => typed(ValueKind.I16, v);
export const u32 = (v: number) => typed(ValueKind.U32, v);
export const i32 = (v: number) => typed(ValueKind.I32, v);
export const u64 = (v: bigint | number) => typed(ValueKind.U64, BigInt(v));
export const i64 = (v: bigint | number) => typed(ValueKind.I64, BigInt(v));
export const f32 = (v: number) => typed(ValueKind.F32, v);
export const f64 = (v: number) => typed(ValueKind.F64, v);
export const str = (v: string) => typed(ValueKind.STRING, v);
export const bytes = (v: Uint8Array) => typed(ValueKind.BYTES, v);
/** A message-rs payload; decode it with the generated schema's `decodeBy(key)`. */
export const message = (v: Uint8Array) => typed(ValueKind.MESSAGE, v);

/** Anything `publish` accepts: a plain JS value or an explicitly typed one. */
export type Publishable = boolean | number | bigint | string | Uint8Array | Typed;

function isTyped(v: Publishable): v is Typed {
  return typeof v === 'object' && v !== null && 'kind' in v && 'value' in v;
}

/** Resolves a value to its wire kind and payload bytes. */
export function resolve(v: Publishable): { kind: number; bytes: Uint8Array } {
  if (isTyped(v)) return { kind: v.kind, bytes: encode(v.kind, v.value) };
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
      throw new TypeError(`cannot publish a value of type ${typeof v}`);
  }
}

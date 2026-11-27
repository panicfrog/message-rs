/**
 * Value decoding for the JS surface — the mirror of `Value.decode` on Swift and Kotlin.
 *
 * The kind is known, so basic types come back as plain JS values: callers never see raw bytes
 * for a bool or a u16. Only `MESSAGE` stays binary, because decoding it needs the generated
 * message schema (`decodeBy(key)`), which the hub deliberately knows nothing about.
 *
 * Byte layouts follow the message-rs wire conventions: little-endian scalars, `bool` one byte,
 * `String` raw UTF-8. 64-bit kinds decode to `bigint` (no precision loss); f32/f64 to `number`.
 */

import { ValueKind, CLOSED_KIND } from './kinds';

/** A decoded hub value. `MESSAGE` payloads stay as bytes for the schema layer. */
export type DecodedValue = boolean | number | bigint | string | Uint8Array;

export class ValueError extends Error {}

function view(bytes: Uint8Array): DataView {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
}

function need(bytes: Uint8Array, n: number, kind: number): void {
  if (bytes.byteLength !== n) {
    throw new ValueError(`kind ${kind} needs ${n} bytes, got ${bytes.byteLength}`);
  }
}

/**
 * Decodes a payload of the given kind. Throws [ValueError] on a wrong length or invalid UTF-8.
 * `CLOSED_KIND` has no payload and decodes to `null` via [decodeEvent] instead.
 */
export function decode(kind: number, bytes: Uint8Array): DecodedValue {
  const dv = view(bytes);
  switch (kind) {
    case ValueKind.BOOL:
      return bytes.byteLength > 0 && bytes[0] !== 0;
    case ValueKind.U8:
      need(bytes, 1, kind);
      return dv.getUint8(0);
    case ValueKind.I8:
      need(bytes, 1, kind);
      return dv.getInt8(0);
    case ValueKind.U16:
      need(bytes, 2, kind);
      return dv.getUint16(0, true);
    case ValueKind.I16:
      need(bytes, 2, kind);
      return dv.getInt16(0, true);
    case ValueKind.U32:
      need(bytes, 4, kind);
      return dv.getUint32(0, true);
    case ValueKind.I32:
      need(bytes, 4, kind);
      return dv.getInt32(0, true);
    case ValueKind.U64:
      need(bytes, 8, kind);
      return dv.getBigUint64(0, true);
    case ValueKind.I64:
      need(bytes, 8, kind);
      return dv.getBigInt64(0, true);
    case ValueKind.F32:
      need(bytes, 4, kind);
      return dv.getFloat32(0, true);
    case ValueKind.F64:
      need(bytes, 8, kind);
      return dv.getFloat64(0, true);
    case ValueKind.STRING:
      try {
        return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
      } catch {
        throw new ValueError('string payload is not valid utf-8');
      }
    case ValueKind.BYTES:
    case ValueKind.MESSAGE:
      return bytes;
    default:
      throw new ValueError(`unknown kind ${kind}`);
  }
}

/** Encodes a value into the hub's byte layout (the inverse of [decode]). */
export function encode(kind: number, value: DecodedValue): Uint8Array {
  const buf = (n: number) => new Uint8Array(n);
  switch (kind) {
    case ValueKind.BOOL:
      return Uint8Array.of(value ? 1 : 0);
    case ValueKind.U8:
    case ValueKind.I8: {
      const b = buf(1);
      view(b).setUint8(0, Number(value));
      return b;
    }
    case ValueKind.U16:
    case ValueKind.I16: {
      const b = buf(2);
      const dv = view(b);
      if (kind === ValueKind.U16) dv.setUint16(0, Number(value), true);
      else dv.setInt16(0, Number(value), true);
      return b;
    }
    case ValueKind.U32:
    case ValueKind.I32: {
      const b = buf(4);
      const dv = view(b);
      if (kind === ValueKind.U32) dv.setUint32(0, Number(value), true);
      else dv.setInt32(0, Number(value), true);
      return b;
    }
    case ValueKind.U64:
    case ValueKind.I64: {
      const b = buf(8);
      const dv = view(b);
      const v = BigInt(value as bigint | number);
      if (kind === ValueKind.U64) dv.setBigUint64(0, v, true);
      else dv.setBigInt64(0, v, true);
      return b;
    }
    case ValueKind.F32: {
      const b = buf(4);
      view(b).setFloat32(0, Number(value), true);
      return b;
    }
    case ValueKind.F64: {
      const b = buf(8);
      view(b).setFloat64(0, Number(value), true);
      return b;
    }
    case ValueKind.STRING:
      return new TextEncoder().encode(String(value));
    case ValueKind.BYTES:
    case ValueKind.MESSAGE:
      return value as Uint8Array;
    default:
      throw new ValueError(`unknown kind ${kind}`);
  }
}

export { ValueKind, CLOSED_KIND };

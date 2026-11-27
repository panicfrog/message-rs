import { describe, expect, it } from 'vitest';
import { decode, encode, ValueKind, ValueError } from '../value';

// The byte layouts must match the Rust core (and therefore Swift/Kotlin): little-endian
// scalars, bool one byte, String raw UTF-8. These vectors are the same ones the Swift and
// Kotlin suites assert on.
describe('value codec', () => {
  it('decodes basic kinds to plain JS values', () => {
    expect(decode(ValueKind.BOOL, Uint8Array.of(1))).toBe(true);
    expect(decode(ValueKind.BOOL, Uint8Array.of(0))).toBe(false);
    expect(decode(ValueKind.U8, Uint8Array.of(200))).toBe(200);
    expect(decode(ValueKind.I8, Uint8Array.of(0xff))).toBe(-1);
    expect(decode(ValueKind.U16, Uint8Array.of(0x02, 0x01))).toBe(0x0102);
    expect(decode(ValueKind.I16, Uint8Array.of(0xfe, 0xff))).toBe(-2);
    expect(decode(ValueKind.U32, Uint8Array.of(1, 0, 0, 0))).toBe(1);
    expect(decode(ValueKind.I32, Uint8Array.of(0xff, 0xff, 0xff, 0xff))).toBe(-1);
    expect(decode(ValueKind.F32, Uint8Array.of(0, 0, 0x80, 0x3f))).toBe(1);
    expect(decode(ValueKind.STRING, new TextEncoder().encode('山'))).toBe('山');
  });

  it('keeps 64-bit precision with bigint', () => {
    const max = Uint8Array.of(0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff);
    expect(decode(ValueKind.U64, max)).toBe(18446744073709551615n);
    expect(decode(ValueKind.I64, max)).toBe(-1n);
  });

  it('leaves BYTES and MESSAGE binary (the schema decodes MESSAGE)', () => {
    const b = Uint8Array.of(1, 2, 3);
    expect(decode(ValueKind.BYTES, b)).toEqual(b);
    expect(decode(ValueKind.MESSAGE, b)).toEqual(b);
  });

  it('round-trips every kind through encode', () => {
    const cases: [number, unknown][] = [
      [ValueKind.BOOL, true],
      [ValueKind.U8, 200],
      [ValueKind.I8, -1],
      [ValueKind.U16, 65535],
      [ValueKind.I16, -32768],
      [ValueKind.U32, 4294967295],
      [ValueKind.I32, -2147483648],
      [ValueKind.U64, 18446744073709551615n],
      [ValueKind.I64, -9223372036854775808n],
      [ValueKind.F32, 1.5],
      [ValueKind.F64, -0.5],
      [ValueKind.STRING, 'hello 世界'],
    ];
    for (const [kind, value] of cases) {
      expect(decode(kind, encode(kind, value as never))).toEqual(value);
    }
  });

  it('rejects wrong lengths and invalid utf-8', () => {
    expect(() => decode(ValueKind.U32, Uint8Array.of(1, 2))).toThrow(ValueError);
    expect(() => decode(ValueKind.STRING, Uint8Array.of(0xff))).toThrow(ValueError);
    expect(() => decode(99, Uint8Array.of(0))).toThrow(ValueError);
  });

  it('matches the bytes the other surfaces produce', () => {
    // same assertions as HubTests.swift / HubJvmTest.kt
    expect(Array.from(encode(ValueKind.BOOL, true))).toEqual([1]);
    expect(Array.from(encode(ValueKind.U16, 0x0102))).toEqual([2, 1]);
    expect(Array.from(encode(ValueKind.I16, -2))).toEqual([0xfe, 0xff]);
    expect(Array.from(encode(ValueKind.U32, 1))).toEqual([1, 0, 0, 0]);
    expect(Array.from(encode(ValueKind.F32, 1.0))).toEqual([0, 0, 0x80, 0x3f]);
    expect(Array.from(encode(ValueKind.STRING, '山'))).toEqual(Array.from(new TextEncoder().encode('山')));
  });
});

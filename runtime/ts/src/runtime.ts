/**
 * message-rs TypeScript runtime (zero-dependency; works on RN (Hermes), Web and Node).
 *
 * Layout rules follow the repo SPEC.md: little-endian payload; slot `{ offset: u32, len: u32 }`
 * offsets are payload-relative; extension blocks are
 * `[ bitmap | fixed fields | var data | since:u16 | block_len:u32 ]` with the trailer at the
 * tail, located by walking back from the payload end.
 */

// ---- Fixed-size little-endian reads (dv is the payload view; offsets are dv-relative) ----

export function readU8(dv: DataView, o: number): number {
  return dv.getUint8(o);
}
export function readI8(dv: DataView, o: number): number {
  return dv.getInt8(o);
}
export function readBool(dv: DataView, o: number): boolean {
  return dv.getUint8(o) !== 0;
}
export function readU16(dv: DataView, o: number): number {
  return dv.getUint16(o, true);
}
export function readI16(dv: DataView, o: number): number {
  return dv.getInt16(o, true);
}
export function readU32(dv: DataView, o: number): number {
  return dv.getUint32(o, true);
}
export function readI32(dv: DataView, o: number): number {
  return dv.getInt32(o, true);
}
export function readU64(dv: DataView, o: number): number {
  return Number(dv.getBigUint64(o, true));
}
export function readI64(dv: DataView, o: number): number {
  return Number(dv.getBigInt64(o, true));
}
/** Exact u64 read (for #[msg(precise)] fields). */
export function readU64Big(dv: DataView, o: number): bigint {
  return dv.getBigUint64(o, true);
}
export function readF32(dv: DataView, o: number): number {
  return dv.getFloat32(o, true);
}
export function readF64(dv: DataView, o: number): number {
  return dv.getFloat64(o, true);
}

export function readSlot(dv: DataView, o: number): [number, number] {
  return [dv.getUint32(o, true), dv.getUint32(o + 4, true)];
}

export function readBitmapBit(dv: DataView, blockStart: number, bit: number): boolean {
  return ((dv.getUint8(blockStart + (bit >> 3)) >> (bit & 7)) & 1) === 1;
}

// ---- Slots and strings ----

const TD = new TextDecoder();
const TE = new TextEncoder();

export function slotBytes(dv: DataView, off: number, len: number): Uint8Array {
  return new Uint8Array(dv.buffer, dv.byteOffset + off, len);
}

export function utf8(dv: DataView, off: number, len: number): string {
  if (len === 0) return '';
  return TD.decode(slotBytes(dv, off, len));
}

export function utf8Encode(s: string): Uint8Array {
  return TE.encode(s);
}

/**
 * Locate an extension block by walking back from the payload end; returns its payload start
 * (trailer excluded). Blocks newer than `target` are stepped over even when this reader does not
 * know them (an older reader must still find its own blocks behind a newer writer's). `minLen` is
 * the block's fixed region: a block too short to hold its fields counts as absent.
 */
export function findExtBlock(
  dv: DataView,
  versions: readonly number[],
  hv: number,
  target: number,
  minLen = 0,
): number | null {
  let pos = dv.byteLength;
  const maxKnown = versions.length ? versions[versions.length - 1] : 0;
  for (;;) {
    if (pos < 6) return null;
    const since = dv.getUint16(pos - 6, true);
    const blen = dv.getUint32(pos - 4, true);
    if (since < 2 || since > hv || blen < 6 || blen > pos) return null;
    const start = pos - blen;
    if (since === target) return pos - 6 - start >= minLen ? start : null;
    if (since > target && (since > maxKnown || versions.includes(since))) {
      pos = start;
      continue;
    }
    return null;
  }
}

// ---- Lazy views ----

export class ScalarListView<T = number> {
  constructor(
    private readonly dv: DataView,
    private readonly start: number,
    private readonly count: number,
    private readonly stride: number,
    private readonly rd: (dv: DataView, o: number) => T,
  ) {}
  get length(): number {
    return this.count;
  }
  get(i: number): T {
    if (i >= this.count) throw new Error(`index ${i} out of bounds (len ${this.count})`);
    return this.rd(this.dv, this.start + i * this.stride);
  }
}

export class StringListView {
  constructor(
    private readonly dv: DataView,
    private readonly start: number,
    private readonly count: number,
  ) {}
  get length(): number {
    return this.count;
  }
  get(i: number): string {
    if (i >= this.count) throw new Error(`index ${i} out of bounds (len ${this.count})`);
    const [o, l] = readSlot(this.dv, this.start + i * 8);
    return utf8(this.dv, o, l);
  }
}

export class BytesListView {
  constructor(
    private readonly dv: DataView,
    private readonly start: number,
    private readonly count: number,
  ) {}
  get length(): number {
    return this.count;
  }
  get(i: number): Uint8Array {
    if (i >= this.count) throw new Error(`index ${i} out of bounds (len ${this.count})`);
    const [o, l] = readSlot(this.dv, this.start + i * 8);
    return slotBytes(this.dv, o, l);
  }
}

/** Raw slot view for nested arrays: each element contributes an 8-byte slot. */
export class SlotListView {
  constructor(
    private readonly dv: DataView,
    private readonly start: number,
    private readonly count: number,
  ) {}
  get length(): number {
    return this.count;
  }
  slotOf(i: number): [number, number] {
    if (i >= this.count) throw new Error(`index ${i} out of bounds (len ${this.count})`);
    return readSlot(this.dv, this.start + i * 8);
  }
}

export class StructListView<T> {
  constructor(
    private readonly dv: DataView,
    private readonly start: number,
    private readonly count: number,
    private readonly stride: number,
    private readonly mk: (dv: DataView, o: number, hv: number) => T,
  ) {}
  get length(): number {
    return this.count;
  }
  get(i: number): T {
    if (i >= this.count) throw new Error(`index ${i} out of bounds (len ${this.count})`);
    return this.mk(this.dv, this.start + i * this.stride, 0);
  }
}

// ---- IR model and the generic encoder ----
// Shape-compatible with message-snapshot.json. The write path is IR-driven (layout comes from the IR and is never recomputed).

export type TypeIr =
  | 'bool' | 'u8' | 'i8' | 'u16' | 'i16' | 'u32' | 'i32' | 'u64' | 'i64' | 'f32' | 'f64'
  | 'string' | 'bytes'
  | { enum: string }
  | { struct: string }
  | { array: { elem: TypeIr; fixed?: number | null } }
  | { map: { key: TypeIr; value: TypeIr } };

export interface FieldIr {
  name: string;
  type: TypeIr;
  optional?: boolean;
  since: number;
  precise?: boolean;
  reserved?: boolean;
  doc?: string;
  offset: number;
  size: number;
  optionalBit?: number;
  slot?: boolean;
  elementSize?: number;
  mapValueOffset?: number;
}

export interface MsgIr {
  key: number;
  name: string;
  doc?: string;
  fields: FieldIr[];
  layout: {
    bitmapLen: number;
    fixedLen: number;
    maxAlign: number;
    extBlocks?: { version: number; bitmapLen: number; fixedLen: number }[];
  };
}

export interface SchemaIr {
  messages: MsgIr[];
  enums?: { name: string; doc?: string; variants: { name: string; value: number; doc?: string }[] }[];
}

const HEADER_LEN = 8;

class Enc {
  data: number[] = [];
  /** Append bytes, returning the payload-relative offset. */
  append(bytes: ArrayLike<number>): number {
    const off = this.data.length - HEADER_LEN;
    for (let i = 0; i < bytes.length; i++) this.data.push(bytes[i]);
    return off;
  }
  /** Reserve a zeroed region, returning the absolute start. */
  reserve(n: number): number {
    const start = this.data.length;
    for (let i = 0; i < n; i++) this.data.push(0);
    return start;
  }
  payloadRel(abs: number): number {
    return abs - HEADER_LEN;
  }
  setBit(blockStart: number, bit: number): void {
    this.data[blockStart + (bit >> 3)] |= 1 << (bit & 7);
  }
  u8(pos: number, v: number): void {
    this.data[pos] = v & 0xff;
  }
  u16(pos: number, v: number): void {
    this.data[pos] = v & 0xff;
    this.data[pos + 1] = (v >>> 8) & 0xff;
  }
  u32(pos: number, v: number): void {
    this.data[pos] = v & 0xff;
    this.data[pos + 1] = (v >>> 8) & 0xff;
    this.data[pos + 2] = (v >>> 16) & 0xff;
    this.data[pos + 3] = (v >>> 24) & 0xff;
  }
  u64(pos: number, v: number | bigint | string): void {
    let b: bigint;
    if (typeof v === 'bigint') b = v;
    else if (typeof v === 'string') b = BigInt(v);
    else b = BigInt(Math.round(v));
    for (let i = 0; i < 8; i++) {
      this.data[pos + i] = Number(b & 0xffn);
      b >>= 8n;
    }
  }
  patchSlot(pos: number, off: number, len: number): void {
    this.u32(pos, off);
    this.u32(pos + 4, len);
  }
  finish(): Uint8Array {
    return new Uint8Array(this.data);
  }
}

function scalarWidth(t: string): number {
  switch (t) {
    case 'bool': case 'u8': case 'i8': return 1;
    case 'u16': case 'i16': return 2;
    case 'u32': case 'i32': case 'f32': case 'u64': case 'i64': case 'f64': return 4 * ((t === 'u64' || t === 'i64' || t === 'f64') ? 2 : 1);
    default: return 4;
  }
}

function writeScalar(enc: Enc, t: string, pos: number, v: unknown): void {
  switch (t) {
    case 'bool': enc.u8(pos, v ? 1 : 0); return;
    case 'u8': enc.u8(pos, v as number); return;
    case 'i8': enc.u8(pos, (v as number) & 0xff); return;
    case 'u16': enc.u16(pos, v as number); return;
    case 'i16': enc.u16(pos, (v as number) & 0xffff); return;
    case 'u32': enc.u32(pos, v as number); return;
    case 'i32': enc.u32(pos, (v as number) | 0); return;
    case 'u64': case 'i64': enc.u64(pos, v as number); return;
    case 'f32': {
      const b = new DataView(new ArrayBuffer(4));
      b.setFloat32(0, v as number, true);
      for (let i = 0; i < 4; i++) enc.u8(pos + i, b.getUint8(i));
      return;
    }
    case 'f64': {
      const b = new DataView(new ArrayBuffer(8));
      b.setFloat64(0, v as number, true);
      for (let i = 0; i < 8; i++) enc.u8(pos + i, b.getUint8(i));
      return;
    }
  }
}

function pick(value: Record<string, unknown>, name: string): unknown {
  if (value[name] !== undefined) return value[name];
  const camel = name.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());
  return value[camel];
}

/** Write one version block's fixed fields in declaration order and append var data (DFS); the blockAbs region is pre-reserved. */
function writeBlock(
  enc: Enc,
  schema: SchemaIr,
  fields: FieldIr[],
  blockAbs: number,
  value: Record<string, unknown>,
): void {
  for (const f of fields) {
    if (f.reserved) continue; // deleted from the API: the pre-zeroed region is left as-is
    let v = pick(value, f.name);
    if (f.optional) {
      if (v === null || v === undefined) continue;
      enc.setBit(blockAbs, f.optionalBit ?? 0);
    }
    writeValue(enc, schema, f, blockAbs + f.offset, v);
  }
}

function writeValue(enc: Enc, schema: SchemaIr, f: FieldIr, pos: number, v: unknown): void {
  const t = f.type;
  if (typeof t === 'string') {
    if (t === 'string') {
      const bytes = utf8Encode(String(v));
      const off = enc.append(bytes);
      enc.patchSlot(pos, off, bytes.length);
    } else if (t === 'bytes') {
      const bytes = v as ArrayLike<number>;
      const off = enc.append(bytes);
      enc.patchSlot(pos, off, bytes.length);
    } else {
      writeScalar(enc, t, pos, v);
    }
    return;
  }
  if ('enum' in t) {
    const num = typeof v === 'string' ? enumValue(schema, t.enum, v) : (v as number);
    writeScalar(enc, 'i32', pos, num);
    return;
  }
  if ('struct' in t) {
    const sub = schema.messages.find((m) => m.name === t.struct);
    if (!sub) throw new Error(`unknown struct ${t.struct}`);
    // V1: referenced structs are guaranteed base-only
    writeBlock(enc, schema, sub.fields.filter((x) => x.since === 1), pos, v as Record<string, unknown>);
    return;
  }
  if ('array' in t) {
    const { elem, fixed } = t.array;
    writeArraySlot(enc, schema, elem, fixed, f.elementSize ?? 8, pos, v);
    return;
  }
  if ('map' in t) {
    const obj = (v as Record<string, unknown>) ?? {};
    const kt = t.map.key;
    // Resolve keys to their wire form first: the canonical order is by wire value (Rust sorts
    // entries by key), so enum keys must order by value, not by variant spelling.
    const entries = Object.keys(obj).map((k) => {
      let kv = mapKeyNum(schema, kt, k);
      if (typeof kt !== 'string' && 'enum' in kt && typeof kv === 'string') kv = enumValue(schema, kt.enum, kv);
      return { kv, v: obj[k] };
    });
    entries.sort((a, b) => compareKeys(a.kv, b.kv));
    const n = entries.length;
    const stride = f.elementSize ?? 8;
    const vOff = f.mapValueOffset ?? 0;
    const region = enc.reserve(n * stride);
    for (let i = 0; i < n; i++) {
      const e = region + i * stride;
      writeElemInline(enc, schema, kt, e, entries[i].kv);
      const vt = t.map.value;
      if (typeof vt !== 'string' && 'array' in vt) {
        writeArraySlot(enc, schema, vt.array.elem, vt.array.fixed, elemWidth(schema, vt.array.elem), e + vOff, entries[i].v);
      } else {
        writeElemInline(enc, schema, vt, e + vOff, entries[i].v);
      }
    }
    enc.patchSlot(pos, enc.payloadRel(region), n);
    return;
  }
}

function compareKeys(a: string | number | bigint, b: string | number | bigint): number {
  if (typeof a === 'string' || typeof b === 'string') {
    const sa = String(a);
    const sb = String(b);
    return sa < sb ? -1 : sa > sb ? 1 : 0;
  }
  if (typeof a === 'bigint' || typeof b === 'bigint') {
    const x = typeof a === 'bigint' ? a : BigInt(a);
    const y = typeof b === 'bigint' ? b : BigInt(b);
    return x < y ? -1 : x > y ? 1 : 0;
  }
  return (a as number) - (b as number);
}

function elemWidth(schema: SchemaIr, elem: TypeIr): number {
  if (typeof elem === 'string') return (elem === 'string' || elem === 'bytes') ? 8 : scalarWidth(elem);
  if ('enum' in elem) return 4;
  if ('struct' in elem) return schema.messages.find((m) => m.name === elem.struct)?.layout.fixedLen ?? 0;
  return 8; // slot-shaped element
}

/** Write an array at `pos`: an inline fixed region, or a slot pointing at a reserved element region. `stride` is the element footprint. */
function writeArraySlot(
  enc: Enc,
  schema: SchemaIr,
  elem: TypeIr,
  fixed: number | null | undefined,
  stride: number,
  pos: number,
  v: unknown,
): void {
  const arr = (v as unknown[]) ?? [];
  const elemIsSlot = typeof elem === 'string' && (elem === 'string' || elem === 'bytes')
    || (typeof elem !== 'string' && ('array' in elem || 'map' in elem));
  if (fixed != null && !elemIsSlot) {
    // inline fixed-length array
    for (let i = 0; i < arr.length && i < fixed; i++) {
      writeElemInline(enc, schema, elem, pos + i * elemWidth(schema, elem), arr[i]);
    }
    return;
  }
  const n = arr.length;
  const region = enc.reserve(n * stride);
  writeElems(enc, schema, elem, region, stride, arr);
  enc.patchSlot(pos, enc.payloadRel(region), n);
}

function writeElemInline(enc: Enc, schema: SchemaIr, elem: TypeIr, pos: number, v: unknown): void {
  if (typeof elem === 'string') {
    if (elem === 'string') {
      const bytes = utf8Encode(String(v));
      const off = enc.append(bytes);
      enc.patchSlot(pos, off, bytes.length);
    } else if (elem === 'bytes') {
      const bytes = v as ArrayLike<number>;
      const off = enc.append(bytes);
      enc.patchSlot(pos, off, bytes.length);
    } else {
      writeScalar(enc, elem, pos, v);
    }
  } else if ('enum' in elem) {
    const num = typeof v === 'string' ? enumValue(schema, elem.enum, v) : (v as number);
    writeScalar(enc, 'i32', pos, num);
  } else if ('struct' in elem) {
    const sub = schema.messages.find((m) => m.name === elem.struct);
    if (!sub) throw new Error(`unknown struct ${elem.struct}`);
    writeBlock(enc, schema, sub.fields.filter((x) => x.since === 1), pos, v as Record<string, unknown>);
  } else {
    throw new Error('slot-shaped elements cannot be written inline');
  }
}

/** Write the array data region: element footprints are contiguous, var data appended in DFS. */
function writeElems(
  enc: Enc,
  schema: SchemaIr,
  elem: TypeIr,
  region: number,
  stride: number,
  arr: unknown[],
): void {
  for (let i = 0; i < arr.length; i++) {
    const v = arr[i];
    if (typeof elem === 'string') {
      if (elem === 'string') {
        const bytes = utf8Encode(String(v));
        const off = enc.append(bytes);
        enc.patchSlot(region + i * stride, off, bytes.length);
      } else if (elem === 'bytes') {
        const bytes = v as ArrayLike<number>;
        const off = enc.append(bytes);
        enc.patchSlot(region + i * stride, off, bytes.length);
      } else {
        writeScalar(enc, elem, region + i * stride, v);
      }
    } else if ('enum' in elem) {
      const num = typeof v === 'string' ? enumValue(schema, elem.enum, v) : (v as number);
      writeScalar(enc, 'i32', region + i * stride, num);
    } else if ('struct' in elem) {
      writeElemInline(enc, schema, elem, region + i * stride, v);
    } else if ('array' in elem) {
      // nested dynamic array: the element contributes an 8-byte slot pointing at the inner region
      writeArraySlot(enc, schema, elem.array.elem, elem.array.fixed, elemWidth(schema, elem.array.elem), region + i * 8, v);
    } else {
      throw new Error('maps cannot be array elements');
    }
  }
}

function enumValue(schema: SchemaIr, name: string, variant: string): number {
  const e = schema.enums?.find((x) => x.name === name);
  const v = e?.variants.find((x) => x.name === variant);
  if (!v) throw new Error(`unknown variant ${name}::${variant}`);
  return v.value;
}

function mapKeyNum(schema: SchemaIr, key: TypeIr, s: string): string | number | bigint {
  if (typeof key === 'string') {
    if (key === 'u64' || key === 'i64') return BigInt(s);
    if (key === 'string') return s;
    return Number(s);
  }
  if ('enum' in key) {
    // enum keys are the numeric value as a string (what decode produces); a variant name is also accepted
    const n = Number(s);
    return Number.isNaN(n) ? s : n;
  }
  return s;
}

/** Canonical encoding: byte-identical to Rust (declaration-order DFS var data, keys sorted). */
export function encodeMessage(schema: SchemaIr, msgName: string, value: Record<string, unknown>): Uint8Array {
  const msg = schema.messages.find((m) => m.name === msgName);
  if (!msg) throw new Error(`unknown message ${msgName}`);
  const enc = new Enc();
  enc.reserve(HEADER_LEN);
  const baseAbs = enc.reserve(msg.layout.fixedLen);
  writeBlock(enc, schema, msg.fields.filter((f) => f.since === 1), baseAbs, value);
  const versions = (msg.layout.extBlocks ?? []).map((b) => b.version);
  for (const v of versions) {
    const blk = enc.reserve((msg.layout.extBlocks ?? []).find((b) => b.version === v)!.fixedLen);
    writeBlock(enc, schema, msg.fields.filter((f) => f.since === v), blk, value);
    const endPayload = enc.data.length - HEADER_LEN;
    const blen = endPayload - enc.payloadRel(blk) + 6;
    const trailer = enc.reserve(6);
    enc.u16(trailer, v);
    enc.u32(trailer + 2, blen);
  }
  enc.u32(0, msg.key);
  enc.u16(4, msg.layout.fixedLen);
  enc.u8(6, versions.length > 0 ? versions[versions.length - 1] : 0);
  enc.u8(7, 0);
  return enc.finish();
}

// ---- Strict validation ----

/** Field names and element indices from the message root down to the leaf, e.g. `['edges', 0, 'children']`. */
export type ValidationPath = (string | number)[];

export class ValidationError extends Error {
  readonly path: ValidationPath;
  constructor(readonly reason: string, path: ValidationPath = []) {
    super(path.length ? `at ${path.map(String).join('.')}: ${reason}` : reason);
    this.name = 'ValidationError';
    this.path = path;
  }
}

const TD_STRICT = new TextDecoder('utf-8', { fatal: true });

function vSlot(dv: DataView, pos: number, path: ValidationPath): [number, number] {
  const off = dv.getUint32(pos, true);
  const len = dv.getUint32(pos + 4, true);
  if (off + len > dv.byteLength) {
    throw new ValidationError(`slot out of bounds (off=${off} len=${len}, payload=${dv.byteLength})`, path);
  }
  return [off, len];
}

function vStr(dv: DataView, pos: number, path: ValidationPath): void {
  const [o, l] = vSlot(dv, pos, path);
  try {
    TD_STRICT.decode(slotBytes(dv, o, l));
  } catch {
    throw new ValidationError('invalid utf-8', path);
  }
}

/** Slot whose length is an element count; returns (offset, count). */
function vRegion(dv: DataView, pos: number, stride: number, path: ValidationPath): [number, number] {
  const off = dv.getUint32(pos, true);
  const count = dv.getUint32(pos + 4, true);
  if (off + count * stride > dv.byteLength) {
    throw new ValidationError(`slot out of bounds (off=${off} len=${count}, payload=${dv.byteLength})`, path);
  }
  return [off, count];
}

function vEnum(schema: SchemaIr, name: string, dv: DataView, pos: number, path: ValidationPath): void {
  const v = dv.getInt32(pos, true);
  const e = schema.enums?.find((x) => x.name === name);
  if (!e || !e.variants.some((x) => x.value === v)) {
    throw new ValidationError(`unknown ${name} value ${v}`, path);
  }
}

/** Stride / value offset of the composite being descended into. */
interface VCtx {
  stride: number;
  valueOffset: number;
}

function validateFields(schema: SchemaIr, fields: FieldIr[], dv: DataView, origin: number, hv: number, path: ValidationPath): void {
  for (const f of fields) {
    if (f.reserved) continue;
    if (f.optional && !readBitmapBit(dv, origin, f.optionalBit ?? 0)) continue;
    validateValue(schema, f.type, dv, origin + f.offset, hv, [...path, f.name], {
      stride: f.elementSize ?? 8,
      valueOffset: f.mapValueOffset ?? 0,
    });
  }
}

function validateValue(schema: SchemaIr, type: TypeIr, dv: DataView, pos: number, hv: number, path: ValidationPath, ctx: VCtx): void {
  if (typeof type === 'string') {
    if (type === 'string') vStr(dv, pos, path);
    else if (type === 'bytes') vSlot(dv, pos, path);
    return;
  }
  if ('enum' in type) {
    vEnum(schema, type.enum, dv, pos, path);
    return;
  }
  if ('struct' in type) {
    const sub = schema.messages.find((m) => m.name === type.struct);
    if (!sub) throw new ValidationError(`unknown struct ${type.struct}`, path);
    validateFields(schema, sub.fields.filter((x) => x.since === 1), dv, pos, hv, path);
    return;
  }
  if ('array' in type) {
    const { elem, fixed } = type.array;
    const slotShaped = (typeof elem === 'string' && (elem === 'string' || elem === 'bytes'))
      || (typeof elem !== 'string' && ('array' in elem || 'map' in elem));
    const w = elemWidth(schema, elem);
    if (fixed != null && !slotShaped) {
      for (let i = 0; i < fixed; i++) validateElem(schema, elem, dv, pos + i * w, hv, [...path, i]);
      return;
    }
    const [off, count] = vRegion(dv, pos, ctx.stride, path);
    for (let i = 0; i < count; i++) validateElem(schema, elem, dv, off + i * ctx.stride, hv, [...path, i]);
    return;
  }
  if ('map' in type) {
    const [off, count] = vRegion(dv, pos, ctx.stride, path);
    for (let i = 0; i < count; i++) {
      const e = off + i * ctx.stride;
      const p = [...path, i];
      const kt = type.map.key;
      if (kt === 'string') vStr(dv, e, p);
      else if (typeof kt !== 'string' && 'enum' in kt) vEnum(schema, kt.enum, dv, e, p);
      const vt = type.map.value;
      validateValue(schema, vt, dv, e + ctx.valueOffset, hv, p, { stride: elemWidth(schema, vt), valueOffset: 0 });
    }
  }
}

function validateElem(schema: SchemaIr, elem: TypeIr, dv: DataView, pos: number, hv: number, path: ValidationPath): void {
  validateValue(schema, elem, dv, pos, hv, path, { stride: elemWidth(schema, elem), valueOffset: 0 });
}

/**
 * Strictly check an untrusted buffer (header, every slot range, utf-8, enum values, the extension
 * chain). Returns null when valid; after that no getter / view access can throw. Reserved fields
 * are not part of the API and are not checked.
 */
export function validateMessage(schema: SchemaIr, msgName: string, buf: Uint8Array): ValidationError | null {
  const msg = schema.messages.find((m) => m.name === msgName);
  if (!msg) throw new Error(`unknown message ${msgName}`);
  try {
    const need = HEADER_LEN + msg.layout.fixedLen;
    if (buf.length < need) throw new ValidationError(`buffer too short: need ${need} bytes, got ${buf.length}`);
    const full = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    const key = full.getUint32(0, true);
    if (key !== msg.key) throw new ValidationError(`key mismatch: expected ${msg.key}, got ${key}`);
    const fl = full.getUint16(4, true);
    if (fl !== msg.layout.fixedLen) throw new ValidationError(`header fixed_len mismatch: expected ${msg.layout.fixedLen}, got ${fl}`);
    const hv = full.getUint8(6);
    const dv = new DataView(buf.buffer, buf.byteOffset + HEADER_LEN, buf.byteLength - HEADER_LEN);
    validateFields(schema, msg.fields.filter((f) => f.since === 1), dv, 0, hv, []);
    const blocks = msg.layout.extBlocks ?? [];
    const versions = blocks.map((b) => b.version);
    // Highest version first: a corrupt tail is reported at the block that owns it.
    for (let i = blocks.length - 1; i >= 0; i--) {
      if (blocks[i].version > hv) continue;
      const bs = findExtBlock(dv, versions, hv, blocks[i].version, blocks[i].fixedLen);
      if (bs === null) throw new ValidationError(`extension block v${blocks[i].version} missing or malformed`);
      validateFields(schema, msg.fields.filter((f) => f.since === blocks[i].version), dv, bs, hv, []);
    }
    return null;
  } catch (e) {
    if (e instanceof ValidationError) return e;
    throw e;
  }
}

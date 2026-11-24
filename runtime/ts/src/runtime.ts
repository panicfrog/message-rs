/**
 * message-rs TypeScript runtime (zero-dependency; works on RN (Hermes), Web and Node).
 *
 * Layout rules follow the repo SPEC.md: little-endian payload; slot `{ offset: u32, len: u32 }`
 * offsets are payload-relative; extension blocks are
 * `[ bitmap | fixed fields | var data | since:u16 | block_len:u32 ]` with the trailer at the
 * tail, located by walking back from the payload end.
 *
 * The schema is compiled into schema.ts (readers, encoders and validators with literal offsets);
 * nothing in here knows about a specific schema.
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

/** High bit of an array/map slot's length: elements are self-contained instances behind slots. */
export const SHAPE_BIT = 0x80000000;
/** Count bits of an array/map slot's length. */
export const LEN_MASK = 0x7fffffff;
/** Header version passed to instance readers: no extension version is promised inside an instance. */
export const INSTANCE_HV = 255;

/**
 * Locate an extension block by walking back from `hi` (the payload end for top-level messages,
 * the instance end for self-contained struct instances), never crossing below `lo`; returns the
 * block's payload start (trailer excluded). Blocks newer than `target` are stepped over even when
 * this reader does not know them (an older reader must still find its own blocks behind a newer
 * writer's). `minLen` is the block's fixed region: a block too short to hold its fields counts as
 * absent. Instance readers pass `hv = INSTANCE_HV`.
 */
export function findExtBlockIn(
  dv: DataView,
  versions: readonly number[],
  hv: number,
  target: number,
  minLen: number,
  lo: number,
  hi: number,
): number | null {
  let pos = hi;
  const maxKnown = versions.length ? versions[versions.length - 1] : 0;
  for (;;) {
    if (pos < lo + 6) return null;
    const since = dv.getUint16(pos - 6, true);
    const blen = dv.getUint32(pos - 4, true);
    if (since < 2 || since > hv || blen < 6 || blen > pos - lo) return null;
    const start = pos - blen;
    if (since === target) return pos - 6 - start >= minLen ? start : null;
    if (since > target && (since > maxKnown || versions.includes(since))) {
      pos = start;
      continue;
    }
    return null;
  }
}

/** Top-level variant: the walk spans the whole payload. */
export function findExtBlock(
  dv: DataView,
  versions: readonly number[],
  hv: number,
  target: number,
  minLen = 0,
): number | null {
  return findExtBlockIn(dv, versions, hv, target, minLen, 0, dv.byteLength);
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

/** Fixed-stride elements read by `mk` (enum arrays). */
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

/**
 * Struct elements of an array region whose slot length carries the shape bit. shape=0: open
 * fixed-region footprints of `fixedLen` bytes (readers get the payload as their extent and hv=0,
 * so extension fields are absent); shape=1: 8-byte slots, each pointing at a self-contained
 * instance whose `[lo, hi)` extent bounds the reader (hv = INSTANCE_HV).
 */
export class StructEvoListView<T> {
  constructor(
    private readonly dv: DataView,
    private readonly start: number,
    private readonly rawLen: number,
    private readonly fixedLen: number,
    private readonly mk: (dv: DataView, origin: number, end: number, hv: number) => T,
  ) {}
  get length(): number {
    return this.rawLen & LEN_MASK;
  }
  /** Whether the writer emitted per-element instances (its element type had extension fields). */
  get isInstanceMode(): boolean {
    return (this.rawLen & SHAPE_BIT) !== 0;
  }
  get(i: number): T {
    if (i >= this.length) throw new Error(`index ${i} out of bounds (len ${this.length})`);
    if (this.isInstanceMode) {
      const [o, l] = readSlot(this.dv, this.start + i * 8);
      return this.mk(this.dv, o, o + l, INSTANCE_HV);
    }
    return this.mk(this.dv, this.start + i * this.fixedLen, this.dv.byteLength, 0);
  }
}

// ---- Encoder ----

/**
 * Growable little-endian byte buffer. Positions are absolute (the 8-byte header included);
 * `rel()` turns them into the payload-relative values slots carry.
 */
export class Writer {
  private buf: Uint8Array;
  private len = 0;
  private readonly scratch = new DataView(new ArrayBuffer(8));

  constructor(capacity = 64) {
    this.buf = new Uint8Array(Math.max(8, capacity));
  }

  private ensure(extra: number): void {
    if (this.len + extra > this.buf.length) {
      let cap = this.buf.length * 2;
      while (cap < this.len + extra) cap *= 2;
      const next = new Uint8Array(cap);
      next.set(this.buf.subarray(0, this.len));
      this.buf = next;
    }
  }

  /** Reserve `n` zeroed bytes, returning the absolute start. */
  reserve(n: number): number {
    const start = this.len;
    this.ensure(n);
    this.len += n;
    return start;
  }

  /** Append bytes, returning the payload-relative offset. */
  append(bytes: ArrayLike<number>): number {
    const off = this.len - 8;
    this.ensure(bytes.length);
    this.buf.set(bytes as Uint8Array, this.len);
    this.len += bytes.length;
    return off;
  }

  rel(abs: number): number {
    return abs - 8;
  }

  /** Bytes written so far (absolute; the header included). */
  get length(): number {
    return this.len;
  }

  slot(pos: number, off: number, len: number): void {
    this.u32(pos, off);
    this.u32(pos + 4, len);
  }

  bit(block: number, bit: number): void {
    this.buf[block + (bit >> 3)] |= 1 << (bit & 7);
  }

  /** Append the `since | block_len` trailer of the block starting at `block` (absolute). */
  trailer(block: number, version: number): void {
    const t = this.reserve(6);
    this.u16(t, version);
    this.u32(t + 2, this.len - block);
  }

  u8(pos: number, v: number): void {
    this.buf[pos] = v & 0xff;
  }
  i8(pos: number, v: number): void {
    this.buf[pos] = v & 0xff;
  }
  u16(pos: number, v: number): void {
    this.buf[pos] = v & 0xff;
    this.buf[pos + 1] = (v >>> 8) & 0xff;
  }
  i16(pos: number, v: number): void {
    this.u16(pos, v & 0xffff);
  }
  u32(pos: number, v: number): void {
    this.buf[pos] = v & 0xff;
    this.buf[pos + 1] = (v >>> 8) & 0xff;
    this.buf[pos + 2] = (v >>> 16) & 0xff;
    this.buf[pos + 3] = (v >>> 24) & 0xff;
  }
  i32(pos: number, v: number): void {
    this.u32(pos, v | 0);
  }
  u64(pos: number, v: number | bigint | string): void {
    let b: bigint;
    if (typeof v === 'bigint') b = v;
    else if (typeof v === 'string') b = BigInt(v);
    else b = BigInt(Math.round(v));
    for (let i = 0; i < 8; i++) {
      this.buf[pos + i] = Number(b & 0xffn);
      b >>= 8n;
    }
  }
  i64(pos: number, v: number | bigint | string): void {
    this.u64(pos, v);
  }
  f32(pos: number, v: number): void {
    this.scratch.setFloat32(0, v, true);
    for (let i = 0; i < 4; i++) this.buf[pos + i] = this.scratch.getUint8(i);
  }
  f64(pos: number, v: number): void {
    this.scratch.setFloat64(0, v, true);
    for (let i = 0; i < 8; i++) this.buf[pos + i] = this.scratch.getUint8(i);
  }
  bool(pos: number, v: boolean): void {
    this.buf[pos] = v ? 1 : 0;
  }

  /** Exact-size copy of everything written. */
  finish(): Uint8Array {
    return this.buf.slice(0, this.len);
  }
}

/** Numeric value of an enum given either its number or a variant name. */
export function enumNum(e: object, v: unknown): number {
  if (typeof v === 'string') {
    const n = (e as Record<string, unknown>)[v];
    if (typeof n !== 'number') throw new Error(`unknown variant ${v}`);
    return n;
  }
  return v as number;
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

function leaf(path: ValidationPath, field: string, index: number | undefined): ValidationPath {
  return index === undefined ? [...path, field] : [...path, field, index];
}

/** The slot at `pos` must stay inside the payload; returns (offset, length). */
export function vSlot(dv: DataView, pos: number, path: ValidationPath, field: string, index?: number): [number, number] {
  const off = dv.getUint32(pos, true);
  const len = dv.getUint32(pos + 4, true);
  if (off + len > dv.byteLength) {
    throw new ValidationError(`slot out of bounds (off=${off} len=${len}, payload=${dv.byteLength})`, leaf(path, field, index));
  }
  return [off, len];
}

export function vStr(dv: DataView, pos: number, path: ValidationPath, field: string, index?: number): void {
  const [o, l] = vSlot(dv, pos, path, field, index);
  try {
    TD_STRICT.decode(slotBytes(dv, o, l));
  } catch {
    throw new ValidationError('invalid utf-8', leaf(path, field, index));
  }
}

/** The slot at `pos` holds an element count of `stride`-sized elements; returns (offset, count). */
export function vRegion(dv: DataView, pos: number, stride: number, path: ValidationPath, field: string, index?: number): [number, number] {
  const off = dv.getUint32(pos, true);
  const count = dv.getUint32(pos + 4, true) & LEN_MASK;
  if (off + count * stride > dv.byteLength) {
    throw new ValidationError(`slot out of bounds (off=${off} len=${count}, payload=${dv.byteLength})`, leaf(path, field, index));
  }
  return [off, count];
}

/** Array/map region slot split into (offset, count, shape); the extent is checked by `vExtent`. */
export function vRegionShape(dv: DataView, pos: number, path: ValidationPath, field: string, index?: number): [number, number, boolean] {
  const off = dv.getUint32(pos, true);
  const raw = dv.getUint32(pos + 4, true);
  const count = raw & LEN_MASK;
  if (off + count > dv.byteLength) {
    throw new ValidationError(`slot out of bounds (off=${off} len=${raw}, payload=${dv.byteLength})`, leaf(path, field, index));
  }
  return [off, count, raw >= SHAPE_BIT];
}

/** `count` elements of `stride` bytes starting at `off` must lie inside the payload. */
export function vExtent(dv: DataView, off: number, count: number, stride: number, path: ValidationPath, field: string, index?: number): void {
  if (off + count * stride > dv.byteLength) {
    throw new ValidationError(`slot out of bounds (off=${off} len=${count}, payload=${dv.byteLength})`, leaf(path, field, index));
  }
}

/** The instance slot at `pos` must stay inside the payload; returns its [lo, hi) extent. */
export function vInstance(dv: DataView, pos: number, path: ValidationPath, field: string, index?: number): [number, number] {
  const [off, len] = vSlot(dv, pos, path, field, index);
  return [off, off + len];
}

/** The i32 at `pos` must be a known variant of the (numeric) enum object `e` named `name`. */
export function vEnum(e: object, name: string, dv: DataView, pos: number, path: ValidationPath, field: string, index?: number): void {
  const v = dv.getInt32(pos, true);
  if (typeof (e as Record<number, unknown>)[v] !== 'string') {
    throw new ValidationError(`unknown ${name} value ${v}`, leaf(path, field, index));
  }
}

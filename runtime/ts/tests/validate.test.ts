import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { Legacy, Tally, User, UserReader } from '../src/schema.js';
import { findExtBlock } from '../src/runtime.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../..');
const vector = JSON.parse(readFileSync(path.join(root, 'vectors/user.json'), 'utf8'));
const tallyVector = JSON.parse(readFileSync(path.join(root, 'vectors/tally.json'), 'utf8'));

// The committed IR snapshot is only used by the tests to locate fields; the runtime never sees it.
interface FieldIr { name: string; offset: number; mapValueOffset?: number; elementSize?: number }
interface MsgIr { name: string; fields: FieldIr[] }
const snapshot: { messages: MsgIr[] } = JSON.parse(
  readFileSync(path.join(root, 'vectors/message-snapshot.json'), 'utf8'),
);

const HEADER = 8;

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}

function putU32(b: Uint8Array, pos: number, v: number): void {
  new DataView(b.buffer, b.byteOffset, b.byteLength).setUint32(pos, v, true);
}

/** Absolute position of a base field (header included). */
function posOf(msgName: string, field: string): number {
  const m = snapshot.messages.find((x) => x.name === msgName)!;
  return HEADER + m.fields.find((f) => f.name === field)!.offset;
}

function field(msgName: string, name: string): FieldIr {
  return snapshot.messages.find((x) => x.name === msgName)!.fields.find((f) => f.name === name)!;
}

function user(): Uint8Array {
  return fromHex(vector.hex);
}

describe('validate: strict check of untrusted buffers', () => {
  it('accepts canonical encodings', () => {
    expect(User.validate(user())).toBeNull();
    expect(Tally.validate(fromHex(tallyVector.hex))).toBeNull();
  });

  it('checks the header', () => {
    const short = User.validate(user().slice(0, 10))!;
    expect(short.reason).toBe('buffer too short: need 176 bytes, got 10');
    expect(short.path).toEqual([]);
    const badKey = user();
    badKey[0] ^= 0xff;
    expect(User.validate(badKey)!.reason).toMatch(/^key mismatch/);
    const badLen = user();
    badLen[4] ^= 0xff;
    expect(User.validate(badLen)!.reason).toMatch(/^header fixed_len mismatch/);
  });

  it('reports slot bounds, utf-8 and enum values with their field path (camelCase, like the API)', () => {
    const oob = user();
    putU32(oob, posOf('User', 'name'), 0xffffffff);
    const e1 = User.validate(oob)!;
    expect(e1.path).toEqual(['name']);
    expect(e1.message).toBe(`at name: ${e1.reason}`);
    expect(e1.reason).toMatch(/^slot out of bounds \(off=4294967295 len=6/);

    const badUtf8 = user();
    const nameOff = new DataView(badUtf8.buffer).getUint32(posOf('User', 'name'), true);
    badUtf8[HEADER + nameOff] = 0xff;
    expect(User.validate(badUtf8)!.reason).toBe('invalid utf-8');

    const badEnum = user();
    putU32(badEnum, posOf('User', 'color'), 99);
    const e3 = User.validate(badEnum)!;
    expect(e3.path).toEqual(['color']);
    expect(e3.reason).toBe('unknown Color value 99');
  });

  it('nests the path through arrays, map values and nested arrays', () => {
    const byId = user();
    const byIdF = field('User', 'by_id');
    const region = new DataView(byId.buffer).getUint32(posOf('User', 'by_id'), true);
    const bioSlot = HEADER + region + byIdF.mapValueOffset! + field('Profile', 'bio').offset;
    putU32(byId, bioSlot, 0xffffffff);
    expect(User.validate(byId)!.path).toEqual(['byId', 0, 'bio']);

    const tags = user();
    const tagsRegion = new DataView(tags.buffer).getUint32(posOf('User', 'tags'), true);
    putU32(tags, HEADER + tagsRegion + field('User', 'tags').elementSize!, 0xffffffff);
    expect(User.validate(tags)!.path).toEqual(['tags', 1]);

    // grid[2][1] inside the nested dynamic array
    const grid = fromHex(tallyVector.hex);
    const payload = new DataView(grid.buffer, grid.byteOffset + HEADER, grid.byteLength - HEADER);
    const gridRegion = payload.getUint32(posOf('Tally', 'grid') - HEADER, true);
    const inner = payload.getUint32(gridRegion + 2 * 8, true);
    const innerCount = payload.getUint32(gridRegion + 2 * 8 + 4, true);
    putU32(grid, HEADER + inner + 1 * 8, 0xffffffff);
    expect(innerCount).toBe(2);
    expect(Tally.validate(grid)!.path).toEqual(['grid', 1]);
  });

  it('checks the extension chain from the tail', () => {
    const malformed = 'extension block v3 missing or malformed';

    const badSince = user();
    new DataView(badSince.buffer).setUint16(badSince.length - 6, 9, true);
    expect(User.validate(badSince)!.reason).toBe(malformed);

    const tooShort = user();
    new DataView(tooShort.buffer).setUint32(tooShort.length - 4, 7, true);
    expect(User.validate(tooShort)!.reason).toBe(malformed);
    // the lazy reader degrades instead of throwing
    expect(UserReader.decode(tooShort)!.vip).toBeNull();

    const promised = user();
    const payload = new DataView(promised.buffer, promised.byteOffset + HEADER, promised.byteLength - HEADER);
    const v3Start = findExtBlock(payload, [2, 3], 3, 3)!;
    expect(User.validate(promised.slice(0, HEADER + v3Start))!.reason).toBe(malformed);
  });

  it('findExtBlock steps over newer blocks an older reader does not know', () => {
    const payload = new DataView(user().buffer, user().byteOffset + HEADER, user().byteLength - HEADER);
    // an older reader (knows v2 only) reading a v3 buffer still finds its block behind v3
    expect(findExtBlock(payload, [2], 3, 2, 20)).not.toBeNull();
    expect(findExtBlock(payload, [2], 2, 2, 20)).toBeNull();
  });

  it('ignores reserved fields', () => {
    const buf = Legacy.encode({ id: 1, note: 'n' });
    expect(Legacy.validate(buf)).toBeNull();
    putU32(buf, posOf('Legacy', 'old_name'), 0xffffffff);
    expect(Legacy.validate(buf)).toBeNull();
  });
});

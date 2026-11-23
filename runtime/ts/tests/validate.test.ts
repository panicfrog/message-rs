import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { Tally, User, UserReader } from '../src/schema.js';
import { encodeMessage, findExtBlock, validateMessage, type SchemaIr } from '../src/runtime.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../..');
const vector = JSON.parse(readFileSync(path.join(root, 'vectors/user.json'), 'utf8'));
const tallyVector = JSON.parse(readFileSync(path.join(root, 'vectors/tally.json'), 'utf8'));
const schema: SchemaIr = JSON.parse(readFileSync(path.join(root, 'vectors/message-snapshot.json'), 'utf8'));

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
  const m = schema.messages.find((x) => x.name === msgName)!;
  return HEADER + m.fields.find((f) => f.name === field)!.offset;
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

  it('reports slot bounds, utf-8 and enum values with their field path', () => {
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

  it('nests the path through arrays and map values', () => {
    const byId = user();
    const byIdF = schema.messages.find((m) => m.name === 'User')!.fields.find((f) => f.name === 'by_id')!;
    const profile = schema.messages.find((m) => m.name === 'Profile')!;
    const region = new DataView(byId.buffer).getUint32(posOf('User', 'by_id'), true);
    const bioSlot = HEADER + region + byIdF.mapValueOffset! + profile.fields.find((f) => f.name === 'bio')!.offset;
    putU32(byId, bioSlot, 0xffffffff);
    expect(User.validate(byId)!.path).toEqual(['by_id', 0, 'bio']);

    const tags = user();
    const tagsF = schema.messages.find((m) => m.name === 'User')!.fields.find((f) => f.name === 'tags')!;
    const tagsRegion = new DataView(tags.buffer).getUint32(posOf('User', 'tags'), true);
    putU32(tags, HEADER + tagsRegion + tagsF.elementSize!, 0xffffffff);
    expect(User.validate(tags)!.path).toEqual(['tags', 1]);
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

  it('lets an older reader find its blocks behind a newer writer', () => {
    const v2Schema: SchemaIr = JSON.parse(JSON.stringify(schema));
    const u = v2Schema.messages.find((m) => m.name === 'User')!;
    u.fields = u.fields.filter((f) => f.since <= 2);
    u.layout.extBlocks = u.layout.extBlocks!.filter((b) => b.version <= 2);
    expect(validateMessage(v2Schema, 'User', user())).toBeNull();
    const payload = new DataView(user().buffer, user().byteOffset + HEADER, user().byteLength - HEADER);
    expect(findExtBlock(payload, [2], 3, 2, 20)).not.toBeNull();
  });

  it('ignores reserved fields', () => {
    const buf = encodeMessage(schema, 'Legacy', { id: 1, note: 'n' });
    expect(validateMessage(schema, 'Legacy', buf)).toBeNull();
    putU32(buf, posOf('Legacy', 'old_name'), 0xffffffff);
    expect(validateMessage(schema, 'Legacy', buf)).toBeNull();
  });
});

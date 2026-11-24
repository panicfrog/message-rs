import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { Tally, TallyReader, User, UserReader } from '../src/schema.js';
import { StringListView } from '../src/runtime.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../..');
const vector = JSON.parse(readFileSync(path.join(root, 'vectors/user.json'), 'utf8'));
const tallyVector = JSON.parse(readFileSync(path.join(root, 'vectors/tally.json'), 'utf8'));

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** The vectors carry the Rust field names; the TS object layer uses camelCase (and bigint for precise u64). */
function toTsValue(v: unknown, precise = false): unknown {
  if (Array.isArray(v)) return v.map((e) => toTsValue(e));
  if (v && typeof v === 'object') {
    const o: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v as Record<string, unknown>)) {
      o[k.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase())] = toTsValue(x, k === 'precise_id');
    }
    return o;
  }
  return precise ? BigInt(v as string) : v;
}

describe('Rust <-> TS cross-language conformance (full-type User vector)', () => {
  it('decode: zero-copy reads match the expected values encoded by Rust', () => {
    const r = UserReader.decode(fromHex(vector.hex));
    expect(r).not.toBeNull();
    const e = vector.expected;

    expect(r!.id).toBe(e.id);
    expect(r!.name).toBe(e.name);
    expect(r!.color).toBe(e.color);
    expect(r!.profile.age).toBe(e.profile.age);
    expect(r!.profile.bio).toBe(e.profile.bio);
    expect(r!.profile.nickname).toBe(e.profile.nickname);
    // Profile's own extension block (v2) lives in its self-contained instance
    expect(r!.profile.verified).toBe(e.profile.verified);

    expect(r!.matrix.length).toBe(4);
    for (let i = 0; i < 4; i++) expect(r!.matrix.get(i)).toBeCloseTo(e.matrix[i]);

    expect(r!.nums.length).toBe(e.nums.length);
    const nums = [] as number[];
    for (let i = 0; i < r!.nums.length; i++) nums.push(r!.nums.get(i));
    expect(nums).toEqual(e.nums);

    expect(r!.tags.length).toBe(3);
    expect(r!.tags.get(1)).toBe(e.tags[1]);

    expect(Array.from(r!.blobs.get(0))).toEqual(e.blobs[0]);
    expect(r!.blobs.get(1).length).toBe(0);
    expect(r!.blobs.get(2).length).toBe(300);

    expect(r!.edges.length).toBe(2);
    expect(r!.edges.get(0).value).toBe(1);
    expect(r!.edges.get(0).children.get(0).value).toBe(11);
    expect(r!.edges.get(0).children.get(0).children.length).toBe(0);

    expect(r!.counts.length).toBe(2);
    const counts: Record<string, number> = {};
    for (let i = 0; i < r!.counts.length; i++) counts[r!.counts.key(i)] = r!.counts.value(i);
    expect(counts).toEqual(e.counts);

    expect(r!.byId.length).toBe(1);
    expect(r!.byId.key(0)).toBe(7);
    expect(r!.byId.value(0).age).toBe(e.by_id['7'].age);
    expect(r!.byId.value(0).bio).toBe(e.by_id['7'].bio);
    expect(r!.byId.value(0).nickname).toBeNull();
    expect(r!.byId.value(0).verified).toBeNull();

    // unevolved struct elements stay compact (no shape bit, fixed-region footprints)
    expect(r!.points.length).toBe(2);
    expect(r!.points.isInstanceMode).toBe(false);
    expect(r!.points.get(0).x).toBe(e.points[0].x);
    expect(r!.points.get(1).y).toBe(e.points[1].y);

    expect(Array.from(r!.avatar!)).toEqual(e.avatar);
    expect(r!.motto).toBeNull();
    expect(r!.stamp).toBe(e.stamp);
    expect(r!.ratio).toBe(e.ratio);
    expect(r!.flag).toBe(e.flag);
    expect(r!.small).toBe(e.small);
    expect(r!.wide).toBe(e.wide);
    expect(r!.offset).toBe(e.offset);
    expect(r!.preciseId).toBe(BigInt(e.precise_id));

    // extension-block fields (v2/v3)
    expect(r!.email).toBe(e.email);
    expect(r!.score).toBe(e.score);
    expect(r!.tags2.get(0)).toBe(e.tags2[0]);
    expect(r!.vip).toBe(e.vip);
  });

  it('encode: the generated encoder is byte-identical to Rust', () => {
    const bytes = User.encode(toTsValue(vector.expected) as User);
    expect(toHex(bytes)).toBe(vector.hex);
  });

  it('object layer: fromBytes -> plain object -> encode byte-identical roundtrip', () => {
    const u = User.fromBytes(fromHex(vector.hex));
    expect(u).not.toBeNull();
    expect(u!.id).toBe(42);
    expect(u!.name).toBe('山田');
    expect(u!.profile.bio).toBe('hello 世界');
    expect(u!.tags).toEqual(['a', 'b', 'c']);
    expect(u!.counts).toEqual({ one: 1, two: 2 });
    expect(u!.byId['7'].bio).toBe('nested');
    expect(u!.byId['7'].verified).toBeNull();
    expect(u!.profile).toEqual({ age: 30, bio: 'hello 世界', nickname: 'yamada', verified: true });
    expect(u!.email).toBe('user@example.com');
    expect(u!.vip).toBe(true);
    // Reader <-> object
    const r = UserReader.decode(fromHex(vector.hex))!;
    expect(User.fromReader(r)).toEqual(u);
    // re-encode the object; must match Rust byte for byte
    expect(toHex(User.encode(u!))).toBe(vector.hex);
  });

  it('encode: nullish arrays and maps are treated as empty (lenient input)', () => {
    const u = User.fromBytes(fromHex(vector.hex))!;
    const sparse = { ...u, nums: null, counts: null, motto: undefined } as unknown as User;
    const bytes = User.encode(sparse);
    const r = UserReader.decode(bytes)!;
    expect(r.nums.length).toBe(0);
    expect(r.counts.length).toBe(0);
    expect(r.motto).toBeNull();
    expect(r.id).toBe(42);
  });
});

describe('Rust <-> TS conformance (enum-keyed / array-valued maps, fixed string arrays, nested arrays)', () => {
  it('decode: enum keys come back as the numeric value, array values as views', () => {
    const buf = fromHex(tallyVector.hex);
    const r = TallyReader.decode(buf);
    expect(r).not.toBeNull();
    expect(r!.usage.length).toBe(2);
    expect(r!.usage.key(0)).toBe(0); // Color.Red
    expect(r!.usage.value(0)).toBe(1);
    expect(r!.usage.key(1)).toBe(2); // Color.Blue
    expect(r!.usage.value(1)).toBe(3);
    expect(r!.labels.key(0)).toBe('a');
    expect(r!.labels.value(0).get(1)).toBe('y');
    expect(r!.labels.key(1)).toBe('b');
    expect(r!.labels.value(1).length).toBe(0);
    expect(r!.aliases.length).toBe(2);
    expect(r!.aliases.get(1)).toBe('as');
    expect(r!.grid.length).toBe(3);
    // nested dynamic array: each element is a slot pointing at an inner string-list region
    const payload = new DataView(buf.buffer, buf.byteOffset + 8, buf.byteLength - 8);
    const [o, l] = r!.grid.slotOf(2);
    expect(new StringListView(payload, o, l).get(1)).toBe('i');
  });

  it('encode: byte-identical to Rust with numeric-string enum keys', () => {
    expect(toHex(Tally.encode(toTsValue(tallyVector.expected) as Tally))).toBe(tallyVector.hex);
  });

  it('object layer: roundtrip is byte-identical; variant names are accepted as enum keys', () => {
    const t = Tally.fromBytes(fromHex(tallyVector.hex));
    expect(t).not.toBeNull();
    expect(t!.usage).toEqual({ '0': 1, '2': 3 });
    expect(t!.labels).toEqual({ a: ['x', 'y'], b: [] });
    expect(t!.aliases).toEqual(['al', 'as']);
    expect(t!.grid).toEqual([['g'], [], ['h', 'i']]);
    expect(toHex(Tally.encode(t!))).toBe(tallyVector.hex);
    const byName = { ...t!, usage: { Red: 1, Blue: 3 } } as Tally;
    expect(toHex(Tally.encode(byName))).toBe(tallyVector.hex);
  });
});

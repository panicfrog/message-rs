import { describe, expect, it, vi, beforeEach } from 'vitest';

// The generated accessor bootstraps through react-native's TurboModuleRegistry; in tests the
// native module is a fake modelling the Rust core's semantics. This pins the user-facing
// contract: publish(id, value), pull returning values, Closed, and kind inference.
type NativeEvent = { kind: number; bytes: number[] | null; closed: boolean };
type Channel = { id: number; kind: number; closed: boolean; value: unknown; bytes?: Uint8Array };

interface FakeNative {
  subscribe(id: number, cb: (e: NativeEvent) => void): number;
  unsubscribe(subId: number): void;
  publish(handle: number, data: ArrayBuffer): void;
  get(id: number): ArrayBuffer;
  createPublisher(id: number, kind: number): number;
  closePublisher(handle: number): void;
  readonly kind: number;
}

/** A minimal stand-in for the Rust hub: same push/pull/Closed semantics as the core. */
function makeFake(): FakeNative {
  const subs = new Map<number, { id: number; cb: (e: NativeEvent) => void }>();
  const pubs = new Map<number, { id: number; kind: number }>();
  const last = new Map<number, { kind: number; bytes: Uint8Array }>();
  let nextSub = 1;
  let nextPub = 1;
  let lastKind = -1;

  return {
    subscribe(id, cb) {
      const subId = nextSub++;
      subs.set(subId, { id, cb });
      return subId;
    },
    unsubscribe(subId) {
      subs.delete(subId);
    },
    createPublisher(id, kind) {
      const h = nextPub++;
      pubs.set(h, { id, kind });
      return h;
    },
    publish(handle, data) {
      const p = pubs.get(handle);
      if (!p) throw new Error(`unknown publisher handle ${handle}`);
      const bytes = new Uint8Array(data);
      last.set(p.id, { kind: p.kind, bytes });
      for (const s of subs.values()) {
        if (s.id === p.id) s.cb({ kind: p.kind, bytes: Array.from(bytes), closed: false });
      }
    },
    closePublisher(handle) {
      const p = pubs.get(handle);
      if (!p) return;
      pubs.delete(handle);
      const stillLive = [...pubs.values()].some((q) => q.id === p.id);
      if (!stillLive) {
        last.delete(p.id);
        for (const s of subs.values()) {
          if (s.id === p.id) s.cb({ kind: 255, bytes: null, closed: true });
        }
      }
    },
    get(id) {
      const v = last.get(id);
      lastKind = v ? v.kind : -1;
      return v ? v.bytes.slice().buffer : new ArrayBuffer(0);
    },
    get kind() {
      return lastKind;
    },
  };
}

let fake: FakeNative;

vi.mock('../generated/rust-turbo/Hub', () => ({ getHub: () => fake }));

const hub = await import('../index');

beforeEach(() => {
  fake = makeFake();
});

describe('value-first API', () => {
  it('publishes plain values and pulls them back decoded', () => {
    hub.publish(1, true);
    expect(hub.pull(1)).toBe(true);
    hub.publish(2, 'hi');
    expect(hub.pull(2)).toBe('hi');
    hub.publish(3, 1.5);
    expect(hub.pull(3)).toBe(1.5); // number defaults to f64
    hub.publish(4, 42n);
    expect(hub.pull(4)).toBe(42n); // bigint defaults to i64
    hub.publish(5, new Uint8Array([1, 2, 3]));
    expect(Array.from(hub.pull(5) as Uint8Array)).toEqual([1, 2, 3]);
  });

  it('honours explicit widths through the wrappers', () => {
    hub.publish(10, hub.u16(65535));
    expect(hub.pullRaw(10)!.kind).toBe(hub.ValueKind.U16);
    expect(hub.pull(10)).toBe(65535);
    hub.publish(11, hub.i32(-1));
    expect(hub.pullRaw(11)!.kind).toBe(hub.ValueKind.I32);
    expect(hub.pull(11)).toBe(-1);
    hub.publish(12, hub.u64(18446744073709551615n));
    expect(hub.pull(12)).toBe(18446744073709551615n);
    hub.publish(13, hub.f32(1.5));
    expect(hub.pullRaw(13)!.kind).toBe(hub.ValueKind.F32);
  });

  it('channel delivers decoded values, and the returned function unsubscribes', () => {
    const seen: Channel[] = [];
    const off = hub.channel(20, (e) => seen.push(e));
    hub.publish(20, 'hello');
    expect(seen).toHaveLength(1);
    expect(seen[0].value).toBe('hello');
    expect(seen[0].closed).toBe(false);
    off();
    hub.publish(20, 'again');
    expect(seen).toHaveLength(1);
  });

  it('close delivers Closed once and empties the pull', () => {
    const seen: Channel[] = [];
    hub.channel(21, (e) => seen.push(e));
    hub.publish(21, 1);
    hub.close(21);
    expect(seen.filter((e) => e.closed)).toHaveLength(1);
    expect(hub.pull(21)).toBeNull();
    hub.close(21); // idempotent
    expect(seen.filter((e) => e.closed)).toHaveLength(1);
  });

  it('a subscription survives Closed and sees a new publisher', () => {
    const seen: Channel[] = [];
    hub.channel(22, (e) => seen.push(e));
    hub.publish(22, 'first');
    hub.close(22);
    hub.publish(22, 'second');
    expect(seen.map((e) => e.value)).toEqual(['first', null, 'second']);
  });

  it('changing the kind on an id supersedes the previous publisher', () => {
    hub.publish(23, hub.u16(1));
    expect(hub.pullRaw(23)!.kind).toBe(hub.ValueKind.U16);
    hub.publish(23, 'now a string');
    expect(hub.pullRaw(23)!.kind).toBe(hub.ValueKind.STRING);
    expect(hub.pull(23)).toBe('now a string');
  });

  it('passes the Rust message vector through byte-for-byte', async () => {
    const { readFileSync } = await import('node:fs');
    const { resolve } = await import('node:path');
    const vector = JSON.parse(
      readFileSync(resolve(__dirname, '../../../vectors/user.json'), 'utf8'),
    ) as { hex: string; key: number };
    const bytes = Uint8Array.from((vector.hex.match(/../g) ?? []).map((b) => parseInt(b, 16)));
    hub.publish(vector.key, hub.message(bytes));
    expect(Array.from(hub.pull(vector.key) as Uint8Array)).toEqual(Array.from(bytes));
    expect(hub.pullRaw(vector.key)!.kind).toBe(hub.ValueKind.MESSAGE);
  });
});

describe('generics are a compile-time hint only', () => {
  it('decodes by the sender kind regardless of the type argument', () => {
    // The publisher sent a u16; asking for a string does not change what comes back.
    hub.publish(30, hub.u16(65535));
    const asString = hub.pull<string>(30);
    expect(typeof asString).toBe('number');
    expect(asString as unknown as number).toBe(65535);
    expect(hub.pullRaw(30)!.kind).toBe(hub.ValueKind.U16);
  });

  it('channel events carry the real kind even with a narrower hint', () => {
    const seen: { kind: number; value: unknown }[] = [];
    hub.channel<boolean>(31, (e) => { if (!e.closed) seen.push({ kind: e.kind, value: e.value }); });
    hub.publish(31, 'a string, not a bool');
    expect(seen[0].kind).toBe(hub.ValueKind.STRING);
    expect(seen[0].value).toBe('a string, not a bool');
  });
});

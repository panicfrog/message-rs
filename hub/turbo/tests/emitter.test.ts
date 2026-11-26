import { describe, expect, it, vi, beforeEach } from 'vitest';

// The generated accessor bootstraps through react-native's TurboModuleRegistry; in tests the
// native module is a fake driven by the real Rust core semantics (a small in-memory model).
// This pins the emitter layer's contract: wake-up + pull, handle routing, Closed delivery.
type Event = { kind: number; bytes: number[] | null; closed: boolean };

interface FakeNative {
  subscribe(id: number, cb: (e: Event) => void): number;
  unsubscribe(subId: number): void;
  publish(handle: number, data: ArrayBuffer): void;
  get(id: number): ArrayBuffer;
  createPublisher(id: number, kind: number): number;
  closePublisher(handle: number): void;
  readonly kind: number;
}

/** A minimal stand-in for the Rust hub: same push/pull/Closed semantics as the core. */
function makeFake(): FakeNative & { fire(id: number, bytes: number[]): void } {
  const subs = new Map<number, { id: number; cb: (e: Event) => void }>();
  const pubs = new Map<number, { id: number; kind: number; closed: boolean }>();
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
      pubs.set(h, { id, kind, closed: false });
      return h;
    },
    publish(handle, data) {
      const p = pubs.get(handle);
      if (!p || p.closed) throw new Error(`unknown publisher handle ${handle}`);
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
      // Closed fires when this was the id's last live publisher
      const stillLive = [...pubs.values()].some((q) => q.id === p.id && !q.closed);
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
    fire(id, bytes) {
      for (const s of subs.values()) {
        if (s.id === id) s.cb({ kind: 12, bytes, closed: false });
      }
    },
  };
}

let fake: ReturnType<typeof makeFake>;

vi.mock('../generated/rust-turbo/Hub', () => ({
  getHub: () => fake,
}));

// Imported after the mock so the emitter binds to the fake.
const emitter = await import('../index');

beforeEach(() => {
  fake = makeFake();
});

describe('hub-turbo emitter layer', () => {
  it('channel delivers events and the returned function unsubscribes', () => {
    const seen: Event[] = [];
    const off = emitter.channel(1, (e) => seen.push(e));
    const h = emitter.createPublisher(1, emitter.ValueKind.BYTES);
    emitter.publish(h, new Uint8Array([1, 2, 3]).buffer);
    expect(seen).toHaveLength(1);
    expect(seen[0].bytes).toEqual([1, 2, 3]);
    expect(seen[0].closed).toBe(false);

    off();
    emitter.publish(h, new Uint8Array([4]).buffer);
    expect(seen).toHaveLength(1);
  });

  it('pull returns the newest payload zero-copy-shaped and null when absent', () => {
    expect(emitter.pull(9)).toBeNull();
    const h = emitter.createPublisher(9, emitter.ValueKind.STRING);
    emitter.publish(h, new TextEncoder().encode('hi').buffer);
    const got = emitter.pull(9);
    expect(got).not.toBeNull();
    expect(new TextDecoder().decode(got!)).toBe('hi');
    expect(emitter.lastKind()).toBe(emitter.ValueKind.STRING);
  });

  it('Closed arrives once when the last publisher closes; the subscription survives', () => {
    const seen: Event[] = [];
    emitter.channel(2, (e) => seen.push(e));
    const a = emitter.createPublisher(2, emitter.ValueKind.BYTES);
    const b = emitter.createPublisher(2, emitter.ValueKind.BYTES);
    emitter.publish(a, new Uint8Array([1]).buffer);
    emitter.closePublisher(a);
    expect(seen.filter((e) => e.closed)).toHaveLength(0); // b is still live
    emitter.closePublisher(b);
    expect(seen.filter((e) => e.closed)).toHaveLength(1);
    expect(emitter.pull(2)).toBeNull();

    // a new publisher reaches the surviving subscription again
    const c = emitter.createPublisher(2, emitter.ValueKind.BYTES);
    emitter.publish(c, new Uint8Array([7]).buffer);
    expect(seen.filter((e) => !e.closed)).toHaveLength(2);
  });

  it('publish reports an unknown handle as false instead of throwing', () => {
    expect(emitter.publish(999, new ArrayBuffer(2))).toBe(false);
    const h = emitter.createPublisher(3, emitter.ValueKind.BYTES);
    expect(emitter.publish(h, new Uint8Array([1]).buffer)).toBe(true);
    emitter.closePublisher(h);
    expect(emitter.publish(h, new Uint8Array([1]).buffer)).toBe(false);
  });

  it('the message vector passes through byte-for-byte', async () => {
    const { readFileSync } = await import('node:fs');
    const { resolve } = await import('node:path');
    const vector = JSON.parse(
      readFileSync(resolve(__dirname, '../../../vectors/user.json'), 'utf8'),
    ) as { hex: string; key: number };
    const bytes = Uint8Array.from(
      (vector.hex.match(/../g) ?? []).map((b) => parseInt(b, 16)),
    );
    const h = emitter.createPublisher(vector.key, emitter.ValueKind.MESSAGE);
    emitter.publish(h, bytes.buffer);
    const got = new Uint8Array(emitter.pull(vector.key)!);
    expect(Array.from(got)).toEqual(Array.from(bytes));
    expect(emitter.lastKind()).toBe(emitter.ValueKind.MESSAGE);
  });
});

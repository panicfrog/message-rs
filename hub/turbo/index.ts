/**
 * message-rs data hub for React Native — the JS emitter layer over the generated
 * `Hub` turbo module (see hub/DESIGN.md).
 *
 * Push is a wake-up: the callback fires with a small event (the payload for small
 * values is already in `event.bytes`); the current value always comes from `pull` /
 * `get`, zero-copy as an ArrayBuffer. `MESSAGE` payloads decode with the generated
 * message schema's `decodeBy(key)` when passed to it.
 *
 * ```ts
 * import { channel } from '@message-rs/hub-turbo';
 *
 * const off = channel(1, (event) => { ... });
 * const bytes = pull(1);          // ArrayBuffer | null
 * off();
 * ```
 */

import { getHub } from './generated/rust-turbo/Hub';
import type { HubEvent } from './generated/rust-turbo/structs/HubEvent';
import { decode, encode, ValueError, type DecodedValue } from './value';
import { ValueKind, CLOSED_KIND } from './kinds';
import { resolve, type Publishable } from './typed';

export { getHub } from './generated/rust-turbo/Hub';
export type { Hub } from './generated/rust-turbo/Hub';
export type { HubEvent } from './generated/rust-turbo/structs/HubEvent';

export { ValueKind, CLOSED_KIND } from './kinds';
export { decode, encode, ValueError, type DecodedValue } from './value';
export {
  bool, u8, i8, u16, i16, u32, i32, u64, i64, f32, f64, str, bytes, message,
  type Typed, type Publishable,
} from './typed';

type NativeHub = ReturnType<typeof getHub>;

let native: NativeHub | null = null;
function hub(): NativeHub {
  if (!native) native = getHub();
  return native;
}

/**
 * Subscribes `handler` to `id`. Returns the unsubscribe function. The handler runs on the
 * JS thread (the turbo callback hops through the CallInvoker). Closed arrives once when
 * the id's last publisher goes away; the subscription survives it.
 *
 * `T` is a **compile-time hint only**: it narrows what the handler sees, it does not convert
 * or validate anything. The value always comes from decoding the payload's real kind, so a
 * publisher sending another type is a publisher bug — the data you receive is still correct.
 */
export function channel<T extends DecodedValue = DecodedValue>(
  id: number,
  handler: (event: ChannelEvent<T>) => void,
): () => void {
  const subId = hub().subscribe(id, (raw) => {
    const e = raw as HubEvent;
    if (e.closed || e.kind === CLOSED_KIND) {
      handler({ id, kind: CLOSED_KIND, closed: true, value: null });
      return;
    }
    const bytes = Uint8Array.from(e.bytes ?? []);
    // The cast only satisfies the caller's hint; the value is whatever the kind decoded to.
    handler({ id, kind: e.kind, closed: false, value: decode(e.kind, bytes) as T, bytes });
  });
  return () => hub().unsubscribe(subId);
}

/**
 * What a [channel] handler receives: the value already decoded (bytes only for MESSAGE).
 * `T` narrows `value` for the compiler; `kind` always reports the real wire kind.
 */
export interface ChannelEvent<T extends DecodedValue = DecodedValue> {
  id: number;
  /** The real wire kind of this payload (255 for Closed). */
  kind: number;
  closed: boolean;
  /** The decoded value; null for Closed. `Uint8Array` for BYTES/MESSAGE. */
  value: T | null;
  /** The raw payload, for callers that want it (also set for basic kinds). */
  bytes?: Uint8Array;
}

/**
 * Pulls the newest value of `id`, **already decoded**: a bool/number/bigint/string for the basic
 * kinds, a `Uint8Array` for BYTES and MESSAGE (MESSAGE needs the generated schema to go further).
 * Null when nothing is published or the last publisher closed.
 *
 * `T` is a **compile-time hint only** — `pull<string>(id)` saves you a narrowing step, it does
 * not convert or check. The result is always the real kind's decoding; use [pullRaw] to see the
 * kind, or leave `T` off and narrow with `typeof` when the id's type is not fixed by convention.
 */
export function pull<T extends DecodedValue = DecodedValue>(id: number): T | null {
  const got = pullRaw(id);
  return got === null ? null : (decode(got.kind, got.bytes) as T);
}

/** The raw pull: `{ kind, bytes }`, for byte-level work (schema decoding, hashing, forwarding). */
export function pullRaw(id: number): { kind: number; bytes: Uint8Array } | null {
  const buf = hub().get(id);
  const kind = hub().kind;
  if (kind < 0 || buf.byteLength === 0) return null;
  return { kind, bytes: new Uint8Array(buf) };
}

/**
 * Publishes a value to `id`. The wire kind follows from the value (boolean/string/bigint/bytes;
 * `number` is f64) — wrap it for an exact width: `publish(id, u16(65535))`.
 *
 * The publisher handle is managed per id and per kind: publishing a different kind to the same
 * id closes the previous publisher first (a publisher is a source of exactly one type).
 */
export function publish(id: number, value: Publishable): void {
  const { kind, bytes } = resolve(value);
  const key = `${id}:${kind}`;
  let handle = handles.get(key);
  if (handle === undefined) {
    // a different kind on this id supersedes the old publisher
    for (const [k, h] of handles) {
      if (k.startsWith(`${id}:`) && k !== key) {
        hub().closePublisher(h);
        handles.delete(k);
      }
    }
    handle = hub().createPublisher(id, kind);
    handles.set(key, handle);
  }
  hub().publish(handle, bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer);
}

/**
 * Ends publishing to `id`: subscribers get Closed once, and pull goes empty. Idempotent.
 * (The per-id publisher handles are internal; this is the user-facing lifecycle call.)
 */
export function close(id: number): void {
  for (const [k, h] of handles) {
    if (k.startsWith(`${id}:`)) {
      hub().closePublisher(h);
      handles.delete(k);
    }
  }
}

const handles = new Map<string, number>();


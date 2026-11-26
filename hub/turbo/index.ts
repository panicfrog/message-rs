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

export { getHub } from './generated/rust-turbo/Hub';
export type { Hub } from './generated/rust-turbo/Hub';
export type { HubEvent } from './generated/rust-turbo/structs/HubEvent';

/** The kind discriminants, mirroring `message_hub::ValueKind`. */
export const ValueKind = {
  BOOL: 0, U8: 1, I8: 2, U16: 3, I16: 4, U32: 5, I32: 6,
  U64: 7, I64: 8, F32: 9, F64: 10, STRING: 11, BYTES: 12, MESSAGE: 13,
} as const;

/** Closed events carry this kind. */
export const CLOSED_KIND = 255;

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
 */
export function channel(
  id: number,
  handler: (event: HubEvent) => void,
): () => void {
  const subId = hub().subscribe(id, (event) => handler(event));
  return () => hub().unsubscribe(subId);
}

/**
 * Pulls the newest payload of `id` as an ArrayBuffer (zero-copy; the JS GC owns the
 * backing memory). Null when nothing is published or the last publisher closed. The kind
 * of the returned value is available as `lastKind()` immediately after.
 */
export function pull(id: number): ArrayBuffer | null {
  const buf = hub().get(id);
  if (buf.byteLength === 0) return null;
  return buf;
}

/** The kind of the value `pull` just returned (-1 when nothing was published). */
export function lastKind(): number {
  return hub().kind;
}

/**
 * Publishes a payload through the publisher created by `createPublisher`. The buffer is
 * read in place. Returns false for an unknown (closed) handle.
 */
export function publish(handle: number, data: ArrayBuffer): boolean {
  try {
    hub().publish(handle, data);
    return true;
  } catch {
    return false;
  }
}

/** Creates a publisher for `(id, kind)`; returns its handle. */
export function createPublisher(id: number, kind: number): number {
  return hub().createPublisher(id, kind);
}

/** Closes a publisher; subscribers get Closed when this is the id's last live one. */
export function closePublisher(handle: number): void {
  hub().closePublisher(handle);
}

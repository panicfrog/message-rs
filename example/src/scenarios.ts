/**
 * The demo's cross-language checks. Each scenario asserts on bytes, not just "an event
 * arrived" — the claim under test is that JS, Swift and Kotlin share ONE Rust registry.
 *
 * Run from the UI (App.tsx) or any automation; results are plain data so both can render them.
 */

import { NativeModules, NativeEventEmitter } from 'react-native';
import { channel, pull, createPublisher, closePublisher, publish, ValueKind, CLOSED_KIND } from 'hub-turbo';

export type Result = { name: string; ok: boolean; detail: string };

const native = NativeModules.HubNative as {
  publishFromNative(id: number, kind: number, bytes: number[]): Promise<number>;
  closePublisher(handle: number): void;
  subscribeFromNative(id: number): Promise<number>;
  unsubscribeNative(handle: number): void;
  pullFromNative(id: number): Promise<{ kind: number; bytes: number[] } | null>;
} | undefined;

function bytesOf(buf: ArrayBuffer): number[] {
  return Array.from(new Uint8Array(buf));
}

function eq(a: number[], b: number[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

/** Waits for `cond`, polling; the deliveries hop threads so nothing is synchronous here. */
async function until(cond: () => boolean, ms = 3000): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (cond()) return true;
    await new Promise<void>((r) => setTimeout(() => r(), 25));
  }
  return cond();
}

/** JS → JS: the baseline, already covered by unit tests, re-checked in the real runtime. */
async function jsToJs(): Promise<Result> {
  const id = 9001;
  const seen: number[][] = [];
  const off = channel(id, (e) => { if (!e.closed && e.bytes) seen.push(e.bytes); });
  try {
    const h = createPublisher(id, ValueKind.BYTES);
    publish(h, new Uint8Array([1, 2, 3]).buffer);
    const arrived = await until(() => seen.length >= 1);
    const pulled = pull(id);
    const ok = arrived && eq(seen[0], [1, 2, 3]) && !!pulled && eq(bytesOf(pulled), [1, 2, 3]);
    closePublisher(h);
    return { name: 'JS → JS', ok, detail: ok ? 'event + pull match' : `seen=${JSON.stringify(seen)} pull=${pulled ? bytesOf(pulled) : null}` };
  } finally {
    off();
  }
}

/** JS publishes, the native subscription fires and reports back through the RN emitter. */
async function jsToSwift(): Promise<Result> {
  if (!native) return { name: 'JS → native', ok: false, detail: 'HubNative module missing' };
  const id = 9002;
  const seen: number[][] = [];
  const emitter = new NativeEventEmitter(NativeModules.HubNative);
  const listener = emitter.addListener('HubNativeEvent', (raw: unknown) => {
    const e = raw as { id: number; bytes: number[]; closed: boolean };
    if (e.id === id && !e.closed) seen.push(e.bytes);
  });
  const subHandle = await native.subscribeFromNative(id);
  try {
    const h = createPublisher(id, ValueKind.BYTES);
    publish(h, new Uint8Array([7, 8, 9]).buffer);
    const arrived = await until(() => seen.length >= 1);
    const ok = arrived && eq(seen[0], [7, 8, 9]);
    closePublisher(h);
    return { name: 'JS → native', ok, detail: ok ? 'the native side saw the JS bytes' : `native saw ${JSON.stringify(seen)}` };
  } finally {
    listener.remove();
    native.unsubscribeNative(subHandle);
  }
}

/** The native side publishes, the JS channel receives — the reverse direction. */
async function swiftToJs(): Promise<Result> {
  if (!native) return { name: 'native → JS', ok: false, detail: 'HubNative module missing' };
  const id = 9003;
  const seen: number[][] = [];
  const off = channel(id, (e) => { if (!e.closed && e.bytes) seen.push(e.bytes); });
  try {
    const handle = await native.publishFromNative(id, ValueKind.BYTES, [4, 5, 6]);
    const arrived = await until(() => seen.length >= 1);
    const ok = arrived && eq(seen[0], [4, 5, 6]);
    native.closePublisher(handle);
    return { name: 'native → JS', ok, detail: ok ? 'JS saw the native bytes' : `js saw ${JSON.stringify(seen)}` };
  } finally {
    off();
  }
}

/** One store: the native side publishes, JS pulls the identical bytes. */
async function sharedPull(): Promise<Result> {
  if (!native) return { name: 'shared pull', ok: false, detail: 'HubNative module missing' };
  const id = 9004;
  const handle = await native.publishFromNative(id, ValueKind.STRING, Array.from(new TextEncoder().encode('from native')));
  try {
    const jsPulled = pull(id);
    const nativePulled = await native.pullFromNative(id);
    const expected = Array.from(new TextEncoder().encode('from native'));
    const ok = !!jsPulled && eq(bytesOf(jsPulled), expected) && !!nativePulled && eq(nativePulled.bytes, expected);
    return {
      name: 'shared pull',
      ok,
      detail: ok ? 'both ends read the same store' : `js=${jsPulled ? bytesOf(jsPulled) : null} native=${nativePulled?.bytes ?? null}`,
    };
  } finally {
    native.closePublisher(handle);
  }
}

/** Closed crosses languages: the native last publisher closes, the JS subscriber sees it once. */
async function closedAcross(): Promise<Result> {
  if (!native) return { name: 'Closed across', ok: false, detail: 'HubNative module missing' };
  const id = 9005;
  let closedCount = 0;
  const off = channel(id, (e) => { if (e.closed || e.kind === CLOSED_KIND) closedCount++; });
  try {
    const handle = await native.publishFromNative(id, ValueKind.BYTES, [1]);
    await until(() => false, 200); // let the value land first
    native.closePublisher(handle);
    const arrived = await until(() => closedCount >= 1);
    const ok = arrived && closedCount === 1 && pull(id) === null;
    return { name: 'Closed across', ok, detail: ok ? 'exactly one Closed, pull empty' : `closedCount=${closedCount}` };
  } finally {
    off();
  }
}


/** Reports what this runtime provides (the ASCII helper's premise, checked not assumed). */
async function runtimeGlobals(): Promise<Result> {
  const g = globalThis as Record<string, unknown>;
  const has = (k: string) => typeof g[k] !== 'undefined';
  let encWorks = false;
  try {
    const E = g.TextEncoder as (new () => { encode(s: string): Uint8Array }) | undefined;
    if (E) encWorks = Array.from(new E().encode('hi')).join(',') === '104,105';
  } catch { /* reported below */ }
  return {
    name: 'runtime globals',
    ok: true,
    detail: `TextEncoder=${has('TextEncoder')} works=${encWorks} TextDecoder=${has('TextDecoder')}`,
  };
}

export async function runAll(): Promise<Result[]> {
  const out: Result[] = [];
  for (const scenario of [jsToJs, jsToSwift, swiftToJs, sharedPull, closedAcross, runtimeGlobals]) {
    try {
      out.push(await scenario());
    } catch (e) {
      out.push({ name: scenario.name, ok: false, detail: String(e) });
    }
  }
  return out;
}

// RN 0.88's Hermes provides TextEncoder/TextDecoder at runtime (verified in the demo's
// "runtime globals" check) but its TypeScript types do not declare them. Declare the minimal
// surface here rather than pulling in the whole DOM lib.
declare class TextEncoder {
  encode(input?: string): Uint8Array;
}

declare class TextDecoder {
  constructor(label?: string);
  decode(input?: ArrayBufferView | ArrayBuffer): string;
}

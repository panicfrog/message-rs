/** The kind discriminants, mirroring `message_hub::ValueKind` (stable across the FFI). */
export const ValueKind = {
  BOOL: 0, U8: 1, I8: 2, U16: 3, I16: 4, U32: 5, I32: 6,
  U64: 7, I64: 8, F32: 9, F64: 10, STRING: 11, BYTES: 12, MESSAGE: 13,
} as const;

/** Closed events carry this kind; outside ValueKind's range on purpose. */
export const CLOSED_KIND = 255;

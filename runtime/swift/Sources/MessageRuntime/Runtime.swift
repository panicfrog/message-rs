// message-rs Swift runtime (zero-dependency; iOS / macOS / Linux).
//
// Layout rules follow the repo SPEC.md: little-endian payload; slot `{ offset: u32, len: u32 }`
// offsets are payload-relative; extension blocks are
// `[ bitmap | fixed fields | var data | since:u16 | block_len:u32 ]` with the trailer at the
// tail, located by walking back from the payload end.
//
// The schema is compiled into the generated Schema.swift (readers, encoders and validators with
// literal offsets); nothing in here knows about a specific schema.

import Foundation

/// Envelope header: `key:u32 | fixed_len:u16 | version:u8 | flags:u8`, little-endian.
public let MESSAGE_HEADER_LEN = 8

/// High bit of an array/map slot's length: elements are self-contained instances behind slots.
public let SHAPE_BIT: UInt32 = 0x8000_0000
/// Count bits of an array/map slot's length.
public let LEN_MASK: UInt32 = 0x7FFF_FFFF
/// Header version passed to instance readers: no extension version is promised inside an instance.
public let INSTANCE_HV: UInt8 = 255

/// Strict-check errors. Leaves describe what is wrong; `nested` wraps them down to the field
/// (and element) where the problem was found, e.g. "at `edges`[0]: at `children`[1]: invalid utf-8".
public indirect enum ValidationError: Error, Equatable, CustomStringConvertible {
    case tooShort(need: Int, got: Int)
    case keyMismatch(expected: UInt32, got: UInt32)
    case fixedLenMismatch(expected: Int, got: Int)
    case extBlock(version: UInt16)
    case slotOutOfBounds(off: Int, len: Int, payload: Int)
    case invalidUtf8
    case unknownEnumValue(type: String, value: Int32)
    case nested(field: String, index: Int?, ValidationError)

    public var description: String {
        switch self {
        case let .tooShort(need, got): return "buffer too short: need \(need) bytes, got \(got)"
        case let .keyMismatch(expected, got): return "key mismatch: expected \(expected), got \(got)"
        case let .fixedLenMismatch(expected, got): return "header fixed_len mismatch: expected \(expected), got \(got)"
        case let .extBlock(version): return "extension block v\(version) missing or malformed"
        case let .slotOutOfBounds(off, len, payload): return "slot out of bounds (off=\(off) len=\(len), payload=\(payload))"
        case .invalidUtf8: return "invalid utf-8"
        case let .unknownEnumValue(type, value): return "unknown \(type) value \(value)"
        case let .nested(field, index, inner):
            if let index { return "at `\(field)`[\(index)]: \(inner)" }
            return "at `\(field)`: \(inner)"
        }
    }
}

/// A decoded buffer: the bytes are copied once into memory owned by this object, and every
/// read afterwards is a plain little-endian load from `payload` (the bytes after the header).
/// Reads out of bounds trap (like Rust); call `validate` first on untrusted input.
public final class RawMessage: @unchecked Sendable {
    public let key: UInt32
    public let fixedLen: Int
    public let version: UInt8
    public let flags: UInt8
    /// Payload view (header excluded); all offsets in generated code are relative to it.
    let payload: UnsafeRawBufferPointer
    private let storage: UnsafeMutableRawPointer

    /// Lazy decode: nil when the buffer is too short for the header plus the fixed region, or
    /// when the key does not match. Nothing else is checked.
    public convenience init?(decoding data: Data, key: UInt32, fixedLen: Int) {
        guard data.count >= MESSAGE_HEADER_LEN + fixedLen else { return nil }
        self.init(copying: data)
        guard self.key == key else { return nil }
    }

    /// Strict decode: every header field must match the compiled schema.
    public convenience init(validating data: Data, key: UInt32, fixedLen: Int) throws {
        let need = MESSAGE_HEADER_LEN + fixedLen
        if data.count < need { throw ValidationError.tooShort(need: need, got: data.count) }
        self.init(copying: data)
        if self.key != key { throw ValidationError.keyMismatch(expected: key, got: self.key) }
        if self.fixedLen != fixedLen { throw ValidationError.fixedLenMismatch(expected: fixedLen, got: self.fixedLen) }
    }

    private init(copying data: Data) {
        let count = data.count
        let storage = UnsafeMutableRawPointer.allocate(byteCount: max(count, MESSAGE_HEADER_LEN), alignment: 8)
        data.withUnsafeBytes { raw in
            if let base = raw.baseAddress { storage.copyMemory(from: base, byteCount: count) }
        }
        self.storage = storage
        self.key = storage.loadUnaligned(as: UInt32.self).littleEndian
        self.fixedLen = Int(storage.loadUnaligned(fromByteOffset: 4, as: UInt16.self).littleEndian)
        self.version = storage.loadUnaligned(fromByteOffset: 6, as: UInt8.self)
        self.flags = storage.loadUnaligned(fromByteOffset: 7, as: UInt8.self)
        self.payload = UnsafeRawBufferPointer(start: count > MESSAGE_HEADER_LEN ? storage + MESSAGE_HEADER_LEN : storage, count: count > MESSAGE_HEADER_LEN ? count - MESSAGE_HEADER_LEN : 0)
    }

    deinit { storage.deallocate() }

    /// Total buffer length (header included).
    public var count: Int { MESSAGE_HEADER_LEN + payload.count }

    /// Payload length (header excluded): the extent of top-level readers and open elements.
    public var payloadCount: Int { payload.count }

    // ---- Fixed-size little-endian reads (offsets are payload-relative) ----

    @inline(__always) func bound(_ off: Int, _ n: Int) {
        precondition(off >= 0 && off + n <= payload.count, "message read out of bounds: off=\(off) n=\(n) payload=\(payload.count)")
    }
    @inline(__always) public func readU8(_ off: Int) -> UInt8 { bound(off, 1); return payload[off] }
    @inline(__always) public func readI8(_ off: Int) -> Int8 { Int8(bitPattern: readU8(off)) }
    @inline(__always) public func readBool(_ off: Int) -> Bool { readU8(off) != 0 }
    @inline(__always) public func readU16(_ off: Int) -> UInt16 { bound(off, 2); return payload.loadUnaligned(fromByteOffset: off, as: UInt16.self).littleEndian }
    @inline(__always) public func readI16(_ off: Int) -> Int16 { Int16(bitPattern: readU16(off)) }
    @inline(__always) public func readU32(_ off: Int) -> UInt32 { bound(off, 4); return payload.loadUnaligned(fromByteOffset: off, as: UInt32.self).littleEndian }
    @inline(__always) public func readI32(_ off: Int) -> Int32 { Int32(bitPattern: readU32(off)) }
    @inline(__always) public func readU64(_ off: Int) -> UInt64 { bound(off, 8); return payload.loadUnaligned(fromByteOffset: off, as: UInt64.self).littleEndian }
    @inline(__always) public func readI64(_ off: Int) -> Int64 { Int64(bitPattern: readU64(off)) }
    @inline(__always) public func readF32(_ off: Int) -> Float { Float(bitPattern: readU32(off)) }
    @inline(__always) public func readF64(_ off: Int) -> Double { Double(bitPattern: readU64(off)) }

    /// Reference slot at `off`: `(payload-relative offset, length)`.
    @inline(__always) public func readSlot(_ off: Int) -> (UInt32, UInt32) { (readU32(off), readU32(off + 4)) }

    @inline(__always) public func bitmapBit(_ block: Int, _ bit: Int) -> Bool {
        (readU8(block + (bit >> 3)) >> UInt8(bit & 7)) & 1 == 1
    }

    /// UTF-8 string slot (invalid sequences are replaced; `validate` rejects them).
    public func readString(_ off: Int) -> String {
        let (o, l) = readSlot(off)
        return String(decoding: slice(Int(o), Int(l)), as: UTF8.self)
    }

    /// Bytes slot, copied.
    public func readBytes(_ off: Int) -> Data {
        let (o, l) = readSlot(off)
        return Data(slice(Int(o), Int(l)))
    }

    func slice(_ off: Int, _ len: Int) -> UnsafeRawBufferPointer {
        let payloadLen = payload.count
        precondition(off >= 0 && len >= 0 && off + len <= payloadLen, "slot out of bounds: off=\(off) len=\(len) payload=\(payloadLen)")
        return UnsafeRawBufferPointer(rebasing: payload[off..<off + len])
    }

    /// Locate an extension block by walking back from `hi` (default: the payload end; the instance
    /// end for self-contained structs), never crossing below `lo`; returns its payload start
    /// (trailer excluded). Blocks newer than `target` are stepped over even when this reader does
    /// not know them. A block shorter than `minLen` (its fixed region) counts as absent. Instance
    /// readers pass `hv = INSTANCE_HV` (no version is promised inside an instance).
    public func findExtBlock(_ versions: [UInt16], _ hv: UInt8, _ target: UInt16, _ minLen: Int, lo: Int = 0, hi: Int? = nil) -> Int? {
        let maxKnown = versions.last ?? 0
        var pos = hi ?? payload.count
        while pos >= lo + 6 {
            let since = readU16(pos - 6)
            let blen = Int(readU32(pos - 4))
            if since < 2 || since > UInt16(hv) || blen < 6 || blen > pos - lo { return nil }
            let start = pos - blen
            if since == target {
                return pos - 6 - start >= minLen ? start : nil
            }
            if since > target && (since > maxKnown || versions.contains(since)) {
                pos = start
                continue
            }
            return nil
        }
        return nil
    }
}

// ---- Protocols the generated code conforms to ----

/// i32-backed enum (`enum Color: Int32, MessageEnum`).
public protocol MessageEnum: RawRepresentable, Hashable where RawValue == Int32 {}

/// Fixed-width value readable from one position (scalars and enums), for `ScalarListView`.
public protocol MessageScalar {
    static var width: Int { get }
    static func read(_ m: RawMessage, _ off: Int) -> Self
}

extension UInt8: MessageScalar { public static var width: Int { 1 }; public static func read(_ m: RawMessage, _ off: Int) -> UInt8 { m.readU8(off) } }
extension Int8: MessageScalar { public static var width: Int { 1 }; public static func read(_ m: RawMessage, _ off: Int) -> Int8 { m.readI8(off) } }
extension Bool: MessageScalar { public static var width: Int { 1 }; public static func read(_ m: RawMessage, _ off: Int) -> Bool { m.readBool(off) } }
extension UInt16: MessageScalar { public static var width: Int { 2 }; public static func read(_ m: RawMessage, _ off: Int) -> UInt16 { m.readU16(off) } }
extension Int16: MessageScalar { public static var width: Int { 2 }; public static func read(_ m: RawMessage, _ off: Int) -> Int16 { m.readI16(off) } }
extension UInt32: MessageScalar { public static var width: Int { 4 }; public static func read(_ m: RawMessage, _ off: Int) -> UInt32 { m.readU32(off) } }
extension Int32: MessageScalar { public static var width: Int { 4 }; public static func read(_ m: RawMessage, _ off: Int) -> Int32 { m.readI32(off) } }
extension UInt64: MessageScalar { public static var width: Int { 8 }; public static func read(_ m: RawMessage, _ off: Int) -> UInt64 { m.readU64(off) } }
extension Int64: MessageScalar { public static var width: Int { 8 }; public static func read(_ m: RawMessage, _ off: Int) -> Int64 { m.readI64(off) } }
extension Float: MessageScalar { public static var width: Int { 4 }; public static func read(_ m: RawMessage, _ off: Int) -> Float { m.readF32(off) } }
extension Double: MessageScalar { public static var width: Int { 8 }; public static func read(_ m: RawMessage, _ off: Int) -> Double { m.readF64(off) } }

/// Zero-copy struct view over `[origin, end)` of the payload (the payload for top-level messages
/// and open elements, the instance for self-contained structs), for `StructEvoListView`.
public protocol MessageReader {
    static var fixedLen: Int { get }
    init(msg: RawMessage, origin: Int, end: Int, hv: UInt8)
}

// ---- Lazy views ----

public struct ScalarListView<T: MessageScalar>: Sequence {
    let msg: RawMessage
    public let start: Int
    public let count: Int
    let stride: Int
    public init(msg: RawMessage, start: Int, count: Int, stride: Int) {
        self.msg = msg; self.start = start; self.count = count; self.stride = stride
    }
    public subscript(i: Int) -> T { T.read(msg, start + i * stride) }
    public struct Iterator: IteratorProtocol {
        let view: ScalarListView<T>
        var i = 0
        public mutating func next() -> T? { guard i < view.count else { return nil }; defer { i += 1 }; return view[i] }
    }
    public func makeIterator() -> Iterator { Iterator(view: self) }
}

public struct StringListView: Sequence {
    let msg: RawMessage
    public let start: Int
    public let count: Int
    public init(msg: RawMessage, start: Int, count: Int) { self.msg = msg; self.start = start; self.count = count }
    public subscript(i: Int) -> String { msg.readString(start + i * 8) }
    public struct Iterator: IteratorProtocol {
        let view: StringListView
        var i = 0
        public mutating func next() -> String? { guard i < view.count else { return nil }; defer { i += 1 }; return view[i] }
    }
    public func makeIterator() -> Iterator { Iterator(view: self) }
}

public struct BytesListView: Sequence {
    let msg: RawMessage
    public let start: Int
    public let count: Int
    public init(msg: RawMessage, start: Int, count: Int) { self.msg = msg; self.start = start; self.count = count }
    public subscript(i: Int) -> Data { msg.readBytes(start + i * 8) }
    public struct Iterator: IteratorProtocol {
        let view: BytesListView
        var i = 0
        public mutating func next() -> Data? { guard i < view.count else { return nil }; defer { i += 1 }; return view[i] }
    }
    public func makeIterator() -> Iterator { Iterator(view: self) }
}

/// Raw slot view for nested dynamic arrays: each element is an 8-byte slot.
public struct SlotListView: Sequence {
    let msg: RawMessage
    public let start: Int
    public let count: Int
    public init(msg: RawMessage, start: Int, count: Int) { self.msg = msg; self.start = start; self.count = count }
    public func slot(at i: Int) -> (UInt32, UInt32) { msg.readSlot(start + i * 8) }
    public struct Iterator: IteratorProtocol {
        let view: SlotListView
        var i = 0
        public mutating func next() -> (UInt32, UInt32)? { guard i < view.count else { return nil }; defer { i += 1 }; return view.slot(at: i) }
    }
    public func makeIterator() -> Iterator { Iterator(view: self) }
}

/// Struct elements of an array region whose slot length carries the shape bit. shape=0: open
/// fixed-region footprints (readers see the payload as their extent and hv=0, so extension fields
/// are absent); shape=1: 8-byte slots, each pointing at a self-contained instance whose extent
/// bounds the reader (hv = INSTANCE_HV).
public struct StructEvoListView<T: MessageReader>: Sequence {
    let msg: RawMessage
    public let start: Int
    let rawLen: UInt32
    public init(msg: RawMessage, start: Int, rawLen: UInt32) {
        self.msg = msg; self.start = start; self.rawLen = rawLen
    }
    public var count: Int { Int(rawLen & LEN_MASK) }
    /// Whether the writer emitted per-element instances (its element type had extension fields).
    public var isInstanceMode: Bool { rawLen & SHAPE_BIT != 0 }
    public subscript(i: Int) -> T {
        precondition(i >= 0 && i < count, "index \(i) out of bounds (len \(count))")
        if isInstanceMode {
            let (o, l) = msg.readSlot(start + i * 8)
            return T(msg: msg, origin: Int(o), end: Int(o) + Int(l), hv: INSTANCE_HV)
        }
        return T(msg: msg, origin: start + i * T.fixedLen, end: msg.payloadCount, hv: 0)
    }
    public struct Iterator: IteratorProtocol {
        let view: StructEvoListView<T>
        var i = 0
        public mutating func next() -> T? { guard i < view.count else { return nil }; defer { i += 1 }; return view[i] }
    }
    public func makeIterator() -> Iterator { Iterator(view: self) }
}

// ---- Encoder ----

/// Growable little-endian byte buffer. Positions are absolute (the 8-byte header included);
/// `rel()` turns them into the payload-relative values slots carry.
public struct Writer {
    var bytes: [UInt8]
    public init(capacity: Int = 64) {
        bytes = []
        bytes.reserveCapacity(max(capacity, 16))
    }

    /// Reserve `n` zeroed bytes, returning the absolute start.
    @discardableResult public mutating func reserve(_ n: Int) -> Int {
        let start = bytes.count
        bytes.append(contentsOf: repeatElement(0, count: n))
        return start
    }

    /// Payload-relative offset of an absolute position.
    public func rel(_ abs: Int) -> Int { abs - MESSAGE_HEADER_LEN }

    /// Bytes written so far (absolute; the header included).
    public var length: Int { bytes.count }

    public mutating func writeU8(_ pos: Int, _ v: UInt8) { bytes[pos] = v }
    public mutating func writeBool(_ pos: Int, _ v: Bool) { bytes[pos] = v ? 1 : 0 }
    public mutating func writeU16(_ pos: Int, _ v: UInt16) {
        let le = v.littleEndian
        withUnsafeBytes(of: le) { bytes.replaceSubrange(pos..<pos + 2, with: $0) }
    }
    public mutating func writeU32(_ pos: Int, _ v: UInt32) {
        let le = v.littleEndian
        withUnsafeBytes(of: le) { bytes.replaceSubrange(pos..<pos + 4, with: $0) }
    }
    public mutating func writeI32(_ pos: Int, _ v: Int32) { writeU32(pos, UInt32(bitPattern: v)) }
    public mutating func writeU64(_ pos: Int, _ v: UInt64) {
        let le = v.littleEndian
        withUnsafeBytes(of: le) { bytes.replaceSubrange(pos..<pos + 8, with: $0) }
    }
    public mutating func writeI64(_ pos: Int, _ v: Int64) { writeU64(pos, UInt64(bitPattern: v)) }
    public mutating func writeF32(_ pos: Int, _ v: Float) { writeU32(pos, v.bitPattern) }
    public mutating func writeF64(_ pos: Int, _ v: Double) { writeU64(pos, v.bitPattern) }
    public mutating func writeSlot(_ pos: Int, _ off: Int, _ len: Int) {
        writeU32(pos, UInt32(off))
        writeU32(pos + 4, UInt32(len))
    }
    public mutating func setBit(_ block: Int, _ bit: Int) { bytes[block + (bit >> 3)] |= 1 << UInt8(bit & 7) }

    /// Append the UTF-8 bytes of `s` and point the slot at `pos` at them.
    public mutating func writeStringSlot(_ pos: Int, _ s: String) {
        let off = rel(bytes.count)
        var n = 0
        for b in s.utf8 { bytes.append(b); n += 1 }
        writeSlot(pos, off, n)
    }

    /// Append `d` and point the slot at `pos` at it.
    public mutating func writeDataSlot(_ pos: Int, _ d: Data) {
        let off = rel(bytes.count)
        bytes.append(contentsOf: d)
        writeSlot(pos, off, d.count)
    }

    /// Append the `since | block_len` trailer of the block starting at `block` (absolute).
    public mutating func writeTrailer(_ block: Int, _ version: UInt16) {
        let t = reserve(6)
        writeU16(t, version)
        writeU32(t + 2, UInt32(bytes.count - block))
    }

    /// Exact-size copy of everything written.
    public func finish() -> Data { Data(bytes) }
}

// ---- Strict validation ----

/// Wrap errors thrown by `body` as having been found inside `field` (element `index`).
public func nest(_ field: String, _ index: Int? = nil, _ body: () throws -> Void) throws {
    do {
        try body()
    } catch let e as ValidationError {
        throw ValidationError.nested(field: field, index: index, e)
    }
}

/// The slot at `pos` must stay inside the payload; returns (offset, length).
@discardableResult public func checkBytesSlot(_ m: RawMessage, _ pos: Int) throws -> (Int, Int) {
    let (o, l) = m.readSlot(pos)
    if Int(o) + Int(l) > m.payload.count { throw ValidationError.slotOutOfBounds(off: Int(o), len: Int(l), payload: m.payload.count) }
    return (Int(o), Int(l))
}

public func checkStrSlot(_ m: RawMessage, _ pos: Int) throws {
    let (o, l) = try checkBytesSlot(m, pos)
    let bytes = m.slice(o, l)
    if #available(macOS 15.0, iOS 18.0, *) {
        if String(validating: bytes, as: UTF8.self) == nil { throw ValidationError.invalidUtf8 }
    } else {
        // Lossy decoding replaces invalid sequences with U+FFFD, so the round trip differs
        // exactly when the input was not valid UTF-8 (a literal U+FFFD in valid input round-trips).
        if !String(decoding: bytes, as: UTF8.self).utf8.elementsEqual(bytes) { throw ValidationError.invalidUtf8 }
    }
}

/// The slot at `pos` holds an element count of `stride`-sized elements; returns (offset, count).
@discardableResult public func checkRegion(_ m: RawMessage, _ pos: Int, _ stride: Int) throws -> (Int, Int) {
    let (o, raw) = m.readSlot(pos)
    let n = Int(raw & LEN_MASK)
    let (size, overflow1) = n.multipliedReportingOverflow(by: stride)
    let (end, overflow2) = Int(o).addingReportingOverflow(size)
    if !overflow1 && !overflow2 && end <= m.payload.count { return (Int(o), n) }
    throw ValidationError.slotOutOfBounds(off: Int(o), len: Int(raw), payload: m.payload.count)
}

/// Array/map region slot split into (offset, count, shape); the extent is checked by `checkExtent`.
public func checkRegionShape(_ m: RawMessage, _ pos: Int, _ field: String, _ index: Int? = nil) throws -> (Int, Int, Bool) {
    let (o, raw) = m.readSlot(pos)
    let n = Int(raw & LEN_MASK)
    if Int(o) + n > m.payload.count {
        throw ValidationError.nested(field: field, index: index, .slotOutOfBounds(off: Int(o), len: Int(raw), payload: m.payload.count))
    }
    return (Int(o), n, raw & SHAPE_BIT != 0)
}

/// `count` elements of `stride` bytes starting at `off` must lie inside the payload.
public func checkExtent(_ m: RawMessage, _ off: Int, _ count: Int, _ stride: Int, _ field: String, _ index: Int? = nil) throws {
    let (size, overflow1) = count.multipliedReportingOverflow(by: stride)
    let (end, overflow2) = off.addingReportingOverflow(size)
    if overflow1 || overflow2 || end > m.payload.count {
        throw ValidationError.nested(field: field, index: index, .slotOutOfBounds(off: off, len: count, payload: m.payload.count))
    }
}

/// The instance slot at `pos` must stay inside the payload; returns its [lo, hi) extent.
public func checkInstance(_ m: RawMessage, _ pos: Int) throws -> (Int, Int) {
    let (o, l) = try checkBytesSlot(m, pos)
    return (o, o + l)
}

/// Same, with a region failure located at `field` (element `index`) like the element checks are.
@discardableResult public func checkRegion(_ m: RawMessage, _ pos: Int, _ stride: Int, _ field: String, _ index: Int? = nil) throws -> (Int, Int) {
    do {
        return try checkRegion(m, pos, stride)
    } catch let e as ValidationError {
        throw ValidationError.nested(field: field, index: index, e)
    }
}

/// The i32 at `pos` must be a known variant of `E` (pass `Color.self`).
public func checkEnum<E: MessageEnum>(_ type: E.Type, _ m: RawMessage, _ pos: Int) throws {
    let v = m.readI32(pos)
    if E(rawValue: v) == nil { throw ValidationError.unknownEnumValue(type: String(describing: E.self), value: v) }
}

/// Locate an extension block the writer must have written (version <= header version).
public func checkExtBlock(_ m: RawMessage, _ versions: [UInt16], _ hv: UInt8, _ target: UInt16, _ minLen: Int) throws -> Int {
    guard let bs = m.findExtBlock(versions, hv, target, minLen) else { throw ValidationError.extBlock(version: target) }
    return bs
}

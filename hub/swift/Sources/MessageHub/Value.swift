import Foundation

/// How a hub payload decodes. Raw values mirror `message_hub::ValueKind` (stable u8
/// discriminants); byte layouts follow the message-rs wire conventions: little-endian scalars,
/// `bool` one byte, `String` raw UTF-8.
public enum ValueKind: UInt8 {
    case bool = 0
    case u8 = 1
    case i8 = 2
    case u16 = 3
    case i16 = 4
    case u32 = 5
    case i32 = 6
    case u64 = 7
    case i64 = 8
    case f32 = 9
    case f64 = 10
    case string = 11
    case bytes = 12
    /// A message-rs encoding; the item id is the message key.
    case message = 13
}

/// A decoded hub value (the mirror of `message_hub::Value`).
public enum Value {
    case bool(Bool)
    case u8(UInt8)
    case i8(Int8)
    case u16(UInt16)
    case i16(Int16)
    case u32(UInt32)
    case i32(Int32)
    case u64(UInt64)
    case i64(Int64)
    case f32(Float)
    case f64(Double)
    case string(String)
    case bytes([UInt8])
    case message([UInt8])

    public var kind: ValueKind {
        switch self {
        case .bool: return .bool
        case .u8: return .u8
        case .i8: return .i8
        case .u16: return .u16
        case .i16: return .i16
        case .u32: return .u32
        case .i32: return .i32
        case .u64: return .u64
        case .i64: return .i64
        case .f32: return .f32
        case .f64: return .f64
        case .string: return .string
        case .bytes: return .bytes
        case .message: return .message
        }
    }

    /// Encodes into the hub's byte layout.
    public func encode(_ kind: ValueKind? = nil) -> [UInt8] {
        let target = kind ?? self.kind
        precondition(target == self.kind, "value \(self.kind) does not match kind \(target)")
        switch self {
        case .bool(let v): return [v ? 1 : 0]
        case .u8(let v): return [v]
        case .i8(let v): return [UInt8(bitPattern: v)]
        case .u16(let v): return withUnsafeBytes(of: v.littleEndian) { Array($0) }
        case .i16(let v): return withUnsafeBytes(of: v.littleEndian) { Array($0) }
        case .u32(let v): return withUnsafeBytes(of: v.littleEndian) { Array($0) }
        case .i32(let v): return withUnsafeBytes(of: v.littleEndian) { Array($0) }
        case .u64(let v): return withUnsafeBytes(of: v.littleEndian) { Array($0) }
        case .i64(let v): return withUnsafeBytes(of: v.littleEndian) { Array($0) }
        case .f32(let v): return withUnsafeBytes(of: v.bitPattern.littleEndian) { Array($0) }
        case .f64(let v): return withUnsafeBytes(of: v.bitPattern.littleEndian) { Array($0) }
        case .string(let s): return Array(s.utf8)
        case .bytes(let b), .message(let b): return b
        }
    }

    /// Decodes a payload of the given kind. Fixed-size kinds reject wrong lengths, strings must
    /// be valid UTF-8; `bytes` / `message` pass through.
    public static func decode(_ kind: ValueKind, _ payload: [UInt8]) throws -> Value {
        func fixed(_ n: Int) throws -> ArraySlice<UInt8> {
            guard payload.count == n else { throw ValueError.kindLength(kind, n, payload.count) }
            return payload[...]
        }
        switch kind {
        case .bool: return .bool(payload.first.map { $0 != 0 } ?? false)
        case .u8: return .u8(try fixed(1).first!)
        case .i8: return .i8(Int8(bitPattern: try fixed(1).first!))
        case .u16: return .u16(try fixed(2).loadLE())
        case .i16: return .i16(Int16(bitPattern: try fixed(2).loadLE()))
        case .u32: return .u32(try fixed(4).loadLE())
        case .i32: return .i32(Int32(bitPattern: try fixed(4).loadLE()))
        case .u64: return .u64(try fixed(8).loadLE())
        case .i64: return .i64(Int64(bitPattern: try fixed(8).loadLE()))
        case .f32: return .f32(Float(bitPattern: try fixed(4).loadLE()))
        case .f64: return .f64(Double(bitPattern: try fixed(8).loadLE()))
        case .string:
            guard let s = String(bytes: payload, encoding: .utf8) else {
                throw ValueError.invalidUtf8
            }
            return .string(s)
        case .bytes: return .bytes(payload)
        case .message: return .message(payload)
        }
    }
}

public struct ValueError: Error, Equatable {
    let reason: String
    init(_ reason: String) { self.reason = reason }

    /// A fixed-size kind received the wrong payload length.
    public static func kindLength(_ kind: ValueKind, _ expected: Int, _ got: Int) -> ValueError {
        ValueError("\(kind) needs \(expected) bytes, got \(got)")
    }
    public static let invalidUtf8 = ValueError("string payload is not valid utf-8")
}

private extension ArraySlice where Element == UInt8 {
    /// Little-endian load of an n-byte slice into an integer of matching width.
    func loadLE<T: FixedWidthInteger>() -> T {
        var v: T = 0
        for (i, b) in enumerated() {
            v |= T(truncatingIfNeeded: b) << (8 * i)
        }
        return v
    }
}

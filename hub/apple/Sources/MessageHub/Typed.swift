import Foundation
#if canImport(MessageRuntime)
import MessageRuntime
#endif

/// The one contract every publishable/pullable value conforms to: basic Swift types and
/// `MessageBytes` below, and every `MessageCodec` type (generated structs included) via the
/// blanket conformance at the bottom of this file. Conforming a new type is implementing
/// `MessageCodec` — nothing here needs to change.
public protocol HubValue {
    static var hubKind: ValueKind { get }
    func encode() -> Data
    static func fromBytes(_ data: Data) -> Self?
}

/// Basic types share one implementation: the byte layout lives in `Value.encode`/`decode`
/// (already written for the raw `Publisher.publish(_ value: Value)` path); each type only picks
/// its case, so there is no second copy of the little-endian/UTF-8 rules to drift out of sync.
extension Bool: HubValue {
    public static var hubKind: ValueKind { .bool }
    public func encode() -> Data { Data(Value.bool(self).encode()) }
    public static func fromBytes(_ data: Data) -> Bool? {
        guard case .bool(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension UInt8: HubValue {
    public static var hubKind: ValueKind { .u8 }
    public func encode() -> Data { Data(Value.u8(self).encode()) }
    public static func fromBytes(_ data: Data) -> UInt8? {
        guard case .u8(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension Int8: HubValue {
    public static var hubKind: ValueKind { .i8 }
    public func encode() -> Data { Data(Value.i8(self).encode()) }
    public static func fromBytes(_ data: Data) -> Int8? {
        guard case .i8(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension UInt16: HubValue {
    public static var hubKind: ValueKind { .u16 }
    public func encode() -> Data { Data(Value.u16(self).encode()) }
    public static func fromBytes(_ data: Data) -> UInt16? {
        guard case .u16(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension Int16: HubValue {
    public static var hubKind: ValueKind { .i16 }
    public func encode() -> Data { Data(Value.i16(self).encode()) }
    public static func fromBytes(_ data: Data) -> Int16? {
        guard case .i16(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension UInt32: HubValue {
    public static var hubKind: ValueKind { .u32 }
    public func encode() -> Data { Data(Value.u32(self).encode()) }
    public static func fromBytes(_ data: Data) -> UInt32? {
        guard case .u32(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension Int32: HubValue {
    public static var hubKind: ValueKind { .i32 }
    public func encode() -> Data { Data(Value.i32(self).encode()) }
    public static func fromBytes(_ data: Data) -> Int32? {
        guard case .i32(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension UInt64: HubValue {
    public static var hubKind: ValueKind { .u64 }
    public func encode() -> Data { Data(Value.u64(self).encode()) }
    public static func fromBytes(_ data: Data) -> UInt64? {
        guard case .u64(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension Int64: HubValue {
    public static var hubKind: ValueKind { .i64 }
    public func encode() -> Data { Data(Value.i64(self).encode()) }
    public static func fromBytes(_ data: Data) -> Int64? {
        guard case .i64(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension Float: HubValue {
    public static var hubKind: ValueKind { .f32 }
    public func encode() -> Data { Data(Value.f32(self).encode()) }
    public static func fromBytes(_ data: Data) -> Float? {
        guard case .f32(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension Double: HubValue {
    public static var hubKind: ValueKind { .f64 }
    public func encode() -> Data { Data(Value.f64(self).encode()) }
    public static func fromBytes(_ data: Data) -> Double? {
        guard case .f64(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}
extension String: HubValue {
    public static var hubKind: ValueKind { .string }
    public func encode() -> Data { Data(Value.string(self).encode()) }
    public static func fromBytes(_ data: Data) -> String? {
        guard case .string(let v) = try? Value.decode(hubKind, Array(data)) else { return nil }
        return v
    }
}

/// Raw bytes. Use `MessageBytes` (or any `MessageCodec` type) for a message-rs payload.
extension Array: HubValue where Element == UInt8 {
    public static var hubKind: ValueKind { .bytes }
    public func encode() -> Data { Data(self) }
    public static func fromBytes(_ data: Data) -> [UInt8]? { Array(data) }
}

/// A message-rs payload without a schema on hand: the id is the message key, decode it yourself
/// with the generated `XxxReader`. Prefer publishing the generated struct directly (below).
public struct MessageBytes: HubValue {
    public let bytes: [UInt8]
    public init(_ bytes: [UInt8]) { self.bytes = bytes }
    public static var hubKind: ValueKind { .message }
    public func encode() -> Data { Data(bytes) }
    public static func fromBytes(_ data: Data) -> MessageBytes? { MessageBytes(Array(data)) }
}

/// The wire kind of every `MessageCodec` type — generated structs included. `Hub.publish`/`pull`
/// have a `MessageCodec`-constrained overload alongside the `HubValue` one (see Hub.swift):
/// `encode()`/`fromBytes(_:)` are already exactly what both protocols ask for, so a generated
/// struct needs no per-type wiring here — Swift can't declare "every MessageCodec conforms to
/// HubValue" across modules, but it can accept `some MessageCodec` directly, which reads the same
/// at the call site: `publish(id:, user)` / `pull(id:) as User?`.
extension MessageCodec {
    public static var hubKind: ValueKind { .message }
}

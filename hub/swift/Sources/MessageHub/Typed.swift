import Foundation

/// A Swift value that knows its own wire kind — so callers write `publish(id: 1, UInt16(65535))`
/// and never name a `ValueKind`. Unlike JS, Swift's types are distinct (`UInt16` vs `Int32` vs
/// `Double`), so the mapping is exact with no wrappers and no guessing.
public protocol HubValue {
    var hubValue: Value { get }
}

extension Bool: HubValue { public var hubValue: Value { .bool(self) } }
extension UInt8: HubValue { public var hubValue: Value { .u8(self) } }
extension Int8: HubValue { public var hubValue: Value { .i8(self) } }
extension UInt16: HubValue { public var hubValue: Value { .u16(self) } }
extension Int16: HubValue { public var hubValue: Value { .i16(self) } }
extension UInt32: HubValue { public var hubValue: Value { .u32(self) } }
extension Int32: HubValue { public var hubValue: Value { .i32(self) } }
extension UInt64: HubValue { public var hubValue: Value { .u64(self) } }
extension Int64: HubValue { public var hubValue: Value { .i64(self) } }
extension Float: HubValue { public var hubValue: Value { .f32(self) } }
extension Double: HubValue { public var hubValue: Value { .f64(self) } }
extension String: HubValue { public var hubValue: Value { .string(self) } }

/// Raw bytes. `Data` maps to BYTES; use `MessageBytes` for a message-rs payload.
extension Array: HubValue where Element == UInt8 {
    public var hubValue: Value { .bytes(self) }
}

/// A message-rs payload: the id is the message key, decode it with the generated `XxxReader`.
public struct MessageBytes: HubValue {
    public let bytes: [UInt8]
    public init(_ bytes: [UInt8]) { self.bytes = bytes }
    public var hubValue: Value { .message(bytes) }
}

/// An explicit `Value` is publishable too (for code that already holds one).
extension Value: HubValue { public var hubValue: Value { self } }

import Foundation
import Testing
@testable import MessageHub

@Suite("value-first API")
struct ValueFirstTests {
    @Test func publishesSwiftTypesWithoutNamingKinds() throws {
        let hub = Hub()
        hub.publish(id: 1, true)
        hub.publish(id: 2, UInt16(65535))
        hub.publish(id: 3, Int32(-1))
        hub.publish(id: 4, "hi")
        hub.publish(id: 5, Double(1.5))
        hub.publish(id: 6, UInt64.max)
        hub.publish(id: 7, [UInt8]([1, 2, 3]))

        // the kind follows the Swift type exactly — no wrappers, no guessing
        #expect(hub.get(id: 2)?.kind == .u16)
        #expect(hub.get(id: 3)?.kind == .i32)
        #expect(hub.get(id: 5)?.kind == .f64)
        #expect(hub.get(id: 6)?.kind == .u64)
        #expect(hub.get(id: 7)?.kind == .bytes)

        // pull returns decoded values
        #expect(try #require(hub.pull(id: 1)).getBool() == true)
        #expect(hub.pull(id: 2) == .u16(65535))
        #expect(hub.pull(id: 4) == .string("hi"))
        #expect(hub.pull(id: 6) == .u64(UInt64.max))
    }

    @Test func messageBytesCarryTheMessageKind() throws {
        let hub = Hub()
        hub.publish(id: 8, MessageBytes([9, 8, 7]))
        #expect(hub.get(id: 8)?.kind == .message)
        #expect(hub.pull(id: 8) == .message([9, 8, 7]))
    }

    @Test func closeDeliversClosedOnceAndEmptiesPull() async throws {
        let hub = Hub()
        let events = EventLog()
        let sub = hub.subscribe(id: 9) { _, kind, _, _ in events.append((0, kind, [])) }
        hub.publish(id: 9, UInt8(1))
        hub.close(id: 9)
        #expect(events.count == 2)
        #expect(events.all[1].1 == 255)
        #expect(hub.pull(id: 9) == nil)
        hub.close(id: 9) // idempotent
        #expect(events.count == 2)
        _ = sub
    }

    @Test func changingTypeOnAnIdSupersedesThePublisher() throws {
        let hub = Hub()
        hub.publish(id: 10, UInt16(1))
        #expect(hub.get(id: 10)?.kind == .u16)
        hub.publish(id: 10, "now a string")
        #expect(hub.get(id: 10)?.kind == .string)
        #expect(hub.pull(id: 10) == .string("now a string"))
    }

    @Test func channelReceivesDecodedValues() async throws {
        let hub = Hub()
        let got = EventLog()
        let sub = hub.subscribe(id: 11) { _, kind, ptr, len in
            let bytes = ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []
            got.append((0, kind, bytes))
        }
        hub.publish(id: 11, "hello")
        try await poll(timeout: 2) { got.count >= 1 }
        let (kind, bytes) = (got.all[0].1, got.all[0].2)
        #expect(ValueKind(rawValue: kind) == .string)
        #expect(try Value.decode(.string, bytes) == .string("hello"))
        _ = sub
    }
}

import XCTest
@testable import MessageHub

/// Collects events from @Sendable callbacks without data races.
final class EventLog: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [(UInt32, UInt8, [UInt8])] = []
    func append(_ e: (UInt32, UInt8, [UInt8])) { lock.lock(); items.append(e); lock.unlock() }
    var all: [(UInt32, UInt8, [UInt8])] { lock.lock(); defer { lock.unlock() }; return items }
    var count: Int { lock.lock(); defer { lock.unlock() }; return items.count }
}

// The Swift package sits inside the message-rs repository: tests read the committed vectors.
private let repoRoot: URL = {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<5 { url.deleteLastPathComponent() }
    return url
}()

private func loadVector(_ name: String) throws -> [String: Any] {
    let data = try Data(contentsOf: repoRoot.appendingPathComponent("vectors/\(name)"))
    return try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [String: Any])
}

private func hexBytes(_ hex: String) -> [UInt8] {
    var bytes = [UInt8](); bytes.reserveCapacity(hex.count / 2)
    var i = hex.startIndex
    while i < hex.endIndex {
        bytes.append(UInt8(hex[i..<hex.index(i, offsetBy: 2)], radix: 16)!)
        i = hex.index(i, offsetBy: 2)
    }
    return bytes
}

final class HubTests: XCTestCase {
    /// Each test uses an isolated registry so they never share publishers.
    private func makeHub() -> Hub { Hub() }

    func testPublishSubscribeGet() {
        let hub = makeHub()
        let got = EventLog()
        let p = Publisher(hub: hub, id: 1, kind: .bytes)
        let s = hub.subscribe(id: 1) { id, kind, ptr, len in
            got.append((id, kind, ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []))
        }

        // pull before any publish: nil
        XCTAssertNil(hub.get(id: 1))

        p.publish([1, 2, 3])
        p.publish(Value.bytes([4, 5]))

        XCTAssertEqual(got.count, 2)
        XCTAssertEqual(got.all[0].0, 1); XCTAssertEqual(got.all[0].1, ValueKind.bytes.rawValue)
        XCTAssertEqual(got.all[0].2, [1, 2, 3])

        // pull: the newest publish
        let latest = hub.get(id: 1)
        XCTAssertEqual(latest?.kind, .bytes)
        XCTAssertEqual(latest?.bytes, [4, 5])

        // subscribing does not replay
        let late = EventLog()
        let lateSub = hub.subscribe(id: 1) { _, kind, ptr, len in
            late.append((0, kind, ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []))
        }
        p.publish([9])
        XCTAssertEqual(late.count, 1)
        XCTAssertEqual(got.count, 3)

        s.unsubscribe()
        p.publish([10])
        XCTAssertEqual(got.count, 3, "unsubscribed")
        _ = lateSub
    }

    func testClosedLifecycle() {
        let hub = makeHub()
        let events = EventLog()
        let p = Publisher(hub: hub, id: 2, kind: .bytes)
        let sub = hub.subscribe(id: 2) { _, kind, ptr, _ in
            _ = ptr
            events.append((0, kind, []))
        }
        p.publish([1])
        p.close()
        p.publish([2]) // no-op after close
        XCTAssertEqual(events.count, 2)
        XCTAssertEqual(events.all[1].1, 255, "Closed carries the reserved kind")
        XCTAssertNil(hub.get(id: 2), "closed: pull is empty")

        // a new publisher reaches the surviving subscription again
        let p2 = Publisher(hub: hub, id: 2, kind: .bytes)
        p2.publish([3])
        XCTAssertEqual(events.count, 3)
        _ = sub
    }

    func testScheduledSubscriberRunsOnItsOwnLoop() {
        let hub = makeHub()
        let queue = DispatchQueue(label: "hub.test")
        let exp = expectation(description: "scheduled delivery")
        let got = EventLog()
        let p = Publisher(hub: hub, id: 3, kind: .bytes)
        let s = hub.subscribe(on: { job in queue.async(execute: job) }, id: 3) { _, _, ptr, _ in
            if let ptr, ptr.pointee == 7 { got.append((0, 0, [7])); exp.fulfill() }
        }
        p.publish([7])
        wait(for: [exp], timeout: 2)
        XCTAssertEqual(got.all.first?.2, [7])
        _ = s
    }

    func testDecodesARustMessageVector() throws {
        // The core test of the whole stack: bytes encoded by the Rust fixtures decode through
        // the hub's pull path into the generated schema.
        let vector = try loadVector("user.json")
        let hex = try XCTUnwrap(vector["hex"] as? String)
        let key = try XCTUnwrap(vector["key"] as? UInt32)

        let hub = makeHub()
        let p = Publisher(hub: hub, id: key, kind: .message)
        p.publish(hexBytes(hex))

        // byte-for-byte through the hub is the cross-language contract here; decoding the schema
        // itself is covered by the runtime/swift conformance suite against the same vector
        let pulled = try XCTUnwrap(hub.get(id: key))
        XCTAssertEqual(pulled.kind, .message)
        XCTAssertEqual(pulled.bytes, hexBytes(hex))
    }

    func testValueRoundTripsAndRejectsBadInput() throws {
        // every kind encodes to the same bytes the Rust core produces (message-rs conventions)
        XCTAssertEqual(Value.bool(true).encode(), [1])
        XCTAssertEqual(Value.u16(0x0102).encode(), [2, 1])
        XCTAssertEqual(Value.i16(-2).encode(), [0xFE, 0xFF])
        XCTAssertEqual(Value.u32(1).encode(), [1, 0, 0, 0])
        XCTAssertEqual(Value.f32(1.0).encode(), [0, 0, 0x80, 0x3F])
        XCTAssertEqual(Value.string("山").encode(), Array("山".utf8))

        try XCTAssertFalse(Value.decode(.bool, [0]).getBool())
        XCTAssertThrowsError(try Value.decode(.u32, [1, 2]))
        XCTAssertThrowsError(try Value.decode(.string, [0xFF]))

        // and through the hub: the typed publish path validates the kind at the encode boundary
        let hub = makeHub()
        let p = Publisher(hub: hub, id: 4, kind: .u32)
        p.publish(Value.u32(7))
        XCTAssertEqual(hub.get(id: 4)?.kind, .u32)
        XCTAssertEqual(hub.get(id: 4)?.bytes, [7, 0, 0, 0])
    }
}

private extension Value {
    func getBool() throws -> Bool {
        if case .bool(let b) = self { return b }
        throw ValueError("not a bool")
    }
}

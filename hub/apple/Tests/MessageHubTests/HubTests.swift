import Foundation
import Testing
import MessageHub

// The committed vectors are produced by the Rust fixtures (`cargo run -p message-fixtures --bin
// export-schema -- vectors`).
private let repoRoot: URL = {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<5 { url.deleteLastPathComponent() }
    return url
}()

private func loadVector(_ name: String) throws -> [String: Any] {
    let data = try Data(contentsOf: repoRoot.appendingPathComponent("vectors/\(name)"))
    return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
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

@Suite("Hub: push, pull and lifecycle")
struct HubTests {
    private func makeHub() -> Hub { Hub() }

    @Test func handlesKeepTheirRegistryAliveAfterTheHubGoesOutOfScope() {
        // The Rust handles hold only a Weak to the registry, and an isolated Hub frees it in
        // deinit. Publisher/Subscription must retain the Hub themselves, or a hub that goes out
        // of scope while its handles live on turns every later publish into a silent no-op.
        let got = Counter()
        var publisher: Publisher?
        var subscription: Subscription?
        do {
            let hub = Hub()
            subscription = hub.subscribe(id: 80) { _, kind, _, _ in if kind != 255 { got.bump() } }
            publisher = Publisher(hub: hub, id: 80, kind: .string)
        }
        publisher?.publish(Value.string("still alive"))
        #expect(got.value == 1)
        subscription?.unsubscribe()
        publisher?.close()
    }

    @Test func publishSubscribeGet() {
        let hub = makeHub()
        let got = EventLog()
        let p = Publisher(hub: hub, id: 1, kind: .bytes)
        let s = hub.subscribe(id: 1) { id, kind, ptr, len in
            got.append((id, kind, ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []))
        }

        // pull before any publish: nil
        #expect(hub.get(id: 1) == nil)

        p.publish([1, 2, 3])
        p.publish(Value.bytes([4, 5]))

        #expect(got.count == 2)
        #expect(got.all[0].0 == 1)
        #expect(got.all[0].1 == ValueKind.bytes.rawValue)
        #expect(got.all[0].2 == [1, 2, 3])

        // pull: the newest publish
        let latest = hub.get(id: 1)
        #expect(latest?.kind == .bytes)
        #expect(latest?.bytes == [4, 5])

        // subscribing does not replay
        let late = EventLog()
        let lateSub = hub.subscribe(id: 1) { _, kind, ptr, len in
            late.append((0, kind, ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []))
        }
        p.publish([9])
        #expect(late.count == 1)
        #expect(got.count == 3)

        s.unsubscribe()
        p.publish([10])
        #expect(got.count == 3, "unsubscribed")
        _ = lateSub
    }

    @Test func closedLifecycle() {
        let hub = makeHub()
        let events = EventLog()
        let p = Publisher(hub: hub, id: 2, kind: .bytes)
        let sub = hub.subscribe(id: 2) { _, kind, _, _ in
            events.append((0, kind, []))
        }
        p.publish([1])
        p.close()
        p.publish([2]) // no-op after close
        #expect(events.count == 2)
        #expect(events.all[1].1 == 255, "Closed carries the reserved kind")
        #expect(hub.get(id: 2) == nil, "closed: pull is empty")

        // a new publisher reaches the surviving subscription again
        let p2 = Publisher(hub: hub, id: 2, kind: .bytes)
        p2.publish([3])
        #expect(events.count == 3)
        _ = sub
    }

    @Test func scheduledSubscriberRunsOnItsOwnLoop() async throws {
        let hub = makeHub()
        let queue = DispatchQueue(label: "hub.test")
        let got = EventLog()
        let p = Publisher(hub: hub, id: 3, kind: .bytes)
        let s = hub.subscribe(on: { job in queue.async(execute: job) }, id: 3) { _, _, ptr, _ in
            if let ptr, ptr.pointee == 7 { got.append((0, 0, [7])) }
        }
        p.publish([7])
        try await poll(timeout: 2) { got.count >= 1 }
        #expect(got.all.first?.2 == [7])
        _ = s
    }

    @Test func decodesARustMessageVector() throws {
        // byte-for-byte through the hub is the cross-language contract here; decoding the schema
        // itself is covered by the runtime/swift conformance suite against the same vector
        let vector = try loadVector("user.json")
        let hex = try #require(vector["hex"] as? String)
        let key = try #require(vector["key"] as? UInt32)

        let hub = makeHub()
        let p = Publisher(hub: hub, id: key, kind: .message)
        p.publish(hexBytes(hex))

        let pulled = try #require(hub.get(id: key))
        #expect(pulled.kind == .message)
        #expect(pulled.bytes == hexBytes(hex))
    }

    @Test func valueRoundTripsAndRejectsBadInput() throws {
        // every kind encodes to the same bytes the Rust core produces (message-rs conventions)
        #expect(Value.bool(true).encode() == [1])
        #expect(Value.u16(0x0102).encode() == [2, 1])
        #expect(Value.i16(-2).encode() == [0xFE, 0xFF])
        #expect(Value.u32(1).encode() == [1, 0, 0, 0])
        #expect(Value.f32(1.0).encode() == [0, 0, 0x80, 0x3F])
        #expect(Value.string("山").encode() == Array("山".utf8))

        #expect(try Value.decode(.bool, [0]).getBool() == false)
        do { _ = try Value.decode(.u32, [1, 2]); Issue.record("u32 accepted 2 bytes") } catch {}
        do { _ = try Value.decode(.string, [0xFF]); Issue.record("string accepted invalid utf-8") } catch {}

        // and through the hub: the typed publish path validates the kind at the encode boundary
        let hub = makeHub()
        let p = Publisher(hub: hub, id: 4, kind: .u32)
        p.publish(Value.u32(7))
        #expect(hub.get(id: 4)?.kind == .u32)
        #expect(hub.get(id: 4)?.bytes == [7, 0, 0, 0])
    }
}

struct TestTimeout: Error {}


import Combine
import XCTest
@testable import MessageHub

// NOTE: no async XCTest methods in this target for now. On this toolchain (Xcode 26.1.1,
// swift-testing 1400) an async test entry point pulls in the swift-testing runtime, whose
// initialization crashes the process with signal 5; the bodies below run their awaits through
// a bounded detached task instead.

/// Collects events from @Sendable callbacks without data races.
final class EventLog2: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [(UInt32, UInt8, [UInt8])] = []
    func append(_ e: (UInt32, UInt8, [UInt8])) { lock.lock(); items.append(e); lock.unlock() }
    var all: [(UInt32, UInt8, [UInt8])] { lock.lock(); defer { lock.unlock() }; return items }
    var count: Int { lock.lock(); defer { lock.unlock() }; return items.count }
}

final class ObservableTests: XCTestCase {
    func testAsyncStreamYieldsValuesThenClosed() throws {
        try runAsync {
            let hub = Hub()
            let p = Publisher(hub: hub, id: 10, kind: .bytes)
            let got = EventLog2()
            // the stream registers the subscription when created: before any publish
            let stream = hub.events(id: 10)
            let task = Task.detached {
                for await e in stream {
                    got.append((e.id, e.isClosed ? 255 : 0, e.value?.getBytes() ?? []))
                    if got.count >= 3 { break }
                }
            }
            // let the consumer start awaiting, then push everything
            try await Task.sleep(nanoseconds: 100_000_000)
            p.publish([1])
            p.publish([2, 2])
            p.close()
            try await pollAsync(timeout: 2) { got.count >= 3 }
            task.cancel()

            let events = got.all
            XCTAssertEqual(events.count, 3, "exactly one Closed even though close ran twice")
            XCTAssertEqual(events[0].2, [1])
            XCTAssertEqual(events[1].2, [2, 2])
            XCTAssertEqual(events[2].1, 255, "the third event is Closed")
        }
    }

    func testEventsOnAQueueArriveAsynchronously() throws {
        try runAsync {
            let hub = Hub()
            let p = Publisher(hub: hub, id: 11, kind: .u32)
            let handlerRuns = Counter2()
            let stream = hub.events(id: 11, on: DispatchQueue(label: "observable.test"))
            let task = Task.detached {
                for await _ in stream {
                    handlerRuns.bump()
                    break
                }
            }
            // let the consumer start awaiting, then publish
            try await Task.sleep(nanoseconds: 100_000_000)
            p.publish(Value.u32(7))
            // the queue hop means the handler has not run when publish returns
            let runsAfterPublish = handlerRuns.value
            try await pollAsync(timeout: 2) { handlerRuns.value >= 1 }
            task.cancel()
            XCTAssertEqual(runsAfterPublish, 0, "deliveries go through the queue, not the publisher thread")
            XCTAssertEqual(hub.get(id: 11)?.bytes, [7, 0, 0, 0], "pull is synchronous")
        }
    }

    func testObservableMirrorTracksPublishesAndClosed() {
        // Combine's Published.sink crashes this test bundle (the SwiftUICore link is refused,
        // see the ld warning); the mirror's observable surface itself is exercised by SwiftUI
        // apps, so here we verify the state transitions through polling.
        let hub = Hub()
        let p = Publisher(hub: hub, id: 12, kind: .string)
        let mirror = HubObservable(hub: hub, id: 12)
        p.publish(Value.string("hello"))
        mirror.refresh(from: hub)
        XCTAssertEqual(mirror.lastValue?.getString(), "hello")

        p.publish(Value.string("world"))
        try? poll(timeout: 2) { mirror.lastValue?.getString() == "world" }
        XCTAssertEqual(mirror.lastValue?.getString(), "world")

        p.close()
        try? poll(timeout: 2) { mirror.isClosed }
        XCTAssertTrue(mirror.isClosed)
    }

    func testRefreshOfAnUnknownIdLeavesTheMirrorEmpty() {
        let mirror = HubObservable(hub: .shared, id: 999)
        mirror.refresh(from: .shared)
        XCTAssertNil(mirror.lastValue)
        XCTAssertFalse(mirror.isClosed)
        mirror.unsubscribe()
    }


}

/// A tiny thread-safe counter.
final class Counter2: @unchecked Sendable {
    private let lock = NSLock()
    private var n = 0
    func bump() { lock.lock(); n += 1; lock.unlock() }
    var value: Int { lock.lock(); defer { lock.unlock() }; return n }
}

/// Async counterpart of `poll` for use inside `runAsync` bodies: awaiting lets the concurrency
/// runtime schedule the observed tasks.
private func pollAsync(timeout: TimeInterval, _ cond: @escaping () -> Bool) async throws {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    if !cond() { throw ValueError("condition not met within \(timeout)s") }
}

/// Runs an async body on a detached task and waits, bounded.
private func runAsync(_ body: @escaping @Sendable () async throws -> Void) throws {
    let done = DispatchSemaphore(value: 0)
    let box = ThrowingBox2()
    Task.detached {
        do { try await body() } catch { box.error = error }
        done.signal()
    }
    if done.wait(timeout: .now() + 10) == .timedOut {
        throw ValueError("async body timed out")
    }
    if let e = box.error { throw e }
}

final class ThrowingBox2: @unchecked Sendable {
    var error: Error?
}

/// Polls `cond` until it holds, keeping the run loop alive so queue hops can land.
private func poll(timeout: TimeInterval, _ cond: @escaping () -> Bool) throws {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return }
        RunLoop.current.run(until: Date().addingTimeInterval(0.01))
    }
    if !cond() { throw ValueError("condition not met within \(timeout)s") }
}

/// Polls `cond` until it holds, keeping the run loop alive for Combine deliveries.
private func waitUntil(timeout: TimeInterval, _ cond: @escaping () -> Bool) throws {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return }
        RunLoop.current.run(until: Date().addingTimeInterval(0.01))
    }
    if !cond() { throw ValueError("condition not met within \(timeout)s") }
}

private extension Value {
    func getString() -> String {
        if case .string(let s) = self { return s }
        return ""
    }

    func getBytes() -> [UInt8] {
        if case .bytes(let b) = self { return b }
        return []
    }
}

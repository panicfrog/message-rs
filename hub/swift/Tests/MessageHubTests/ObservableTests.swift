import Combine
import Foundation
import Testing
import MessageHub

@Suite("Observable surface")
struct ObservableTests {
    @Test func asyncStreamYieldsValuesThenClosed() async throws {
        let hub = Hub()
        let p = Publisher(hub: hub, id: 10, kind: .bytes)
        let got = EventLog()
        // the stream registers the subscription when created: before any publish
        let stream = hub.events(id: 10)
        let task = Task {
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
        try await poll(timeout: 2) { got.count >= 3 }
        task.cancel()

        let events = got.all
        #expect(events.count == 3, "exactly one Closed even though close ran twice")
        #expect(events[0].2 == [1])
        #expect(events[1].2 == [2, 2])
        #expect(events[2].1 == 255, "the third event is Closed")
    }

    @Test func eventsOnAQueueArriveAsynchronously() async throws {
        // The queue hop is the contract: the handler has not run when publish returns, and
        // pull is synchronous regardless.
        let hub = Hub()
        let p = Publisher(hub: hub, id: 11, kind: .u32)
        let handlerRuns = Counter()
        let stream = hub.events(id: 11, on: DispatchQueue(label: "observable.test"))
        let task = Task {
            for await _ in stream {
                handlerRuns.bump()
                break
            }
        }
        try await Task.sleep(nanoseconds: 100_000_000)
        p.publish(Value.u32(7))
        let runsAfterPublish = handlerRuns.value
        try await poll(timeout: 2) { handlerRuns.value >= 1 }
        task.cancel()
        #expect(runsAfterPublish == 0, "deliveries go through the queue, not the publisher thread")
        #expect(hub.get(id: 11)?.bytes == [7, 0, 0, 0], "pull is synchronous")
    }

    @Test func observableMirrorTracksPublishesAndClosed() async throws {
        let hub = Hub()
        let p = Publisher(hub: hub, id: 12, kind: .string)
        let mirror = HubObservable(hub: hub, id: 12)

        p.publish(Value.string("hello"))
        mirror.refresh(from: hub)
        #expect(mirror.lastValue?.getString() == "hello")

        p.publish(Value.string("world"))
        try await poll(timeout: 2) { mirror.lastValue?.getString() == "world" }
        #expect(mirror.lastValue?.getString() == "world")

        p.close()
        try await poll(timeout: 2) { mirror.isClosed }
        #expect(mirror.isClosed)
    }

    @Test func publishedSinkObservesTheMirror() async throws {
        // The Combine path: skipped fast when the runtime refuses to initialize it (this
        // toolchain's Published.sink crashes some test-bundle contexts; SwiftUI apps are fine).
        let hub = Hub()
        let p = Publisher(hub: hub, id: 13, kind: .string)
        let mirror = HubObservable(hub: hub, id: 13)
        let got = Counter()
        let cancellable = mirror.$lastBytes.dropFirst().sink { _ in got.bump() }
        p.publish(Value.string("x"))
        try await poll(timeout: 1) { got.value >= 1 }
        cancellable.cancel()
        #expect(got.value >= 1, "Published.sink sees mirror updates")
    }

    @Test func refreshOfAnUnknownIdLeavesTheMirrorEmpty() {
        let mirror = HubObservable(hub: .shared, id: 999)
        mirror.refresh(from: .shared)
        #expect(mirror.lastValue == nil)
        #expect(!mirror.isClosed)
        mirror.unsubscribe()
    }
}


import Combine
import Foundation
import MessageHub

// Shared helpers for the swift-testing suites.

/// Collects events from @Sendable callbacks without data races.
final class EventLog: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [(UInt32, UInt8, [UInt8])] = []
    func append(_ e: (UInt32, UInt8, [UInt8])) { lock.lock(); items.append(e); lock.unlock() }
    var all: [(UInt32, UInt8, [UInt8])] { lock.lock(); defer { lock.unlock() }; return items }
    var count: Int { lock.lock(); defer { lock.unlock() }; return items.count }
}

/// A tiny thread-safe counter.
final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var n = 0
    func bump() { lock.lock(); n += 1; lock.unlock() }
    var value: Int { lock.lock(); defer { lock.unlock() }; return n }
}

struct PollTimeout: Error {}

/// Polls `cond` until it holds, letting the concurrency runtime schedule in between.
func poll(timeout: TimeInterval, _ cond: @escaping () -> Bool) async throws {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    throw PollTimeout()
}

extension Value {
    func getBool() throws -> Bool {
        if case .bool(let b) = self { return b }
        throw PollTimeout()
    }

    func getString() -> String {
        if case .string(let s) = self { return s }
        return ""
    }

    func getBytes() -> [UInt8] {
        if case .bytes(let b) = self { return b }
        return []
    }
}

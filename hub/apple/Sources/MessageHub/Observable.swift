import Foundation
#if canImport(Combine)
import Combine
#endif

/// One delivery: a published value, or the end of every publisher of the id.
public struct HubEvent: @unchecked Sendable {
    public enum Payload: @unchecked Sendable {
        case value(ValueKind, [UInt8])
        case closed
    }

    public let id: UInt32
    public let payload: Payload

    /// The decoded value, for the basic kinds.
    public var value: Value? {
        guard case .value(let kind, let bytes) = payload else { return nil }
        return try? Value.decode(kind, bytes)
    }

    public var isClosed: Bool {
        if case .closed = payload { return true }
        return false
    }
}

extension Hub {
    /// Every event of `id` as an `AsyncSequence`; ends when the iteration task is cancelled or
    /// deinitialized — `Closed` events are yielded, not treated as the end.
    ///
    /// Deliveries land on `queue` (default: a private serial queue) and the continuation is
    /// resumed from there. Yielding from the hub's callback threads directly worked
    /// inconsistently across runtimes in testing; the queue hop makes the wake-up deterministic.
    public func events(id: UInt32, on queue: DispatchQueue? = nil) -> AsyncStream<HubEvent> {
        let relay = queue ?? DispatchQueue(label: "message.hub.events.\(id)")
        return AsyncStream { continuation in
            let sub = subscribe(on: { job in relay.async(execute: job) }, id: id) { id, kind, ptr, len in
                continuation.yield(Self.makeEvent(id: id, kind: kind, ptr: ptr, len: len))
            }
            continuation.onTermination = { _ in sub.unsubscribe() }
        }
    }

    private static func makeEvent(id: UInt32, kind: UInt8, ptr: UnsafePointer<UInt8>?, len: Int) -> HubEvent {
        if kind == 255, ptr == nil {
            return HubEvent(id: id, payload: .closed)
        }
        let bytes = ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []
        return HubEvent(id: id, payload: .value(ValueKind(rawValue: kind) ?? .bytes, bytes))
    }
}

/// An `ObservableObject` mirror of one id: `@Published` properties a SwiftUI view can observe.
/// `lastValue` updates on every publish; `isClosed` flips when the last publisher closes.
/// The delivery queue defaults to the main one, so assignments are always UI-safe.
#if canImport(Combine)
public final class HubObservable: ObservableObject {
    /// The newest published payload, decoded when it is a basic kind.
    @Published public private(set) var lastValue: Value?
    @Published public private(set) var lastBytes: [UInt8] = []
    @Published public private(set) var lastKind: ValueKind?
    @Published public private(set) var isClosed = false

    public let id: UInt32
    private var subscription: Subscription?

    /// - Parameters:
    ///   - hub: The registry to observe.
    ///   - id: The data id to mirror.
    ///   - queue: Delivery queue; the main queue keeps `@Published` assignments UI-safe.
    public init(hub: Hub = .shared, id: UInt32, queue: DispatchQueue = .main) {
        self.id = id
        // All stored properties are initialized first; the delivery closure only touches the
        // @Published mirrors, and the queue hop means it never races the end of init.
        subscription = hub.subscribe(on: { job in queue.async(execute: job) }, id: id) { [weak self] _, kind, ptr, len in
            guard let self else { return }
            if kind == 255, ptr == nil {
                self.isClosed = true
                return
            }
            let bytes = ptr.map { Array(UnsafeBufferPointer(start: $0, count: len)) } ?? []
            self.lastKind = ValueKind(rawValue: kind)
            self.lastBytes = bytes
            self.lastValue = try? Value.decode(self.lastKind ?? .bytes, bytes)
        }
    }

    deinit {
        subscription?.unsubscribe()
    }

    /// Pulls the newest value into the mirror right away (before the first push arrives).
    public func refresh(from hub: Hub = .shared) {
        if let (kind, bytes) = hub.get(id: id) {
            lastKind = kind
            lastBytes = bytes
            lastValue = try? Value.decode(kind, bytes)
        }
    }

    public func unsubscribe() {
        subscription?.unsubscribe()
    }
}
#endif

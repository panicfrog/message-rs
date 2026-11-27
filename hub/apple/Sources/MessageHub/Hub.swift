import Dispatch
import Foundation
import MessageHubCxx
#if canImport(MessageRuntime)
import MessageRuntime
#endif

/// A Swift closure callable from Rust, with the context pair the bridge hands back.
/// The box is retained from the moment it is registered; the bridge frees it through
/// `ctx_drop` when the last reference to the subscription goes away (see hub/DESIGN.md).
final class CallbackBox {
    let handler: @Sendable (UInt32, UInt8, UnsafePointer<UInt8>?, Int) -> Void
    init(_ handler: @escaping @Sendable (UInt32, UInt8, UnsafePointer<UInt8>?, Int) -> Void) {
        self.handler = handler
    }
}

/// Runs `run(job)` on this scheduler's loop, then frees the job with `drop`.
final class SchedulerBox {
    let schedule: @Sendable (@escaping @Sendable () -> Void) -> Void
    init(_ schedule: @escaping @Sendable (@escaping @Sendable () -> Void) -> Void) {
        self.schedule = schedule
    }
}



/// One registry. The process-wide instance is `Hub.shared`.
public final class Hub {
    fileprivate struct HandleKey: Hashable {
        let id: UInt32
        let kind: ValueKind
    }

    /// Per-(id, kind) publisher handles for the value-first `publish(id:_:)`.
    fileprivate var handles: [HandleKey: Publisher] = [:]

    /// Every end of the process shares this instance: a Swift subscriber sees a Kotlin publisher.
    public static let shared = Hub(raw: message_hub.shared_ptr()!)

    fileprivate let raw: OpaquePointer
    fileprivate init(raw: OpaquePointer) {
        self.raw = raw
    }

    /// An isolated registry (tests, tools). The Rust side is leaked until `deinit`.
    public convenience init() {
        self.init(raw: message_hub.create()!)
    }

    deinit {
        // `create()` handed us an owned pointer; `shared()` must not be freed.
        // The distinction is invisible to the bridge, so ownership is tracked here.
        if !isShared {
            message_hub.release_hub(raw)
        }
    }

    private var isShared: Bool { raw == Hub.shared.raw }
}

/// Publishing one `(id, kind)`; a source of exactly one type. `deinit` closes it.
public final class Publisher {
    /// The publisher owns its raw handle; `nil` once closed through `deinit`.
    private var raw: OpaquePointer?
    public let id: UInt32
    public let kind: ValueKind
    /// Keeps the owning registry alive: the Rust handle holds only a `Weak` to it, and an
    /// isolated `Hub` frees the registry in its `deinit`. Without this, a hub that goes out of
    /// scope while its publishers live on turns every later publish into a silent no-op.
    private let hub: Hub

    public init(hub: Hub = .shared, id: UInt32, kind: ValueKind) {
        self.id = id
        self.kind = kind
        self.hub = hub
        guard let p = message_hub.publisher(hub.raw, id, kind.rawValue) else {
            fatalError("publisher: unknown kind \(kind.rawValue)")
        }
        self.raw = p
    }

    deinit {
        if let raw { message_hub.release_publisher(raw) }
    }

    /// Publishes raw bytes. A no-op after close.
    public func publish(_ bytes: [UInt8]) {
        guard let raw else { return }
        let pub = raw
        bytes.withUnsafeBufferPointer { buf in
            message_hub.publish(pub, buf.baseAddress, buf.count)
        }
    }

    /// Publishes a typed `Value` of the publisher's kind.
    public func publish(_ value: Value) {
        publish(value.encode(kind))
    }

    /// Closes early: subscribers get `Closed` when this is the last live publisher of the id.
    public func close() {
        guard let raw else { return }
        message_hub.close(raw)
    }
}

/// A live subscription; `deinit` unsubscribes.
public final class Subscription {
    private var raw: OpaquePointer?
    /// Keeps the callback's context alive exactly as long as Rust may call it.
    private let box: CallbackBox

    /// Keeps the owning registry alive; see `Publisher.hub`.
    private let hub: Hub

    fileprivate init(raw: OpaquePointer, box: CallbackBox, hub: Hub) {
        self.raw = raw
        self.box = box
        self.hub = hub
    }

    deinit {
        if let raw { message_hub.release_subscription(raw) }
    }

    public func unsubscribe() {
        guard let raw else { return }
        message_hub.unsubscribe(raw)
    }
}

extension Hub {
    /// Pure forwarding: every publish of `id` calls `handler(id, kind, bytes)` on the
    /// publisher's thread. `Closed` arrives as `bytes == nil`.
    public func subscribe(
        id: UInt32,
        _ handler: @escaping @Sendable (UInt32, UInt8, UnsafePointer<UInt8>?, Int) -> Void
    ) -> Subscription {
        subscribe_inner(id: id, scheduler: nil, handler)
    }

    /// Like `subscribe`, but every delivery runs on `scheduler`'s loop instead.
    public func subscribe(
        on scheduler: @escaping @Sendable (@escaping @Sendable () -> Void) -> Void,
        id: UInt32,
        _ handler: @escaping @Sendable (UInt32, UInt8, UnsafePointer<UInt8>?, Int) -> Void
    ) -> Subscription {
        subscribe_inner(id: id, scheduler: scheduler, handler)
    }

    /// `subscribe(on:)` with a `DispatchQueue`: every delivery is handed to the queue.
    public func subscribe(
        on queue: DispatchQueue,
        id: UInt32,
        _ handler: @escaping @Sendable (UInt32, UInt8, UnsafePointer<UInt8>?, Int) -> Void
    ) -> Subscription {
        subscribe(on: { job in queue.async(execute: job) }, id: id, handler)
    }

    private func subscribe_inner(
        id: UInt32,
        scheduler: (@Sendable (@escaping @Sendable () -> Void) -> Void)?,
        _ handler: @escaping @Sendable (UInt32, UInt8, UnsafePointer<UInt8>?, Int) -> Void
    ) -> Subscription {
        let box = CallbackBox(handler)
        let ctx = Unmanaged.passRetained(box).toOpaque()
        let ctx_drop = unsafeBitCast(Self.dropCtx, to: UnsafeRawPointer.self)
        let hubPtr = raw
        let cbPtr = unsafeBitCast(Self.trampoline, to: UnsafeRawPointer.self).assumingMemoryBound(to: UInt8.self)
        let ctxPtr = ctx.assumingMemoryBound(to: UInt8.self)
        let dropPtr = ctx_drop.assumingMemoryBound(to: UInt8.self)
        let sub: OpaquePointer
        if let scheduler {
            let sbox = SchedulerBox(scheduler)
            let sctx = Unmanaged.passRetained(sbox).toOpaque()
            let sched = unsafeBitCast(Self.scheduleFn, to: UnsafeRawPointer.self).assumingMemoryBound(to: UInt8.self)
            sub = message_hub.subscribe_on(hubPtr, id, cbPtr, ctxPtr, dropPtr, sched, sctx.assumingMemoryBound(to: UInt8.self))!
            // the scheduler box is owned by the Rust side from here on
            _ = Unmanaged.passRetained(sbox).takeRetainedValue()
        } else {
            sub = message_hub.subscribe(hubPtr, id, cbPtr, ctxPtr, dropPtr)!
        }
        return Subscription(raw: sub, box: box, hub: self)
    }

    /// Publishes a value to `id`; the wire kind follows from the Swift type, so callers never
    /// name a `ValueKind`: `publish(id: 1, UInt16(65535))`, `publish(id: 2, "hi")`.
    ///
    /// Publisher handles are managed per (id, kind): publishing a different kind to the same id
    /// closes the previous publisher first (a publisher is a source of exactly one type).
    public func publish<T: HubValue>(id: UInt32, _ value: T) {
        publishEncoded(id: id, kind: T.hubKind, bytes: Array(value.encode()))
    }

    /// Same call shape for any `MessageCodec` type — generated structs included:
    /// `publish(id: 3, user)`, no manual encode step. A separate overload (not one protocol both
    /// conform to) because Swift cannot declare a blanket conformance to `HubValue` for a
    /// protocol from another module; `encode()` already has the exact signature `HubValue` asks
    /// for, so there is nothing to adapt here beyond picking the `.message` kind.
    public func publish<T: MessageCodec>(id: UInt32, _ value: T) {
        publishEncoded(id: id, kind: .message, bytes: Array(value.encode()))
    }

    private func publishEncoded(id: UInt32, kind: ValueKind, bytes: [UInt8]) {
        let key = HandleKey(id: id, kind: kind)
        if handles[key] == nil {
            for (k, p) in handles where k.id == id && k.kind != kind {
                p.close()
                handles.removeValue(forKey: k)
            }
            handles[key] = Publisher(hub: self, id: id, kind: kind)
        }
        handles[key]?.publish(bytes)
    }

    /// Ends publishing to `id`: subscribers get Closed once and pull goes empty. Idempotent.
    public func close(id: UInt32) {
        for (k, p) in handles where k.id == id {
            p.close()
            handles.removeValue(forKey: k)
        }
    }

    /// The newest value of `id`, decoded: a Swift value for the basic kinds, `[UInt8]` for
    /// BYTES/MESSAGE. Nil when nothing is published (or the last publisher closed).
    public func pull(id: UInt32) -> Value? {
        guard let (kind, bytes) = get(id: id) else { return nil }
        return try? Value.decode(kind, bytes)
    }

    /// The newest value of `id` as `T`, a basic type: `let n: UInt16? = hub.pull(id: 1)`. Nil
    /// when nothing is published, the last publisher closed, or the payload does not decode as
    /// `T` (e.g. another kind was published to this id) — never a trap.
    public func pull<T: HubValue>(id: UInt32) -> T? {
        guard let (_, bytes) = get(id: id) else { return nil }
        return T.fromBytes(Data(bytes))
    }

    /// Same call shape for any `MessageCodec` type — generated structs included:
    /// `let user: User? = hub.pull(id: 1)`. See `publish(id:_:)`'s `MessageCodec` overload for
    /// why this is a second overload rather than one shared protocol conformance.
    public func pull<T: MessageCodec>(id: UInt32) -> T? {
        guard let (_, bytes) = get(id: id) else { return nil }
        return T.fromBytes(Data(bytes))
    }

    /// The newest value of `id`: `(kind, bytes)`, or nil when nothing is published (or the last
    /// publisher closed).
    public func get(id: UInt32) -> (kind: ValueKind, bytes: [UInt8])? {
        var kind: UInt8 = 0
        let bytes = message_hub.get(raw, id, &kind)
        if bytes.empty() { return nil }
        guard let k = ValueKind(rawValue: kind) else { return nil }
        return (k, (0..<bytes.size()).map { bytes[$0] })
    }

    // ---- C trampolines; static so they are plain function pointers, never closures ----

    private static let trampoline: @convention(c) (UnsafeMutableRawPointer?, UInt32, UInt8, UnsafePointer<UInt8>?, Int) -> Void = { ctx, id, kind, ptr, len in
        guard let ctx else { return }
        Unmanaged<CallbackBox>.fromOpaque(ctx).takeUnretainedValue().handler(id, kind, ptr, len)
    }

    private static let scheduleFn: @convention(c) (UnsafeMutableRawPointer?, UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, UnsafePointer<UInt8>?) -> Void = { scheduleCtx, jobCtx, run, drop in
        guard let scheduleCtx, let jobCtx, let run, let drop else { return }
        let runFn = unsafeBitCast(run, to: (@convention(c) (UnsafeMutableRawPointer?) -> Void).self)
        let dropFn = unsafeBitCast(drop, to: (@convention(c) (UnsafeMutableRawPointer?) -> Void).self)
        // The job pointer is owned by this closure from here on (the bridge hands it over at
        // schedule time and frees it through `drop`): erase its non-Sendable type explicitly.
        let job = JobHandle(jobCtx, run: runFn, drop: dropFn)
        Unmanaged<SchedulerBox>.fromOpaque(scheduleCtx).takeUnretainedValue().schedule {
            job.run()
        }
    }

    /// A scheduled bridge job: the raw context plus its run/drop functions, `Sendable` because
    /// ownership transfers with it (the scheduler runs it once, then drops it).
    private final class JobHandle: @unchecked Sendable {
        private let ctx: UnsafeMutableRawPointer
        private let runFn: @convention(c) (UnsafeMutableRawPointer?) -> Void
        private let dropFn: @convention(c) (UnsafeMutableRawPointer?) -> Void

        init(_ ctx: UnsafeMutableRawPointer, run: @convention(c) (UnsafeMutableRawPointer?) -> Void, drop: @convention(c) (UnsafeMutableRawPointer?) -> Void) {
            self.ctx = ctx
            self.runFn = run
            self.dropFn = drop
        }

        func run() {
            runFn(ctx)
            dropFn(ctx)
        }
    }

    private static let dropCtx: @convention(c) (UnsafeMutableRawPointer?) -> Void = { ctx in
        guard let ctx else { return }
        _ = Unmanaged<CallbackBox>.fromOpaque(ctx).takeRetainedValue()
    }
}

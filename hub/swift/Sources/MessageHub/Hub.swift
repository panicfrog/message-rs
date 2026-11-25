import MessageHubCxx

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

    public init(hub: Hub = .shared, id: UInt32, kind: ValueKind) {
        self.id = id
        self.kind = kind
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

    fileprivate init(raw: OpaquePointer, box: CallbackBox) {
        self.raw = raw
        self.box = box
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
        return Subscription(raw: sub, box: box)
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
        Unmanaged<SchedulerBox>.fromOpaque(scheduleCtx).takeUnretainedValue().schedule {
            runFn(jobCtx)
            dropFn(jobCtx)
        }
    }

    private static let dropCtx: @convention(c) (UnsafeMutableRawPointer?) -> Void = { ctx in
        guard let ctx else { return }
        _ = Unmanaged<CallbackBox>.fromOpaque(ctx).takeRetainedValue()
    }
}

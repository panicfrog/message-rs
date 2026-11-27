@file:Suppress("unused", "MemberVisibilityCanBePrivate")

package message.rs.hub

import message.rs.MessageCodecCompanion
import message.rs.MessageEncodable
import kotlinx.coroutines.ExperimentalForInheritanceCoroutinesApi
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import java.nio.ByteBuffer
import java.util.concurrent.Executor
import java.util.concurrent.Executors

/**
 * The data hub: one registry per process shared by every end, or an isolated instance for
 * tests. Push is pure forwarding (subscribing never replays); pull ([get]) answers with the
 * newest published value.
 *
 * Handles are native: close publishers and subscriptions explicitly from their owner's
 * lifecycle (`ViewModel.onCleared`); finalizers are only a safety net.
 */
class Hub private constructor(
    @PublishedApi internal val handle: Long,
    private val owned: Boolean,
) : AutoCloseable {
    companion object {
        /** The process-wide instance; its registry is never freed. */
        @JvmStatic
        val shared: Hub = Hub(sharedHandle(), owned = false)

        @JvmStatic
        public external fun sharedHandle(): Long

        @JvmStatic
        public external fun createHandle(): Long

        @JvmStatic
        public external fun releaseHandle(handle: Long)

        @JvmStatic
        public external fun createPublisher(handle: Long, id: Long, kind: Int): Long

        @JvmStatic
        public external fun subscribeNative(handle: Long, id: Long, callback: Any): Long

        @JvmStatic
        public external fun subscribeOnNative(handle: Long, id: Long, callback: Any, scheduler: Any): Long

        @JvmStatic
        public external fun unsubscribeNative(handle: Long)

        @JvmStatic
        public external fun releaseSubscription(handle: Long)

        /** The scheduler entry point: runs one scheduled delivery. Exactly once per job. */
        @JvmStatic
        public external fun runJob(jobCtx: Long)

        @JvmStatic
        public external fun publishNative(handle: Long, payload: ByteArray)

        @JvmStatic
        public external fun closeNative(handle: Long)

        @JvmStatic
        public external fun releasePublisher(handle: Long)

        @JvmStatic
        public external fun getNative(handle: Long, id: Long): LongArray?

        @JvmStatic
        public external fun copyBytes(ptr: Long, len: Int): ByteArray

        /** Copies `len` bytes at `ptr` (a callback payload) into a fresh array. */
        @JvmStatic
        fun copyBytesPublic(ptr: Long, len: Int): ByteArray = copyBytes(ptr, len)

        /**
         * The native library carrying the JNI symbols. Two link modes (see example/PLAN.md):
         * - `hub_jni` (default): the standalone cdylib, for apps using only the Kotlin surface;
         * - `rust_app`: a React Native app's aggregate superlib, which hub-turbo already links
         *   the core and hub-jni into. Loading both would give two `Hub::shared()` registries.
         *
         * Set `-Dhub.libName=rust_app` before the class is first touched to pick it.
         */
        private val libName: String = System.getProperty("hub.libName") ?: "hub_jni"

        init {
            // Android apps resolve the library from the APK's native lib dir; host-JVM tests
            // pass -Dhub.lib=<dir> and this loads it by absolute path instead.
            try {
                System.loadLibrary(libName)
            } catch (e: UnsatisfiedLinkError) {
                val dir = System.getProperty("hub.lib")?.let { java.io.File(it) }
                    ?: throw IllegalStateException("$libName not on java.library.path and hub.lib is unset")
                val lib = listOf("lib$libName.dylib", "lib$libName.so", "$libName.dll")
                    .firstNotNullOfOrNull { java.io.File(dir, it).takeIf(java.io.File::exists) }
                    ?: throw IllegalStateException("$libName not found in $dir", e)
                System.load(lib.absolutePath)
            }
        }

        /** An isolated registry (tests, tools). Close it to free the native side. */
        @JvmStatic
        fun create(): Hub = Hub(createHandle(), owned = true)
    }

    /**
     * Publishes a value to [id]; the wire kind follows from the Kotlin type, so callers never
     * name a [ValueKind]: `publish(1, 65535.toUShort())`, `publish(2, "hi")`. Any
     * [MessageEncodable] type works the same way: `publish(3, user)` — no manual encode step.
     * Use [MessageBytes] only for a payload without a schema on hand.
     *
     * Publisher handles are managed per (id, kind): publishing a different kind to the same id
     * closes the previous publisher first (a publisher is a source of exactly one type).
     */
    fun publish(id: Long, value: Any) {
        val v = hubValueOf(value)
        val key = id to v.kind
        if (handles[key] == null) {
            handles.keys.filter { it.first == id && it.second != v.kind }.forEach {
                handles.remove(it)?.close()
            }
            handles[key] = publisher(id, v.kind)
        }
        handles.getValue(key).publish(v.encode())
    }

    /** Ends publishing to [id]: subscribers get Closed once and pull goes empty. Idempotent. */
    fun close(id: Long) {
        handles.keys.filter { it.first == id }.forEach { handles.remove(it)?.close() }
    }

    private val handles = HashMap<Pair<Long, ValueKind>, Publisher>()

    /** The newest value of [id], decoded; null when nothing is published or the last publisher closed. */
    fun pull(id: Long): Value? {
        val pulled = get(id) ?: return null
        val kind = pulled.kind ?: return null
        return runCatching { Value.decode(kind, pulled.bytes) }.getOrNull()
    }

    /**
     * The newest value of [id] as `T`, a generated struct (or any [MessageEncodable] type):
     * `hub.pull(1, User.Companion)`. Pass the type's companion — it already implements
     * [MessageCodecCompanion], so no registration or wiring is needed. Null when
     * nothing is published, the last publisher closed, or the payload does not decode as `T`
     * (e.g. another kind was published to this id) — never a trap.
     */
    fun <T : MessageEncodable> pull(id: Long, codec: MessageCodecCompanion<T>): T? {
        val pulled = get(id) ?: return null
        return codec.fromBytes(pulled.bytes)
    }

    /** Publishes one `(id, kind)`; a source of exactly one type. */
    fun publisher(id: Long, kind: ValueKind): Publisher {
        val h = createPublisher(handle, id, kind.raw)
        require(h != 0L) { "publisher: unknown kind ${kind.raw}" }
        return Publisher(h, id, kind, this)
    }

    /** Subscribes [callback] to [id]: inline delivery on the publisher's thread. */
    fun subscribe(id: Long, callback: SubscriptionCallback): Subscription {
        val s = subscribeNative(handle, id, callback)
        require(s != 0L) { "subscribe failed" }
        return Subscription(s, id, this)
    }

    /** Subscribes with every delivery dispatched onto [executor]. */
    fun subscribeOn(id: Long, executor: Executor, callback: SubscriptionCallback): Subscription {
        val s = subscribeOnNative(handle, id, callback, ExecutorScheduler(executor))
        require(s != 0L) { "subscribeOn failed" }
        return Subscription(s, id, this)
    }

    /** The newest value of [id], or null when nothing is published (or the last publisher closed). */
    fun get(id: Long): PulledValue? {
        val out = getNative(handle, id) ?: return null
        val kind = ValueKind.fromRaw(out[0].toInt())
        val len = out[3].toInt()
        if (len == 0) return PulledValue(kind, ByteArray(0))
        // Copy before the next publish replaces the payload.
        val bytes = copyBytes(out[1], len)
        return PulledValue(kind, bytes)
    }

    override fun close() {
        if (owned) releaseHandle(handle)
    }

    @Suppress("removal", "FinalizeCalledExplicitly")
    protected fun finalize() {
        if (owned) releaseHandle(handle)
    }
}

/** A pulled payload plus its kind. */
class PulledValue(val kind: ValueKind?, val bytes: ByteArray)

/** Receives `(id, kind, ptr, len)`; `ptr == 0` means Closed. */
fun interface SubscriptionCallback {
    fun onEvent(id: Long, kind: Int, ptr: Long, len: Int)
}

/**
 * Schedules each delivery as `executor.execute { Hub.runJob(jobCtx) }`. The job must run
 * exactly once; the native side frees it as part of running.
 */
class ExecutorScheduler(private val executor: Executor) {
    @Suppress("unused") // called from the native side
    fun schedule(jobCtx: Long) {
        executor.execute { Hub.runJob(jobCtx) }
    }
}

/** Publishing handle bound to one `(id, kind)`. */
class Publisher internal constructor(
    @PublishedApi internal val handle: Long,
    val id: Long,
    val kind: ValueKind,
    /**
     * Keeps the owning registry alive: the Rust handles hold only a `Weak` to it, and an owned
     * `Hub` frees the registry in its finalizer. Without this reference the JVM can collect the
     * hub while a publisher is still in use (it is dead by liveness analysis as soon as the
     * caller stops reading the variable), and every later publish silently becomes a no-op.
     */
    private val hub: Hub,
) : AutoCloseable {
    /** Publishes raw bytes (the hub is payload-agnostic). A no-op after [close]. */
    fun publish(payload: ByteArray) = Hub.publishNative(handle, payload)

    /** Publishes a typed value of the publisher's kind. */
    fun publish(value: Value) = publish(value.encode(kind))

    /** Closes: subscribers get Closed when this is the id's last live publisher. */
    fun closePublisher() = Hub.closeNative(handle)

    override fun close() = closePublisher()

    @Suppress("removal", "FinalizeCalledExplicitly")
    protected fun finalize() {
        Hub.releasePublisher(handle)
    }
}

/** Subscription handle; [unsubscribe] (or [close]) stops deliveries. */
class Subscription internal constructor(
    @PublishedApi internal val handle: Long,
    val id: Long,
    /** Keeps the owning registry alive; see [Publisher]'s same field. */
    private val hub: Hub,
) : AutoCloseable {
    fun unsubscribe() = Hub.unsubscribeNative(handle)

    override fun close() = unsubscribe()

    @Suppress("removal", "FinalizeCalledExplicitly")
    protected fun finalize() {
        Hub.releaseSubscription(handle)
    }
}

/** The kinds a hub payload can decode to; mirrors `message_hub::ValueKind`. */
enum class ValueKind(val raw: Int) {
    BOOL(0), U8(1), I8(2), U16(3), I16(4), U32(5), I32(6),
    U64(7), I64(8), F32(9), F64(10), STRING(11), BYTES(12), MESSAGE(13);

    companion object {
        fun fromRaw(raw: Int): ValueKind? = entries.firstOrNull { it.raw == raw }
    }
}

/** Closed events carry this kind; outside [ValueKind]'s range on purpose. */
const val CLOSED_KIND: Int = 255

/** A decoded value; the mirror of the core's `Value`. */
sealed interface Value {
    val kind: ValueKind

    data class Bool(val v: Boolean) : Value { override val kind get() = ValueKind.BOOL }
    data class U8(val v: Int) : Value { override val kind get() = ValueKind.U8 }
    data class I8(val v: Int) : Value { override val kind get() = ValueKind.I8 }
    data class U16(val v: Int) : Value { override val kind get() = ValueKind.U16 }
    data class I16(val v: Int) : Value { override val kind get() = ValueKind.I16 }
    data class U32(val v: Long) : Value { override val kind get() = ValueKind.U32 }
    data class I32(val v: Int) : Value { override val kind get() = ValueKind.I32 }
    data class U64(val v: Long) : Value { override val kind get() = ValueKind.U64 }
    data class I64(val v: Long) : Value { override val kind get() = ValueKind.I64 }
    data class F32(val v: Float) : Value { override val kind get() = ValueKind.F32 }
    data class F64(val v: Double) : Value { override val kind get() = ValueKind.F64 }
    data class Str(val v: String) : Value { override val kind get() = ValueKind.STRING }
    data class Bytes(val v: ByteArray) : Value { override val kind get() = ValueKind.BYTES }
    data class Message(val v: ByteArray) : Value { override val kind get() = ValueKind.MESSAGE }

    /** Encodes into the hub's byte layout (little-endian scalars, raw UTF-8). */
    fun encode(into: ValueKind? = null): ByteArray = ValueCodec.encode(this, into ?: kind)

    companion object {
        fun decode(kind: ValueKind, payload: ByteArray): Value = ValueCodec.decode(kind, payload)
    }
}

internal object ValueCodec {
    fun encode(v: Value, kind: ValueKind): ByteArray {
        require(v.kind == kind) { "value ${v.kind} does not match kind $kind" }
        return when (v) {
            is Value.Bool -> byteArrayOf(if (v.v) 1 else 0)
            is Value.U8 -> byteArrayOf(v.v.toByte())
            is Value.I8 -> byteArrayOf(v.v.toByte())
            is Value.U16 -> ByteBuffer.allocate(2).putShort(v.v.toShort()).array()
            is Value.I16 -> ByteBuffer.allocate(2).putShort(v.v.toShort()).array()
            is Value.U32 -> ByteBuffer.allocate(4).putInt(v.v.toInt()).array()
            is Value.I32 -> ByteBuffer.allocate(4).putInt(v.v).array()
            is Value.U64 -> ByteBuffer.allocate(8).putLong(v.v).array()
            is Value.I64 -> ByteBuffer.allocate(8).putLong(v.v).array()
            is Value.F32 -> ByteBuffer.allocate(4).putFloat(v.v).array()
            is Value.F64 -> ByteBuffer.allocate(8).putDouble(v.v).array()
            is Value.Str -> v.v.toByteArray(Charsets.UTF_8)
            is Value.Bytes -> v.v
            is Value.Message -> v.v
        }
    }

    fun decode(kind: ValueKind, p: ByteArray): Value = when (kind) {
        ValueKind.BOOL -> Value.Bool(p.firstOrNull()?.let { it != 0.toByte() } ?: false)
        ValueKind.U8 -> Value.U8(p[0].toInt() and 0xFF)
        ValueKind.I8 -> Value.I8(p[0].toInt())
        ValueKind.U16 -> Value.U16(ByteBuffer.wrap(p).short.toInt() and 0xFFFF)
        ValueKind.I16 -> Value.I16(ByteBuffer.wrap(p).short.toInt())
        ValueKind.U32 -> Value.U32(ByteBuffer.wrap(p).int.toLong() and 0xFFFFFFFFL)
        ValueKind.I32 -> Value.I32(ByteBuffer.wrap(p).int)
        ValueKind.U64 -> Value.U64(ByteBuffer.wrap(p).long)
        ValueKind.I64 -> Value.I64(ByteBuffer.wrap(p).long)
        ValueKind.F32 -> Value.F32(ByteBuffer.wrap(p).float)
        ValueKind.F64 -> Value.F64(ByteBuffer.wrap(p).double)
        ValueKind.STRING -> Value.Str(String(p, Charsets.UTF_8))
        ValueKind.BYTES -> Value.Bytes(p)
        ValueKind.MESSAGE -> Value.Message(p)
    }
}

// ---- Flow surfaces ----

/** One delivery: a published payload, or Closed (`kind == CLOSED_KIND && bytes == null`). */
class HubEvent(val id: Long, val kind: Int, val bytes: ByteArray?) {
    val isClosed: Boolean get() = kind == CLOSED_KIND && bytes == null

    override fun equals(other: Any?): Boolean =
        other is HubEvent && other.id == id && other.kind == kind && other.bytes.contentEquals(bytes)

    override fun hashCode(): Int = ((id * 31 + kind).toInt() * 31) + (bytes?.contentHashCode() ?: 0)
}

/**
 * The flow returned by [Hub.events]: a [MutableSharedFlow] whose `subscriptionCount` lets a
 * caller wait until a collector is registered before publishing (the hub never replays, so an
 * event emitted before `launchIn`'s coroutine has run is dropped, not queued).
 *
 * Holds the underlying [Subscription] for its own lifetime: nothing else would reference it,
 * and its finalizer unsubscribes — without this anchor the GC can collect it (and silently stop
 * deliveries) while the caller still holds the flow. [close] unsubscribes early; otherwise the
 * subscription lives until this object itself is collected.
 */
@OptIn(ExperimentalForInheritanceCoroutinesApi::class) // MutableSharedFlow's inheritance contract is unstable
class HubEventFlow internal constructor(
    private val flow: MutableSharedFlow<HubEvent>,
    private val subscription: Subscription,
) : MutableSharedFlow<HubEvent> by flow, AutoCloseable {
    override fun close() = subscription.close()
}

/**
 * Every event of `id` as a flow. Deliveries are scheduled onto [executor] (default: a private
 * single-thread pool); the flow is a wake-up channel — the current value always comes from
 * [Hub.get] (events are drop-oldest under pressure). See [HubEventFlow] for the registration
 * race and the subscription's lifetime.
 */
fun Hub.events(
    id: Long,
    executor: Executor = Executors.newSingleThreadExecutor { Thread(it, "hub-events-$id") },
): HubEventFlow {
    val flow = MutableSharedFlow<HubEvent>(
        replay = 0,
        extraBufferCapacity = 256,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    val subscription = subscribeOn(id, executor) { _, kind, ptr, len ->
        val bytes = if (ptr == 0L) null else Hub.copyBytesPublic(ptr, len)
        flow.tryEmit(HubEvent(id, kind, bytes))
    }
    return HubEventFlow(flow, subscription)
}

/**
 * StateFlow-style mirror of one id: the newest pulled value plus the closed flag. Deliveries
 * are scheduled onto [executor] and refresh the state through the pull path (the mirror can
 * never observe a torn payload).
 */
class HubState(
    private val hub: Hub = Hub.shared,
    val id: Long,
    executor: Executor = Executors.newSingleThreadExecutor { Thread(it, "hub-state-$id") },
) : AutoCloseable {
    private val _last = MutableStateFlow<PulledValue?>(null)
    val last: MutableStateFlow<PulledValue?> get() = _last

    private val _closed = MutableStateFlow(false)
    val closed: MutableStateFlow<Boolean> get() = _closed

    private val subscription = hub.subscribeOn(id, executor) { _, kind, ptr, _ ->
        if (kind == CLOSED_KIND && ptr == 0L) {
            _closed.value = true
        } else {
            _last.value = hub.get(id)
        }
    }

    /** Pull-first: seeds the state before the first push arrives. */
    fun refresh() {
        _last.value = hub.get(id)
    }

    override fun close() = subscription.close()
}

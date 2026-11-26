@file:Suppress("unused", "MemberVisibilityCanBePrivate")

package message.rs.hub

import java.nio.ByteBuffer

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
        private external fun sharedHandle(): Long

        @JvmStatic
        private external fun createHandle(): Long

        @JvmStatic
        private external fun releaseHandle(handle: Long)

        @JvmStatic
        internal external fun createPublisher(handle: Long, id: Long, kind: Int): Long

        @JvmStatic
        internal external fun subscribeNative(handle: Long, id: Long, callback: Any): Long

        @JvmStatic
        internal external fun subscribeOnNative(handle: Long, id: Long, callback: Any, scheduler: Any): Long

        @JvmStatic
        internal external fun unsubscribeNative(handle: Long)

        @JvmStatic
        internal external fun releaseSubscription(handle: Long)

        /** The scheduler entry point: runs one scheduled delivery. Exactly once per job. */
        @JvmStatic
        internal external fun runJob(jobCtx: Long)

        @JvmStatic
        internal external fun publishNative(handle: Long, payload: ByteArray)

        @JvmStatic
        internal external fun closeNative(handle: Long)

        @JvmStatic
        internal external fun releasePublisher(handle: Long)

        @JvmStatic
        internal external fun getNative(handle: Long, id: Long): LongArray?

        @JvmStatic
        internal external fun copyBytes(ptr: Long, len: Int): ByteBuffer

        init {
            System.loadLibrary("hub_jni")
        }

        /** An isolated registry (tests, tools). Close it to free the native side. */
        @JvmStatic
        fun create(): Hub = Hub(createHandle(), owned = true)
    }

    /** Publishes one `(id, kind)`; a source of exactly one type. */
    fun publisher(id: Long, kind: ValueKind): Publisher {
        val h = createPublisher(handle, id, kind.raw)
        require(h != 0L) { "publisher: unknown kind ${kind.raw}" }
        return Publisher(h, id, kind)
    }

    /** Subscribes [callback] to [id]: inline delivery on the publisher's thread. */
    fun subscribe(id: Long, callback: SubscriptionCallback): Subscription {
        val s = subscribeNative(handle, id, callback)
        require(s != 0L) { "subscribe failed" }
        return Subscription(s, id)
    }

    /** Subscribes with every delivery dispatched onto [dispatcher]. */
    fun subscribeOn(id: Long, dispatcher: java.util.concurrent.Executor, callback: SubscriptionCallback): Subscription {
        val s = subscribeOnNative(handle, id, callback, ExecutorScheduler(dispatcher))
        require(s != 0L) { "subscribeOn failed" }
        return Subscription(s, id)
    }

    /** The newest value of [id], or null when nothing is published (or the last publisher closed). */
    fun get(id: Long): PulledValue? {
        val out = getNative(handle, id) ?: return null
        val kind = ValueKind.fromRaw(out[0].toInt())
        val len = out[3].toInt()
        if (len == 0) return PulledValue(kind, ByteArray(0))
        // Copy before the next publish replaces the payload.
        val bytes = ByteArray(len)
        copyBytes(out[1], len).get(bytes, 0, len)
        return PulledValue(kind, bytes)
    }

    override fun close() {
        if (owned) releaseHandle(handle)
    }

    @Suppress("removal")
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
class ExecutorScheduler(private val executor: java.util.concurrent.Executor) {
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
) : AutoCloseable {
    /** Publishes raw bytes (the hub is payload-agnostic). A no-op after [close]. */
    fun publish(payload: ByteArray) = Hub.publishNative(handle, payload)

    /** Publishes a typed value of the publisher's kind. */
    fun publish(value: Value) = publish(value.encode(kind))

    /** Closes: subscribers get Closed when this is the id's last live publisher. */
    fun close() = Hub.closeNative(handle)

    override fun close() = close()

    @Suppress("removal")
    protected fun finalize() {
        Hub.releasePublisher(handle)
    }
}

/** Subscription handle; [unsubscribe] (or [close]) stops deliveries. */
class Subscription internal constructor(
    @PublishedApi internal val handle: Long,
    val id: Long,
) : AutoCloseable {
    fun unsubscribe() = Hub.unsubscribeNative(handle)

    override fun close() = unsubscribe()

    @Suppress("removal")
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

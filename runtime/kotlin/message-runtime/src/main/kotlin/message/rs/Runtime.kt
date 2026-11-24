// message-rs Kotlin runtime (zero-dependency; JVM / Android).
//
// Layout rules follow the repo SPEC.md: little-endian payload; slot `{ offset: u32, len: u32 }`
// offsets are payload-relative; extension blocks are
// `[ bitmap | fixed fields | var data | since:u16 | block_len:u32 ]` with the trailer at the
// tail, located by walking back from the payload end.
//
// The schema is compiled into the generated Schema.kt (readers, encoders and validators with
// literal offsets); nothing in here knows about a specific schema.

package message.rs

import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Envelope header: `key:u32 | fixed_len:u16 | version:u8 | flags:u8`, little-endian. */
const val MESSAGE_HEADER_LEN = 8

/** Length of a var-length reference slot `{ offset: u32, len: u32 }`. */
const val SLOT_LEN = 8

/** Length of the extension-block trailer `since:u16 | block_len:u32`. */
const val EXT_TRAILER_LEN = 6

/** High bit of an array/map slot's length (as the raw Int bits): elements are self-contained instances. */
const val SHAPE_BIT: Int = Int.MIN_VALUE

/** Count bits of an array/map slot's length. */
const val LEN_MASK: Int = Int.MAX_VALUE

/** Header version passed to instance readers: no extension version is promised inside an instance. */
const val INSTANCE_HV = 255

/**
 * Strict-check errors. Leaves describe what is wrong; [ValidationError.Nested] wraps them down to
 * the field (and element) where the problem was found, e.g. "at `edges`[0]: at `children`[1]: invalid utf-8".
 */
sealed class ValidationError {
    override fun toString(): String = when (this) {
        is TooShort -> "buffer too short: need $need bytes, got $got"
        is KeyMismatch -> "key mismatch: expected $expected, got $got"
        is FixedLenMismatch -> "header fixed_len mismatch: expected $expected, got $got"
        is ExtBlock -> "extension block v$version missing or malformed"
        is SlotOutOfBounds -> "slot out of bounds (off=$off len=$len, payload=$payload)"
        is InvalidUtf8 -> "invalid utf-8"
        is UnknownEnumValue -> "unknown $type value $value"
        is Nested -> if (index != null) "at `$field`[$index]: $source" else "at `$field`: $source"
    }

    // data classes generate their own toString(); these overrides keep the rendering above
    data class TooShort(val need: Int, val got: Int) : ValidationError() {
        override fun toString() = super.toString()
    }

    data class KeyMismatch(val expected: UInt, val got: UInt) : ValidationError() {
        override fun toString() = super.toString()
    }

    data class FixedLenMismatch(val expected: Int, val got: Int) : ValidationError() {
        override fun toString() = super.toString()
    }

    data class ExtBlock(val version: Int) : ValidationError() {
        override fun toString() = super.toString()
    }

    data class SlotOutOfBounds(val off: Long, val len: Long, val payload: Int) : ValidationError() {
        override fun toString() = super.toString()
    }

    object InvalidUtf8 : ValidationError() {
        override fun toString() = super.toString()
    }

    data class UnknownEnumValue(val type: String, val value: Int) : ValidationError() {
        override fun toString() = super.toString()
    }

    data class Nested(val field: String, val index: Int?, val source: ValidationError) : ValidationError() {
        override fun toString() = super.toString()
    }
}

/** Carries a [ValidationError] out of the generated validators; only [runValidation] observes it. */
@PublishedApi
internal class ValidationException(val error: ValidationError) : RuntimeException() {
    override fun fillInStackTrace(): Throwable = this
}

/** Runs strict checks and returns the first error they report, or null when everything held. */
fun runValidation(body: () -> Unit): ValidationError? = try {
    body()
    null
} catch (e: ValidationException) {
    e.error
}

/** Wraps errors reported by `body` as having been found inside `field` (element `index`). */
inline fun nest(field: String, index: Int? = null, body: () -> Unit) {
    try {
        body()
    } catch (e: ValidationException) {
        throw ValidationException(ValidationError.Nested(field, index, e.error))
    }
}

/** A reference slot as read from the buffer: payload-relative offset and length, both raw u32 values. */
data class Slot(val offset: Long, val length: Long)

/** A region the validator located inside the payload: payload-relative offset and element count. */
data class Region(val offset: Int, val count: Int)

/** An array/map region with the writer's shape bit split off. */
data class ShapedRegion(val offset: Int, val count: Int, val instances: Boolean)

/** The `[lo, hi)` extent of a self-contained struct instance. */
data class Extent(val lo: Int, val hi: Int)

/**
 * A decoded buffer: the bytes are copied once into memory owned by this object, and every read
 * afterwards is a plain little-endian load from the payload (the bytes after the header). Reads
 * out of bounds throw (like Rust); call `validate` first on untrusted input.
 */
class RawMessage private constructor(
    val key: UInt,
    val fixedLen: Int,
    /** The writer's highest extension version. */
    val version: Int,
    val flags: Int,
    internal val payload: ByteArray,
    private val buf: ByteBuffer,
) {
    /** Payload length (header excluded); every offset in generated code is relative to it. */
    val payloadSize: Int get() = payload.size

    companion object {
        /** Lazy decode: null when the buffer is too short for the header plus the fixed region, or when the key does not match. */
        fun decode(bytes: ByteArray, key: UInt, fixedLen: Int): RawMessage? {
            if (bytes.size < MESSAGE_HEADER_LEN + fixedLen) return null
            val m = of(bytes)
            return if (m.key == key) m else null
        }

        /** Strict decode: every header field must match the compiled schema. */
        fun validating(bytes: ByteArray, key: UInt, fixedLen: Int): RawMessage {
            val need = MESSAGE_HEADER_LEN + fixedLen
            if (bytes.size < need) throw ValidationException(ValidationError.TooShort(need, bytes.size))
            val m = of(bytes)
            if (m.key != key) throw ValidationException(ValidationError.KeyMismatch(key, m.key))
            if (m.fixedLen != fixedLen) {
                throw ValidationException(ValidationError.FixedLenMismatch(fixedLen, m.fixedLen))
            }
            return m
        }

        private fun of(bytes: ByteArray): RawMessage {
            val header = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
            val payload = bytes.copyOfRange(MESSAGE_HEADER_LEN, bytes.size)
            return RawMessage(
                key = header.getInt(0).toUInt(),
                fixedLen = header.getShort(4).toInt() and 0xFFFF,
                version = header.get(6).toInt() and 0xFF,
                flags = header.get(7).toInt() and 0xFF,
                payload = payload,
                buf = ByteBuffer.wrap(payload).order(ByteOrder.LITTLE_ENDIAN),
            )
        }
    }

    private fun bound(off: Int, n: Int) {
        if (off < 0 || off + n > payload.size) {
            throw IndexOutOfBoundsException("message read out of bounds: off=$off n=$n payload=${payload.size}")
        }
    }

    // ---- Fixed-size little-endian reads (offsets are payload-relative) ----

    fun readU8(off: Int): UByte { bound(off, 1); return buf.get(off).toUByte() }
    fun readI8(off: Int): Byte { bound(off, 1); return buf.get(off) }
    fun readBool(off: Int): Boolean { bound(off, 1); return buf.get(off) != 0.toByte() }
    fun readU16(off: Int): UShort { bound(off, 2); return buf.getShort(off).toUShort() }
    fun readI16(off: Int): Short { bound(off, 2); return buf.getShort(off) }
    fun readU32(off: Int): UInt { bound(off, 4); return buf.getInt(off).toUInt() }
    fun readI32(off: Int): Int { bound(off, 4); return buf.getInt(off) }
    fun readU64(off: Int): ULong { bound(off, 8); return buf.getLong(off).toULong() }
    fun readI64(off: Int): Long { bound(off, 8); return buf.getLong(off) }
    fun readF32(off: Int): Float { bound(off, 4); return buf.getFloat(off) }
    fun readF64(off: Int): Double { bound(off, 8); return buf.getDouble(off) }

    /** Reference slot at `off`. */
    fun readSlot(off: Int): Slot {
        bound(off, SLOT_LEN)
        return Slot(buf.getInt(off).toLong() and 0xFFFFFFFFL, buf.getInt(off + 4).toLong() and 0xFFFFFFFFL)
    }

    /** Reads the slot at `pos` and hands its offset and length (as Int positions) to `build`. */
    inline fun <T> viewAt(pos: Int, build: (off: Int, len: Int) -> T): T {
        val s = readSlot(pos)
        return build(s.offset.toInt(), s.length.toInt())
    }

    fun bitmapBit(block: Int, bit: Int): Boolean = (readU8(block + (bit shr 3)).toInt() shr (bit and 7)) and 1 == 1

    /** UTF-8 string slot (invalid sequences are replaced; `validate` rejects them). */
    fun readString(off: Int): String {
        val s = readSlot(off)
        val o = s.offset.toInt()
        return String(payload, o, slotEnd(s) - o, Charsets.UTF_8)
    }

    /** Bytes slot, copied. */
    fun readBytes(off: Int): ByteArray {
        val s = readSlot(off)
        val o = s.offset.toInt()
        return payload.copyOfRange(o, slotEnd(s))
    }

    private fun slotEnd(s: Slot): Int {
        val end = s.offset + s.length
        if (end > payload.size) {
            throw IndexOutOfBoundsException("slot out of bounds: off=${s.offset} len=${s.length} payload=${payload.size}")
        }
        return end.toInt()
    }

    /**
     * Locate an extension block by walking back from `hi` (default: the payload end; the instance
     * end for self-contained structs), never crossing below `lo`; returns its payload start
     * (trailer excluded). Blocks newer than `target` are stepped over even when this reader does
     * not know them. A block shorter than `minLen` (its fixed region) counts as absent. Instance
     * readers pass `hv = INSTANCE_HV` (no version is promised inside an instance).
     */
    fun findExtBlock(versions: IntArray, hv: Int, target: Int, minLen: Int, lo: Int = 0, hi: Int = payload.size): Int? {
        val maxKnown = versions.lastOrNull() ?: 0
        var pos = hi
        while (pos >= lo + EXT_TRAILER_LEN) {
            val since = readU16(pos - 6).toInt()
            val blen = readU32(pos - 4).toLong()
            if (since < 2 || since > hv || blen < EXT_TRAILER_LEN || blen > pos - lo) return null
            val start = pos - blen.toInt()
            if (since == target) return if (pos - EXT_TRAILER_LEN - start >= minLen) start else null
            if (since > target && (since > maxKnown || versions.contains(since))) {
                pos = start
                continue
            }
            return null
        }
        return null
    }

    /** True when the bytes of `s` are well-formed UTF-8: a lossless decode round trip implies valid input, since a malformed sequence is replaced by U+FFFD and cannot re-encode to itself. */
    internal fun isValidUtf8(s: Slot): Boolean {
        val o = s.offset.toInt()
        val l = s.length.toInt()
        val back = String(payload, o, l, Charsets.UTF_8).toByteArray(Charsets.UTF_8)
        if (back.size != l) return false
        for (i in 0 until l) if (back[i] != payload[o + i]) return false
        return true
    }
}

// ---- Contracts the generated code relies on ----

/** i32-backed enum (`enum class Color(override val value: Int) : MessageEnum`). */
interface MessageEnum {
    val value: Int
}

/** Fixed-width value readable from one position (scalars and enums), for [ScalarListView]. */
interface MessageScalar<T> {
    val width: Int
    fun read(m: RawMessage, off: Int): T

    companion object {
        val BOOL: MessageScalar<Boolean> = scalar(1) { m, o -> m.readBool(o) }
        val U8: MessageScalar<UByte> = scalar(1) { m, o -> m.readU8(o) }
        val I8: MessageScalar<Byte> = scalar(1) { m, o -> m.readI8(o) }
        val U16: MessageScalar<UShort> = scalar(2) { m, o -> m.readU16(o) }
        val I16: MessageScalar<Short> = scalar(2) { m, o -> m.readI16(o) }
        val U32: MessageScalar<UInt> = scalar(4) { m, o -> m.readU32(o) }
        val I32: MessageScalar<Int> = scalar(4) { m, o -> m.readI32(o) }
        val U64: MessageScalar<ULong> = scalar(8) { m, o -> m.readU64(o) }
        val I64: MessageScalar<Long> = scalar(8) { m, o -> m.readI64(o) }
        val F32: MessageScalar<Float> = scalar(4) { m, o -> m.readF32(o) }
        val F64: MessageScalar<Double> = scalar(8) { m, o -> m.readF64(o) }

        private fun <T> scalar(width: Int, read: (RawMessage, Int) -> T): MessageScalar<T> =
            object : MessageScalar<T> {
                override val width = width
                override fun read(m: RawMessage, off: Int): T = read(m, off)
            }
    }
}

/** Implemented by the companion of every generated enum: lookup by wire value. */
interface MessageEnumCompanion<E : MessageEnum> : MessageScalar<E> {
    val enumName: String
    fun fromValue(v: Int): E?

    override val width: Int get() = 4
    override fun read(m: RawMessage, off: Int): E {
        val v = m.readI32(off)
        return fromValue(v) ?: throw IllegalStateException("unknown $enumName value $v")
    }
}

/**
 * Zero-copy struct view over `[origin, end)` of the payload (the payload for top-level messages
 * and open elements, the instance for self-contained structs).
 */
interface MessageReader {
    val msg: RawMessage
    val origin: Int
    val end: Int
    val hv: Int
}

// ---- Lazy views ----

fun checkIndex(i: Int, size: Int) {
    if (i < 0 || i >= size) throw IndexOutOfBoundsException("index $i out of bounds (len $size)")
}

class ScalarListView<T>(val msg: RawMessage, val start: Int, override val size: Int, private val scalar: MessageScalar<T>) :
    AbstractList<T>() {
    override fun get(index: Int): T {
        checkIndex(index, size)
        return scalar.read(msg, start + index * scalar.width)
    }
}

class StringListView(val msg: RawMessage, val start: Int, override val size: Int) : AbstractList<String>() {
    override fun get(index: Int): String {
        checkIndex(index, size)
        return msg.readString(start + index * SLOT_LEN)
    }
}

class BytesListView(val msg: RawMessage, val start: Int, override val size: Int) : AbstractList<ByteArray>() {
    override fun get(index: Int): ByteArray {
        checkIndex(index, size)
        return msg.readBytes(start + index * SLOT_LEN)
    }
}

/** Raw slot view for nested dynamic arrays: each element is an 8-byte slot. */
class SlotListView(val msg: RawMessage, val start: Int, override val size: Int) : AbstractList<Slot>() {
    override fun get(index: Int): Slot {
        checkIndex(index, size)
        return msg.readSlot(start + index * SLOT_LEN)
    }
}

/**
 * Struct elements of an array region whose slot length carries the shape bit (`rawLen` holds the
 * raw u32 bits). shape=0: open fixed-region footprints of `fixedLen` bytes (readers see the payload
 * as their extent and hv=0, so extension fields are absent); shape=1: 8-byte slots, each pointing
 * at a self-contained instance whose extent bounds the reader (hv = INSTANCE_HV). `factory` builds
 * a view at `(msg, origin, end, hv)`.
 */
class StructEvoListView<T>(
    val msg: RawMessage,
    val start: Int,
    private val rawLen: Long,
    private val fixedLen: Int,
    private val factory: (RawMessage, Int, Int, Int) -> T,
) : AbstractList<T>() {
    override val size: Int get() = (rawLen and LEN_MASK.toLong()).toInt()

    /** Whether the writer emitted per-element instances (its element type had extension fields). */
    val isInstanceMode: Boolean get() = (rawLen and 0x8000_0000L) != 0L

    override fun get(index: Int): T {
        checkIndex(index, size)
        if (isInstanceMode) {
            val s = msg.readSlot(start + index * SLOT_LEN)
            return factory(msg, s.offset.toInt(), (s.offset + s.length).toInt(), INSTANCE_HV)
        }
        return factory(msg, start + index * fixedLen, msg.payloadSize, 0)
    }
}

/** Immutable map entry, as yielded by the generated map views. */
class MapEntry<K, V>(override val key: K, override val value: V) : Map.Entry<K, V>

/** Orders strings by their UTF-8 bytes, the order Rust's `str` uses (and so the canonical map key order). */
val UTF8_STRING_ORDER: Comparator<String> = Comparator { a, b ->
    val x = a.toByteArray(Charsets.UTF_8)
    val y = b.toByteArray(Charsets.UTF_8)
    for (i in 0 until minOf(x.size, y.size)) {
        val d = (x[i].toInt() and 0xFF) - (y[i].toInt() and 0xFF)
        if (d != 0) return@Comparator d
    }
    x.size - y.size
}

// ---- Encoder ----

/**
 * Growable little-endian byte buffer. Positions are absolute (the 8-byte header included);
 * [rel] turns them into the payload-relative values slots carry.
 */
class Writer(capacity: Int = 64) {
    private var bytes = ByteArray(maxOf(16, capacity))
    private var buf = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
    private var len = 0

    fun writeU8(pos: Int, v: UByte) { bytes[pos] = v.toByte() }
    fun writeI8(pos: Int, v: Byte) { bytes[pos] = v }
    fun writeBool(pos: Int, v: Boolean) { bytes[pos] = if (v) 1 else 0 }
    fun writeU16(pos: Int, v: UShort) { buf.putShort(pos, v.toShort()) }
    fun writeI16(pos: Int, v: Short) { buf.putShort(pos, v) }
    fun writeU32(pos: Int, v: UInt) { buf.putInt(pos, v.toInt()) }
    fun writeI32(pos: Int, v: Int) { buf.putInt(pos, v) }
    fun writeU64(pos: Int, v: ULong) { buf.putLong(pos, v.toLong()) }
    fun writeI64(pos: Int, v: Long) { buf.putLong(pos, v) }
    fun writeF32(pos: Int, v: Float) { buf.putFloat(pos, v) }
    fun writeF64(pos: Int, v: Double) { buf.putDouble(pos, v) }
    fun writeSlot(pos: Int, off: Int, len: Int) { buf.putInt(pos, off); buf.putInt(pos + 4, len) }

    fun setBit(block: Int, bit: Int) {
        val i = block + (bit shr 3)
        bytes[i] = (bytes[i].toInt() or (1 shl (bit and 7))).toByte()
    }

    private fun ensure(extra: Int) {
        if (len + extra > bytes.size) {
            var cap = bytes.size * 2
            while (cap < len + extra) cap *= 2
            bytes = bytes.copyOf(cap)
            buf = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        }
    }

    /** Reserve `n` zeroed bytes, returning the absolute start. */
    fun reserve(n: Int): Int {
        ensure(n)
        val start = len
        len += n
        return start
    }

    /** Payload-relative offset of an absolute position. */
    fun rel(abs: Int): Int = abs - MESSAGE_HEADER_LEN

    /** Bytes written so far (absolute; the header included). */
    val length: Int get() = len

    /** Append the UTF-8 bytes of `s` and point the slot at `pos` at them. */
    fun writeStringSlot(pos: Int, s: String) = writeBytesSlot(pos, s.toByteArray(Charsets.UTF_8))

    /** Append `data` and point the slot at `pos` at it. */
    fun writeBytesSlot(pos: Int, data: ByteArray) {
        ensure(data.size)
        val off = rel(len)
        System.arraycopy(data, 0, bytes, len, data.size)
        len += data.size
        writeSlot(pos, off, data.size)
    }

    /** Append the `since | block_len` trailer of the block starting at `block` (absolute). */
    fun writeTrailer(block: Int, version: Int) {
        val t = reserve(EXT_TRAILER_LEN)
        buf.putShort(t, version.toShort())
        buf.putInt(t + 2, len - block)
    }

    /** Exact-size copy of everything written. */
    fun finish(): ByteArray = bytes.copyOf(len)
}

// ---- Content equality for data classes holding ByteArray ----

/** Structural equality that compares [ByteArray] by content, also inside lists and maps. */
fun contentEquals(a: Any?, b: Any?): Boolean = when {
    a is ByteArray -> b is ByteArray && a.contentEquals(b)
    a is List<*> -> b is List<*> && a.size == b.size && a.indices.all { contentEquals(a[it], b[it]) }
    a is Map<*, *> -> b is Map<*, *> && a.size == b.size && a.all { (k, v) -> b.containsKey(k) && contentEquals(v, b[k]) }
    else -> a == b
}

/** Hash consistent with [contentEquals]. */
fun contentHashCode(v: Any?): Int = when (v) {
    null -> 0
    is ByteArray -> v.contentHashCode()
    is List<*> -> v.fold(1) { h, e -> 31 * h + contentHashCode(e) }
    is Map<*, *> -> v.entries.sumOf { it.key.hashCode() xor contentHashCode(it.value) }
    else -> v.hashCode()
}

// ---- Strict validation ----

/** The slot at `pos` must stay inside the payload. */
fun checkBytesSlot(m: RawMessage, pos: Int): Region {
    val s = m.readSlot(pos)
    if (s.offset + s.length > m.payloadSize) throw fail(s, m)
    return Region(s.offset.toInt(), s.length.toInt())
}

fun checkStrSlot(m: RawMessage, pos: Int) {
    val s = m.readSlot(pos)
    if (s.offset + s.length > m.payloadSize) throw fail(s, m)
    if (!m.isValidUtf8(s)) throw ValidationException(ValidationError.InvalidUtf8)
}

/** The slot at `pos` holds an element count of `stride`-sized elements; the whole region must lie inside the payload. */
fun checkRegion(m: RawMessage, pos: Int, stride: Int): Region {
    val s = m.readSlot(pos)
    val count = s.length and LEN_MASK.toLong()
    if (s.offset + count * stride > m.payloadSize) throw fail(s, m)
    return Region(s.offset.toInt(), count.toInt())
}

/** Array/map region slot split into (offset, count, shape); the extent is checked by [checkExtent]. */
fun checkRegionShape(m: RawMessage, pos: Int, field: String, index: Int? = null): ShapedRegion {
    val s = m.readSlot(pos)
    val count = s.length and LEN_MASK.toLong()
    if (s.offset + count > m.payloadSize) {
        throw ValidationException(ValidationError.Nested(field, index, ValidationError.SlotOutOfBounds(s.offset, s.length, m.payloadSize)))
    }
    return ShapedRegion(s.offset.toInt(), count.toInt(), (s.length and 0x8000_0000L) != 0L)
}

/** `count` elements of `stride` bytes starting at `off` must lie inside the payload. */
fun checkExtent(m: RawMessage, off: Int, count: Int, stride: Int, field: String, index: Int? = null) {
    if (off.toLong() + count.toLong() * stride > m.payloadSize) {
        throw ValidationException(
            ValidationError.Nested(field, index, ValidationError.SlotOutOfBounds(off.toLong(), count.toLong(), m.payloadSize)),
        )
    }
}

/** The instance slot at `pos` must stay inside the payload; returns its `[lo, hi)` extent. */
fun checkInstance(m: RawMessage, pos: Int): Extent {
    val r = checkBytesSlot(m, pos)
    return Extent(r.offset, r.offset + r.count)
}

/** Same, with a region failure located at `field` (element `index`) like the element checks are. */
fun checkRegion(m: RawMessage, pos: Int, stride: Int, field: String, index: Int? = null): Region = try {
    checkRegion(m, pos, stride)
} catch (e: ValidationException) {
    throw ValidationException(ValidationError.Nested(field, index, e.error))
}

/** The i32 at `pos` must be a known variant of `e` (pass the enum's companion). */
fun checkEnum(e: MessageEnumCompanion<*>, m: RawMessage, pos: Int) {
    val v = m.readI32(pos)
    if (e.fromValue(v) == null) throw ValidationException(ValidationError.UnknownEnumValue(e.enumName, v))
}

/** Locate an extension block the writer must have written (version <= header version). */
fun checkExtBlock(m: RawMessage, versions: IntArray, hv: Int, target: Int, minLen: Int): Int =
    m.findExtBlock(versions, hv, target, minLen) ?: throw ValidationException(ValidationError.ExtBlock(target))

private fun fail(s: Slot, m: RawMessage): ValidationException =
    ValidationException(ValidationError.SlotOutOfBounds(s.offset, s.length, m.payloadSize))

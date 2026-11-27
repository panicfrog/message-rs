package message.rs.hub

import message.rs.MessageCodecCompanion
import message.rs.MessageEncodable

fun Boolean.asHubValue() = Value.Bool(this)
fun UByte.asHubValue() = Value.U8(this.toInt())
fun Byte.asHubValue() = Value.I8(this.toInt())
fun UShort.asHubValue() = Value.U16(this.toInt())
fun Short.asHubValue() = Value.I16(this.toInt())
fun UInt.asHubValue() = Value.U32(this.toLong())
fun Int.asHubValue() = Value.I32(this)
fun ULong.asHubValue() = Value.U64(this.toLong())
fun Long.asHubValue() = Value.I64(this)
fun Float.asHubValue() = Value.F32(this)
fun Double.asHubValue() = Value.F64(this)
fun String.asHubValue() = Value.Str(this)
fun ByteArray.asHubValue() = Value.Bytes(this)

/** A message-rs payload without a schema on hand: decode it yourself with the generated `XxxReader`. */
class MessageBytes(val bytes: ByteArray)

/**
 * Resolves any supported Kotlin value to its [Value]; the kind follows the type. Kotlin's
 * unsigned types are distinct from the signed ones (`UShort` vs `Short` vs `Double`), so the
 * mapping is exact: no wrappers, no guessing. Generated data classes need no new branch either:
 * they already implement [MessageEncodable], checked once below for every current and future
 * generated type — conforming a new type to that protocol is the whole integration.
 */
fun hubValueOf(v: Any): Value = when (v) {
    is Value -> v
    is MessageEncodable -> v.hubValue
    is MessageBytes -> Value.Message(v.bytes)
    is Boolean -> v.asHubValue()
    is UByte -> v.asHubValue()
    is Byte -> v.asHubValue()
    is UShort -> v.asHubValue()
    is Short -> v.asHubValue()
    is UInt -> v.asHubValue()
    is Int -> v.asHubValue()
    is ULong -> v.asHubValue()
    is Long -> v.asHubValue()
    is Float -> v.asHubValue()
    is Double -> v.asHubValue()
    is String -> v.asHubValue()
    is ByteArray -> v.asHubValue()
    else -> throw IllegalArgumentException("cannot publish a value of type ${v::class.simpleName}")
}

/**
 * Every [MessageEncodable] type — generated data classes included — is publishable as-is:
 * `publish(1, user)`, no manual encode step. [MessageEncodable.encode] is exactly the method
 * generated code already has; this only supplies the [Value] wrapper. Adding a new encodable
 * type never touches this file. Pull back with `hub.pull(id, User.Companion)` — the companion
 * already implements [MessageCodecCompanion], so it needs no wiring either.
 */
val MessageEncodable.hubValue: Value get() = Value.Message(encode())

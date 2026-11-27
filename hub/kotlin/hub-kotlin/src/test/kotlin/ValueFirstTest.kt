package message.rs.hub

import message.rs.Profile
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ValueFirstTest {
    companion object {
        init {
            val dir = System.getProperty("hub.lib")!!
            val lib = listOf("libhub_jni.dylib", "libhub_jni.so", "hub_jni.dll")
                .firstNotNullOfOrNull { File(dir, it).takeIf(File::exists) }
            assertNotNull(lib)
            System.load(lib.absolutePath)
        }
    }

    @Test
    fun `publishes Kotlin types without naming kinds`() {
        val hub = Hub.create()
        hub.publish(1, true)
        hub.publish(2, 65535.toUShort())
        hub.publish(3, -1)
        hub.publish(4, "hi")
        hub.publish(5, 1.5)
        hub.publish(6, ULong.MAX_VALUE)
        hub.publish(7, byteArrayOf(1, 2, 3))

        // the kind follows the Kotlin type exactly
        assertEquals(ValueKind.U16, hub.get(2)?.kind)
        assertEquals(ValueKind.I32, hub.get(3)?.kind)
        assertEquals(ValueKind.F64, hub.get(5)?.kind)
        assertEquals(ValueKind.U64, hub.get(6)?.kind)
        assertEquals(ValueKind.BYTES, hub.get(7)?.kind)

        // pull returns decoded values
        assertEquals(Value.Bool(true), hub.pull(1))
        assertEquals(Value.U16(65535), hub.pull(2))
        assertEquals(Value.Str("hi"), hub.pull(4))
        assertEquals(Value.U64(-1L), hub.pull(6)) // ULong.MAX_VALUE bits
    }

    @Test
    fun `message bytes carry the message kind`() {
        val hub = Hub.create()
        hub.publish(8, MessageBytes(byteArrayOf(9, 8, 7)))
        assertEquals(ValueKind.MESSAGE, hub.get(8)?.kind)
        val v = hub.pull(8)
        assertTrue(v is Value.Message && v.v.contentEquals(byteArrayOf(9, 8, 7)))
    }

    @Test
    fun `close delivers Closed once and empties pull`() {
        val hub = Hub.create()
        var closed = 0
        var values = 0
        val sub = hub.subscribe(9) { _, kind, ptr, _ ->
            if (kind == CLOSED_KIND && ptr == 0L) closed++ else values++
        }
        hub.publish(9, 1.toUByte())
        hub.close(9)
        assertEquals(1, closed)
        assertEquals(1, values)
        assertNull(hub.pull(9))
        hub.close(9) // idempotent
        assertEquals(1, closed)
        sub.close()
    }

    @Test
    fun `changing type on an id supersedes the publisher`() {
        val hub = Hub.create()
        hub.publish(10, 1.toUShort())
        assertEquals(ValueKind.U16, hub.get(10)?.kind)
        hub.publish(10, "now a string")
        assertEquals(ValueKind.STRING, hub.get(10)?.kind)
        assertEquals(Value.Str("now a string"), hub.pull(10))
    }

    @Test
    fun `publishes and pulls a generated struct without manual encoding`() {
        // The whole point of the unified protocol: a MessageEncodable type (here, a real
        // generated data class) goes through publish/pull exactly like UShort does — no
        // encode()/fromBytes() call at the call site.
        val hub = Hub.create()
        val profile = Profile(age = 30u, bio = "hello", nickname = "yamada", verified = true)
        hub.publish(20, profile)
        assertEquals(ValueKind.MESSAGE, hub.get(20)?.kind)

        val pulled = hub.pull(20, Profile.Companion)
        assertEquals(profile, pulled)

        // the bytes on the wire are exactly what the schema's own encoder produces
        assertContentEquals(profile.encode(), hub.get(20)?.bytes)
    }

    @Test
    fun `pull of the wrong type is null not a trap`() {
        val hub = Hub.create()
        hub.publish(21, "not a Profile")
        assertNull(hub.pull(21, Profile.Companion))
    }

    @Test
    fun `a scheduled subscription receives the encoded value`() {
        val hub = Hub.create()
        val pool = Executors.newSingleThreadExecutor()
        try {
            val done = CountDownLatch(1)
            var got: ByteArray? = null
            val sub = hub.subscribeOn(11, pool) { _, kind, ptr, len ->
                if (kind != CLOSED_KIND) {
                    got = Hub.copyBytesPublic(ptr, len)
                    done.countDown()
                }
            }
            hub.publish(11, "hello")
            assertTrue(done.await(2, TimeUnit.SECONDS))
            assertContentEquals("hello".toByteArray(), got)
            sub.close()
        } finally {
            pool.shutdown()
        }
    }
}

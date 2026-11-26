package message.rs.hub

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asExecutor
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.runBlocking
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

// The library is loaded through the absolute path of the cargo-built dylib (tests must not
// depend on java.library.path being set by hand).
private val libDir = System.getProperty("hub.lib")!!

class HubJvmTest {
    companion object {
        init {
            val names = listOf("libhub_jni.dylib", "libhub_jni.so", "hub_jni.dll")
            val lib = names.firstNotNullOfOrNull { File(libDir, it).takeIf(File::exists) }
            assertNotNull(lib, "hub_jni native library not found in $libDir")
            System.load(lib.absolutePath)
            println("LOADED ${'$'}{lib.absolutePath}")
        }
    }

    private fun hub() = Hub.create()

    @Test
    fun `push pull and byte equality`() {
        val hub = hub()
        val p = hub.publisher(1, ValueKind.BYTES)
        val events = ArrayList<Int>()
        val s = hub.subscribe(1) { _, kind, _, _ -> events.add(kind) }

        assertNull(hub.get(1), "pull before any publish")

        p.publish(byteArrayOf(1, 2, 3))
        p.publish(Value.Bytes(byteArrayOf(4, 5)))

        assertEquals(listOf(12, 12), events.map { it })
        val pulled = assertNotNull(hub.get(1))
        assertTrue(pulled.bytes.contentEquals(byteArrayOf(4, 5)), "pull: the newest publish")
        assertEquals(ValueKind.BYTES, pulled.kind)

        // subscribing never replays
        var late = 0
        val s2 = hub.subscribe(1) { _, _, _, _ -> late++ }
        assertEquals(0, late)
        s2.close()
        s.close()
    }

    @Test
    fun `closed lifecycle revives subscriptions`() {
        val hub = hub()
        var closed = 0
        var values = 0
        val s = hub.subscribe(3) { _, kind, ptr, _ ->
            if (kind == CLOSED_KIND && ptr == 0L) closed++ else values++
        }
        val p = hub.publisher(3, ValueKind.STRING)
        p.publish(Value.Str("hi"))
        p.close()
        p.publish(Value.Str("gone")) // no-op after close

        assertEquals(1, closed, "Closed exactly once")
        assertEquals(1, values)
        assertNull(hub.get(3), "pull empty after the last publisher closed")

        // a new publisher reaches the surviving subscription
        val p2 = hub.publisher(3, ValueKind.STRING)
        p2.publish(Value.Str("again"))
        assertEquals(2, values, "the subscription survived Closed")
        s.close()
    }

    @Test
    fun `scheduled delivery lands on the executor thread`() {
        val hub = hub()
        val pool = Executors.newSingleThreadExecutor { Thread(it, "hub-scheduler") }
        try {
            var seenThread: Thread? = null
            var got: ByteArray? = null
            val done = CountDownLatch(1)
            val s = hub.subscribeOn(4, pool) { _, kind, ptr, len ->
                if (kind != CLOSED_KIND) {
                    seenThread = Thread.currentThread()
                    // The payload is only valid during the callback: copy inside it.
                    got = ByteArray(len) { i ->
                        // Read through MemoryReader-equivalent: native copy
                        Hub.copyBytesPublic(ptr, len)[i]
                    }
                    done.countDown()
                }
            }
            val p = hub.publisher(4, ValueKind.BYTES)
            p.publish(byteArrayOf(7, 8, 9))
            assertTrue(done.await(2, TimeUnit.SECONDS), "delivery arrived")
            assertEquals("hub-scheduler", seenThread?.name)
            assertTrue(got!!.contentEquals(byteArrayOf(7, 8, 9)))
            s.close()
        } finally {
            pool.shutdown()
        }
    }

    @Test
    fun `shared hub is one registry across wrapper instances`() {
        val a = Hub.shared
        val b = Hub.shared
        val p = a.publisher(5, ValueKind.BYTES)
        var received = 0
        val s = b.subscribe(5) { _, _, _, _ -> received++ }
        p.publish(byteArrayOf(1))
        assertEquals(1, received, "a publisher through one wrapper reaches the other")
        s.close()
    }

    @Test
    fun `value codec round trips every kind`() {
        fun roundTrip(v: Value): Value = Value.decode(v.kind, v.encode())
        assertEquals(Value.Bool(true), roundTrip(Value.Bool(true)))
        assertEquals(Value.U16(0x0102), roundTrip(Value.U16(0x0102)))
        assertEquals(Value.I16(-2), roundTrip(Value.I16(-2)))
        assertEquals(Value.U32(1L), roundTrip(Value.U32(1L)))
        assertEquals(Value.F32(1.5f), roundTrip(Value.F32(1.5f)))
        assertEquals(Value.F64(-0.5), roundTrip(Value.F64(-0.5)))
        assertEquals(Value.Str("山"), roundTrip(Value.Str("山")))
        val rb = roundTrip(Value.Bytes(byteArrayOf(1, 2)))
        assertTrue(rb is Value.Bytes && rb.v.contentEquals(byteArrayOf(1, 2)))
    }

    @Test
    fun `pull bytes match the rust message vector`() {
        // The committed vector is produced by the Rust fixtures; going through the hub is the
        // cross-language contract (decode is the schema's job, covered elsewhere).
        val hex = File(repoRoot(), "vectors/user.json").readText()
            .let { Regex("\"hex\"\\s*:\\s*\"([0-9a-f]+)\"").find(it)!!.groupValues[1] }
        val bytes = hexToBytes(hex)
        val hub = hub()
        val p = hub.publisher(1, ValueKind.MESSAGE) // User's key is 1
        p.publish(bytes)
        val pulled = assertNotNull(hub.get(1))
        assertEquals(ValueKind.MESSAGE, pulled.kind)
        assertTrue(pulled.bytes.contentEquals(bytes), "byte-for-byte through the hub")
    }


    @Test
    fun `probe minimal single publish`() = runBlocking {
        val hub = hub()
        val p = hub.publisher(60, ValueKind.STRING)
        var calls = 0
        val s = hub.subscribeOn(60, Executors.newSingleThreadExecutor()) { id, kind, ptr, len ->
            calls++
            println("PROBE call# " + calls + " kind=" + kind + " ptr=" + ptr + " len=" + len)
        }
        kotlinx.coroutines.delay(100)
        p.publish(Value.Str("x"))
        val deadline = System.currentTimeMillis() + 1000
        while (calls < 1 && System.currentTimeMillis() < deadline) kotlinx.coroutines.delay(10)
        kotlinx.coroutines.delay(200)
        assertEquals(1, calls, "one publish = exactly one delivery")
        s.close()
    }

    @Test
    fun `flow events deliver values and closed`() = runBlocking {
        val hub = hub()
        val p = hub.publisher(6, ValueKind.STRING)
        val collected = ArrayList<HubEvent>()
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val job = hub.events(6, java.util.concurrent.Executors.newSingleThreadExecutor())
            .onEach { collected.add(it) }
            .launchIn(scope)
        // let the subscription register, then push
        kotlinx.coroutines.delay(100)
        p.publish(Value.Str("one"))
        p.publish(Value.Str("two"))
        p.close()
        val deadline = System.currentTimeMillis() + 2000
        while (collected.size < 3 && System.currentTimeMillis() < deadline) {
            kotlinx.coroutines.delay(10)
        }
        scope.cancel()
        job.cancel()

        assertEquals(3, collected.size)
        assertEquals("one", String(collected[0].bytes!!))
        assertEquals("two", String(collected[1].bytes!!))
        assertTrue(collected[2].isClosed)
    }

    @Test
    fun `state mirror tracks the newest value and closed`() = runBlocking {
        val hub = hub()
        val p = hub.publisher(7, ValueKind.STRING)
        val state = HubState(hub, 7, Dispatchers.Default.asExecutor())
        assertNull(state.last.value)

        p.publish(Value.Str("hello"))
        state.refresh() // pull-first
        assertEquals("hello", String(state.last.value!!.bytes))

        p.publish(Value.Str("world"))
        val deadline = System.currentTimeMillis() + 2000
        while (state.last.value?.bytes?.let { !it.contentEquals("world".toByteArray()) } == true
            && System.currentTimeMillis() < deadline
        ) {
            kotlinx.coroutines.delay(10)
        }
        assertEquals("world", String(state.last.value!!.bytes))

        p.close()
        val closedDeadline = System.currentTimeMillis() + 2000
        while (!state.closed.value && System.currentTimeMillis() < closedDeadline) {
            kotlinx.coroutines.delay(10)
        }
        assertTrue(state.closed.value)
        state.close()
    }
}

private fun hexToBytes(hex: String): ByteArray =
    ByteArray(hex.length / 2) { i -> hex.substring(i * 2, i * 2 + 2).toInt(16).toByte() }

private fun repoRoot(): File {
    var dir = File(HubJvmTest::class.java.protectionDomain.codeSource.location.toURI()).parentFile
    while (dir != null && !File(dir, "vectors/user.json").exists()) dir = dir.parentFile
    return assertNotNull(dir, "repository root not found from ${'$'}{HubJvmTest::class.java.protectionDomain.codeSource.location}")
}

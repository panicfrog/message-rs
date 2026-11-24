package message.rs

import java.io.File
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

// The committed vectors are produced by the Rust fixtures (`cargo run -p message-fixtures --bin
// export-schema -- vectors`); the IR snapshot is only used by the tests to locate fields.
private val repoRoot: File =
    generateSequence(File(".").canonicalFile) { it.parentFile }.first { File(it, "vectors/user.json").exists() }

@Suppress("UNCHECKED_CAST")
private fun loadJson(name: String): Map<String, Any?> =
    Json.parse(File(repoRoot, "vectors/$name").readText(Charsets.UTF_8)) as Map<String, Any?>

/** Minimal JSON reader for the vector files (objects keep their file order, integers are Long). */
private object Json {
    fun parse(text: String): Any? = Parser(text).run { value() }

    private class Parser(private val s: String) {
        private var i = 0

        fun value(): Any? {
            skipWs()
            return when (val c = s[i]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> { i += 4; true }
                'f' -> { i += 5; false }
                'n' -> { i += 4; null }
                else -> num()
            }
        }

        private fun obj(): Map<String, Any?> {
            val m = LinkedHashMap<String, Any?>()
            i++
            skipWs()
            if (s[i] == '}') { i++; return m }
            while (true) {
                skipWs()
                val k = str()
                skipWs()
                expectChar(':')
                m[k] = value()
                skipWs()
                if (s[i] == ',') i++ else { expectChar('}'); return m }
            }
        }

        private fun arr(): List<Any?> {
            val l = ArrayList<Any?>()
            i++
            skipWs()
            if (s[i] == ']') { i++; return l }
            while (true) {
                l.add(value())
                skipWs()
                if (s[i] == ',') i++ else { expectChar(']'); return l }
            }
        }

        private fun num(): Any {
            skipWs()
            val start = i
            if (s[i] == '-') i++
            while (i < s.length && (s[i].isDigit() || s[i] in ".eE+-")) i++
            val t = s.substring(start, i)
            return t.toLongOrNull() ?: t.toDouble()
        }

        private fun str(): String {
            expectChar('"')
            val sb = StringBuilder()
            while (true) {
                val c = s[i++]
                if (c == '"') return sb.toString()
                if (c != '\\') { sb.append(c); continue }
                when (val e = s[i++]) {
                    'n' -> sb.append('\n')
                    't' -> sb.append('\t')
                    'r' -> sb.append('\r')
                    'u' -> { sb.append(s.substring(i, i + 4).toInt(16).toChar()); i += 4 }
                    else -> sb.append(e)
                }
            }
        }

        private fun skipWs() {
            while (i < s.length && s[i].isWhitespace()) i++
        }

        private fun expectChar(c: Char) {
            check(i < s.length && s[i] == c) { "expected '$c' at $i" }
            i++
        }
    }
}

private fun hexToBytes(hex: String): ByteArray = ByteArray(hex.length / 2) { hex.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

private fun ByteArray.toHex(): String = joinToString("") { (it.toInt() and 0xFF).toString(16).padStart(2, '0') }

private fun putU32(bytes: ByteArray, pos: Int, v: UInt) {
    bytes[pos] = v.toByte()
    bytes[pos + 1] = (v shr 8).toByte()
    bytes[pos + 2] = (v shr 16).toByte()
    bytes[pos + 3] = (v shr 24).toByte()
}

private fun getU32(bytes: ByteArray, pos: Int): UInt =
    (bytes[pos].toUInt() and 0xFFu) or ((bytes[pos + 1].toUInt() and 0xFFu) shl 8) or
        ((bytes[pos + 2].toUInt() and 0xFFu) shl 16) or ((bytes[pos + 3].toUInt() and 0xFFu) shl 24)

private data class FieldIr(val name: String, val offset: Int, val mapValueOffset: Int?, val elementSize: Int?)

@Suppress("UNCHECKED_CAST")
private fun field(msg: String, name: String): FieldIr {
    val messages = loadJson("message-snapshot.json")["messages"] as List<Map<String, Any?>>
    val m = messages.first { it["name"] == msg }
    val f = (m["fields"] as List<Map<String, Any?>>).first { it["name"] == name }
    fun int(k: String) = (f[k] as Long?)?.toInt()
    return FieldIr(name, int("offset")!!, int("mapValueOffset"), int("elementSize"))
}

/** Absolute buffer position of a base field (header included). */
private fun posOf(msg: String, name: String): Int = 8 + field(msg, name).offset

@Suppress("UNCHECKED_CAST")
private fun userVector(): Pair<String, Map<String, Any?>> {
    val v = loadJson("user.json")
    return v["hex"] as String to v["expected"] as Map<String, Any?>
}

@Suppress("UNCHECKED_CAST")
private fun tallyVector(): Pair<String, Map<String, Any?>> {
    val v = loadJson("tally.json")
    return v["hex"] as String to v["expected"] as Map<String, Any?>
}

/** Mirrors `message_fixtures::sample_user()`. */
private fun sampleUser() = User(
    id = 42uL, name = "山田", color = Color.GREEN,
    profile = Profile(age = 30u, bio = "hello 世界", nickname = "yamada"),
    matrix = listOf(1.5f, -2.25f, 3.125f, 0.0f),
    nums = listOf(Int.MIN_VALUE, -1, 0, 7, Int.MAX_VALUE),
    tags = listOf("a", "b", "c"),
    blobs = listOf(byteArrayOf(1, 2, 3), byteArrayOf(), ByteArray(300) { 255.toByte() }),
    edges = listOf(Node(value = 1u, children = listOf(Node(value = 11u, children = emptyList()))), Node(value = 2u, children = emptyList())),
    counts = mapOf("one" to 1u, "two" to 2u),
    byId = mapOf(7uL to Profile(age = 25u, bio = "nested", nickname = null)),
    avatar = byteArrayOf(9, 8, 7), motto = null,
    stamp = -1_700_000_000_000L, ratio = 3.141592653589793,
    flag = true, small = 200u, wide = 65535u, offset = -32768, preciseId = ULong.MAX_VALUE,
    email = "user@example.com", score = 99.5, tags2 = listOf("v3"), vip = true,
)

class UserConformanceTests {
    @Test
    fun decodeMatchesRustEncodedBytes() {
        val (hex, e) = userVector()
        val r = assertNotNull(UserReader.decode(hexToBytes(hex)))
        assertEquals(3, r.hv)
        assertEquals((e["id"] as Long).toULong(), r.id)
        assertEquals(e["name"], r.name)
        assertEquals(Color.GREEN, r.color)
        val profile = e["profile"] as Map<*, *>
        assertEquals((profile["age"] as Long).toUInt(), r.profile.age)
        assertEquals(profile["bio"], r.profile.bio)
        assertEquals(profile["nickname"], r.profile.nickname)
        assertEquals(listOf(1.5f, -2.25f, 3.125f, 0.0f), r.matrix.toList())
        assertEquals(listOf(Int.MIN_VALUE, -1, 0, 7, Int.MAX_VALUE), r.nums.toList())
        assertEquals(listOf("a", "b", "c"), r.tags.toList())
        assertEquals(3, r.blobs.size)
        assertContentEquals(byteArrayOf(1, 2, 3), r.blobs[0])
        assertContentEquals(byteArrayOf(), r.blobs[1])
        assertEquals(300, r.blobs[2].size)
        assertEquals(2, r.edges.size)
        assertEquals(1u, r.edges[0].value)
        assertEquals(11u, r.edges[0].children[0].value)
        assertEquals(0, r.edges[0].children[0].children.size)
        assertEquals(listOf("one" to 1u, "two" to 2u), r.counts.map { it.key to it.value })
        assertEquals(1, r.byId.size)
        assertEquals(7uL, r.byId.key(0))
        assertEquals("nested", r.byId.value(0).bio)
        assertContentEquals(byteArrayOf(9, 8, 7), r.avatar)
        assertNull(r.motto)
        assertEquals(-1_700_000_000_000L, r.stamp)
        assertEquals(3.141592653589793, r.ratio)
        assertTrue(r.flag)
        assertEquals(200u, r.small)
        assertEquals(65535u, r.wide)
        assertEquals(-32768, r.offset)
        assertEquals(ULong.MAX_VALUE, r.preciseId)
        // extension-block fields (v2 / v3)
        assertEquals("user@example.com", r.email)
        assertEquals(99.5, r.score)
        assertEquals(listOf("v3"), r.tags2.toList())
        assertEquals(true, r.vip)
        // views are read-only Lists: indexing and iteration read the buffer lazily
        assertEquals("b", r.tags[1])
        assertEquals(listOf(1.5f, -2.25f), r.matrix.take(2))
    }

    @Test
    fun handWrittenValueEncodesToTheRustBytes() {
        val (hex, _) = userVector()
        assertEquals(hex, sampleUser().encode().toHex())
    }

    @Test
    fun ownedRoundtripIsByteIdentical() {
        val (hex, _) = userVector()
        val data = hexToBytes(hex)
        val owned = assertNotNull(User.fromBytes(data))
        // ByteArray fields compare by content, also inside lists
        assertEquals(sampleUser(), owned)
        assertEquals(sampleUser().hashCode(), owned.hashCode())
        assertEquals(hex, owned.encode().toHex())
        assertEquals(hex, User.fromBytes(owned.encode())!!.encode().toHex())
    }

    @Test
    fun lazyDecodeRejectsOnlyShortBuffersAndForeignKeys() {
        val (hex, _) = userVector()
        assertNull(UserReader.decode(hexToBytes(hex).copyOf(10)))
        val foreign = hexToBytes(hex)
        foreign[0] = (foreign[0].toInt() xor 0xFF).toByte()
        assertNull(UserReader.decode(foreign))
    }

    @Test
    fun lazyAccessorsThrowOnACorruptBuffer() {
        val (hex, _) = userVector()
        val bytes = hexToBytes(hex)
        putU32(bytes, posOf("User", "name"), UInt.MAX_VALUE)
        val r = assertNotNull(UserReader.decode(bytes))
        assertFailsWith<IndexOutOfBoundsException> { r.name }
        putU32(bytes, posOf("User", "color"), 99u)
        assertFailsWith<IllegalStateException> { UserReader.decode(bytes)!!.color }
    }

    @Test
    fun decodeCopiesTheBuffer() {
        val (hex, _) = userVector()
        val bytes = hexToBytes(hex)
        val r = assertNotNull(UserReader.decode(bytes))
        bytes.fill(0)
        assertEquals("山田", r.name)
        assertEquals(Color.GREEN, r.color)
    }
}

class TallyConformanceTests {
    private fun sampleTally() = Tally(
        usage = mapOf(Color.BLUE to 3u, Color.RED to 1u),
        labels = mapOf("a" to listOf("x", "y"), "b" to emptyList()),
        aliases = listOf("al", "as"),
        grid = listOf(listOf("g"), emptyList(), listOf("h", "i")),
    )

    @Test
    fun decodeMatchesRustEncodedBytes() {
        val (hex, _) = tallyVector()
        val r = assertNotNull(TallyReader.decode(hexToBytes(hex)))
        assertEquals(2, r.usage.size)
        // entries are sorted by enum value: RED (0) before BLUE (2)
        assertEquals(Color.RED, r.usage.key(0))
        assertEquals(1u, r.usage.value(0))
        assertEquals(Color.BLUE, r.usage.key(1))
        assertEquals(3u, r.usage.value(1))
        assertEquals(2, r.labels.size)
        assertEquals("a", r.labels.key(0))
        assertEquals(listOf("x", "y"), r.labels.value(0).toList())
        assertEquals("b", r.labels.key(1))
        assertEquals(0, r.labels.value(1).size)
        assertEquals(listOf("al", "as"), r.aliases.toList())
        assertEquals(3, r.grid.size)
        // nested dynamic array: each element is a slot pointing at an inner string-list region
        val inner = StringListView(r.msg, r.grid[2].offset.toInt(), r.grid[2].length.toInt())
        assertEquals(listOf("h", "i"), inner.toList())
        // map views iterate as entries
        assertEquals(listOf("a" to listOf("x", "y"), "b" to emptyList()), r.labels.map { it.key to it.value.toList() })
    }

    @Test
    fun handWrittenValueEncodesToTheRustBytes() {
        val (hex, _) = tallyVector()
        assertEquals(hex, sampleTally().encode().toHex())
        val owned = assertNotNull(Tally.fromBytes(hexToBytes(hex)))
        assertEquals(sampleTally(), owned)
    }

    @Test
    fun stringKeysSortByUtf8BytesRatherThanUtf16Units() {
        // U+10000 is F0 90 80 80 in UTF-8 but starts with the surrogate D800 in UTF-16, so
        // Kotlin's natural String order would put it first; the canonical order puts U+FFFD first.
        val high = "𐀀"
        val t = Tally(
            usage = emptyMap(),
            labels = linkedMapOf(high to listOf("high"), "�" to listOf("replacement")),
            aliases = emptyList(),
            grid = emptyList(),
        )
        val r = assertNotNull(TallyReader.decode(t.encode()))
        assertEquals(listOf("�", high), r.labels.map { it.key })
    }

    @Test
    fun unknownEnumMapKeysAreReportedWithTheEntryIndex() {
        val (hex, _) = tallyVector()
        val bytes = hexToBytes(hex)
        val region = getU32(bytes, posOf("Tally", "usage")).toInt()
        putU32(bytes, 8 + region, 42u)
        assertEquals(
            ValidationError.Nested("usage", 0, ValidationError.UnknownEnumValue("Color", 42)),
            Tally.validate(bytes),
        )
    }
}

class ValidateTests {
    private val malformed = ValidationError.ExtBlock(3)

    @Test
    fun acceptsCanonicalEncodings() {
        val (hex, _) = userVector()
        val (tallyHex, _) = tallyVector()
        assertNull(User.validate(hexToBytes(hex)))
        assertNull(Tally.validate(hexToBytes(tallyHex)))
        assertNull(Legacy.validate(Legacy(id = 1u, note = "n").encode()))
    }

    @Test
    fun checksTheHeader() {
        val (hex, _) = userVector()
        val data = hexToBytes(hex)
        assertEquals(ValidationError.TooShort(need = 176, got = 10), User.validate(data.copyOf(10)))
        val badKey = data.copyOf()
        badKey[0] = (badKey[0].toInt() xor 0xFF).toByte()
        assertEquals(ValidationError.KeyMismatch(expected = 1u, got = 254u), User.validate(badKey))
        val badLen = data.copyOf()
        badLen[4] = (badLen[4].toInt() xor 0xFF).toByte()
        assertEquals(ValidationError.FixedLenMismatch(expected = 168, got = 87), User.validate(badLen))
    }

    @Test
    fun reportsSlotBoundsUtf8AndEnumValuesWithTheirPath() {
        val (hex, _) = userVector()
        val payload = hex.length / 2 - 8

        val oob = hexToBytes(hex)
        putU32(oob, posOf("User", "name"), UInt.MAX_VALUE)
        val err = User.validate(oob)
        assertEquals(ValidationError.Nested("name", null, ValidationError.SlotOutOfBounds(UInt.MAX_VALUE.toLong(), 6, payload)), err)
        assertEquals("at `name`: slot out of bounds (off=4294967295 len=6, payload=$payload)", err.toString())

        val badUtf8 = hexToBytes(hex)
        val nameOff = getU32(badUtf8, posOf("User", "name")).toInt()
        badUtf8[8 + nameOff] = 0xFF.toByte()
        assertEquals(ValidationError.Nested("name", null, ValidationError.InvalidUtf8), User.validate(badUtf8))

        val badEnum = hexToBytes(hex)
        putU32(badEnum, posOf("User", "color"), 99u)
        assertEquals(ValidationError.Nested("color", null, ValidationError.UnknownEnumValue("Color", 99)), User.validate(badEnum))
    }

    @Test
    fun elementCountOverflowIsReportedAtTheField() {
        // nums claims more elements than the payload can hold: count * stride must not wrap around
        val (hex, _) = userVector()
        val payload = hex.length / 2 - 8
        val nums = field("User", "nums")
        val bytes = hexToBytes(hex)
        val regionOffset = getU32(bytes, 8 + nums.offset).toLong()
        putU32(bytes, 8 + nums.offset + 4, UInt.MAX_VALUE)
        assertEquals(
            ValidationError.Nested("nums", null, ValidationError.SlotOutOfBounds(regionOffset, UInt.MAX_VALUE.toLong(), payload)),
            User.validate(bytes),
        )
    }

    @Test
    fun nestsThePathThroughArraysMapValuesAndNestedArrays() {
        val (hex, _) = userVector()
        val payload = hex.length / 2 - 8

        // byId[0] -> Profile.bio
        val byIdField = field("User", "by_id")
        val byId = hexToBytes(hex)
        val region = getU32(byId, posOf("User", "by_id")).toInt()
        val bioSlot = 8 + region + byIdField.mapValueOffset!! + field("Profile", "bio").offset
        putU32(byId, bioSlot, UInt.MAX_VALUE)
        assertEquals(
            ValidationError.Nested("byId", 0, ValidationError.Nested("bio", null, ValidationError.SlotOutOfBounds(UInt.MAX_VALUE.toLong(), 6, payload))),
            User.validate(byId),
        )

        // tags[1]
        val tagsField = field("User", "tags")
        val tags = hexToBytes(hex)
        val tagsRegion = getU32(tags, posOf("User", "tags")).toInt()
        putU32(tags, 8 + tagsRegion + tagsField.elementSize!!, UInt.MAX_VALUE)
        assertEquals(
            ValidationError.Nested("tags", 1, ValidationError.SlotOutOfBounds(UInt.MAX_VALUE.toLong(), 1, payload)),
            User.validate(tags),
        )

        // grid[1] inside the nested dynamic array
        val (tallyHex, _) = tallyVector()
        val grid = hexToBytes(tallyHex)
        val gridRegion = getU32(grid, posOf("Tally", "grid")).toInt()
        val inner = getU32(grid, 8 + gridRegion + 2 * 8).toInt()
        putU32(grid, 8 + inner + 1 * 8, UInt.MAX_VALUE)
        assertEquals(
            ValidationError.Nested("grid", 1, ValidationError.SlotOutOfBounds(UInt.MAX_VALUE.toLong(), 1, tallyHex.length / 2 - 8)),
            Tally.validate(grid),
        )
    }

    @Test
    fun checksTheExtensionChainFromTheTail() {
        val (hex, _) = userVector()

        val badSince = hexToBytes(hex)
        badSince[badSince.size - 6] = 9
        assertEquals(malformed, User.validate(badSince))

        val tooShort = hexToBytes(hex)
        putU32(tooShort, tooShort.size - 4, 7u)
        assertEquals(malformed, User.validate(tooShort))
        // the lazy reader degrades instead of throwing
        assertNull(UserReader.decode(tooShort)?.vip)

        // header promises v3 but only v2 was written
        val data = hexToBytes(hex)
        val m = assertNotNull(RawMessage.decode(data, 1u, 168))
        val v3Start = assertNotNull(m.findExtBlock(intArrayOf(2, 3), 3, 3, 16))
        assertEquals(malformed, User.validate(data.copyOf(8 + v3Start)))
    }

    @Test
    fun findExtBlockStepsOverNewerBlocksAnOlderReaderDoesNotKnow() {
        val (hex, _) = userVector()
        val m = assertNotNull(RawMessage.decode(hexToBytes(hex), 1u, 168))
        // an older reader (knows v2 only) reading a v3 buffer still finds its block behind v3
        assertNotNull(m.findExtBlock(intArrayOf(2), 3, 2, 20))
        // a block newer than the header claims is inconsistent
        assertNull(m.findExtBlock(intArrayOf(2), 2, 2, 20))
    }

    @Test
    fun ignoresReservedFields() {
        val legacy = Legacy(id = 1u, note = "n")
        val bytes = legacy.encode()
        assertNull(Legacy.validate(bytes))
        assertEquals(legacy, Legacy.fromBytes(bytes))
        // the Rust writer encodes the reserved string slot; a Kotlin writer leaves it zeroed
        putU32(bytes, posOf("Legacy", "old_name"), UInt.MAX_VALUE)
        assertNull(Legacy.validate(bytes))
    }
}

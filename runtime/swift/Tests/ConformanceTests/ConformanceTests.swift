import Testing
import Foundation
import MessageRuntime
import MessageFixtures

// The committed vectors are produced by the Rust fixtures (`cargo run -p message-fixtures --bin
// export-schema -- vectors`); the IR snapshot is only used by the tests to locate fields.
private let repoRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    .deletingLastPathComponent().deletingLastPathComponent()

private func loadJSON(_ name: String) throws -> [String: Any] {
    let data = try Data(contentsOf: repoRoot.appendingPathComponent("vectors/\(name)"))
    return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
}

private func hexToData(_ hex: String) -> Data {
    var bytes = [UInt8](); bytes.reserveCapacity(hex.count / 2)
    var i = hex.startIndex
    while i < hex.endIndex {
        let j = hex.index(i, offsetBy: 2)
        bytes.append(UInt8(hex[i..<j], radix: 16)!)
        i = j
    }
    return Data(bytes)
}

private func dataToHex(_ d: Data) -> String {
    d.map { String(format: "%02x", $0) }.joined()
}

private func putU32(_ bytes: inout [UInt8], _ pos: Int, _ v: UInt32) {
    withUnsafeBytes(of: v.littleEndian) { for (k, b) in $0.enumerated() { bytes[pos + k] = b } }
}

private func getU32(_ bytes: [UInt8], _ pos: Int) -> UInt32 {
    UInt32(bytes[pos]) | UInt32(bytes[pos + 1]) << 8 | UInt32(bytes[pos + 2]) << 16 | UInt32(bytes[pos + 3]) << 24
}

private struct FieldIR { let name: String; let offset: Int; let mapValueOffset: Int?; let elementSize: Int? }
private func field(_ msg: String, _ name: String) throws -> FieldIR {
    let snapshot = try loadJSON("message-snapshot.json")
    let messages = try #require(snapshot["messages"] as? [[String: Any]])
    let m = try #require(messages.first { ($0["name"] as? String) == msg })
    let fields = try #require(m["fields"] as? [[String: Any]])
    let f = try #require(fields.first { ($0["name"] as? String) == name })
    return FieldIR(name: name, offset: f["offset"] as! Int, mapValueOffset: f["mapValueOffset"] as? Int, elementSize: f["elementSize"] as? Int)
}

/// Absolute buffer position of a base field (header included).
private func posOf(_ msg: String, _ name: String) throws -> Int { 8 + (try field(msg, name)).offset }

private func userVector() throws -> (hex: String, expected: [String: Any]) {
    let v = try loadJSON("user.json")
    return (try #require(v["hex"] as? String), try #require(v["expected"] as? [String: Any]))
}

private func tallyVector() throws -> (hex: String, expected: [String: Any]) {
    let v = try loadJSON("tally.json")
    return (try #require(v["hex"] as? String), try #require(v["expected"] as? [String: Any]))
}

/// Mirrors `message_fixtures::sample_user()`.
private func sampleUser() -> User {
    User(
        id: 42, name: "山田", color: .green,
        profile: Profile(age: 30, bio: "hello 世界", nickname: "yamada"),
        matrix: [1.5, -2.25, 3.125, 0.0],
        nums: [Int32.min, -1, 0, 7, Int32.max],
        tags: ["a", "b", "c"],
        blobs: [Data([1, 2, 3]), Data(), Data(repeating: 255, count: 300)],
        edges: [Node(value: 1, children: [Node(value: 11, children: [])]), Node(value: 2, children: [])],
        counts: ["one": 1, "two": 2],
        byId: [7: Profile(age: 25, bio: "nested", nickname: nil)],
        avatar: Data([9, 8, 7]), motto: nil,
        stamp: -1_700_000_000_000, ratio: 3.141592653589793,
        flag: true, small: 200, wide: 65535, offset: -32768, preciseId: UInt64.max,
        email: "user@example.com", score: 99.5, tags2: ["v3"], vip: true
    )
}

@Suite("Rust <-> Swift conformance (full-type User vector)")
struct UserConformance {
    @Test func decodeMatchesRustEncodedBytes() throws {
        let (hex, e) = try userVector()
        let r = try #require(UserReader.decode(hexToData(hex)))
        #expect(r.hv == 3)
        #expect(r.id == (e["id"] as! NSNumber).uint64Value)
        #expect(r.name == (e["name"] as! String))
        #expect(r.color == .green)
        let profile = e["profile"] as! [String: Any]
        #expect(r.profile.age == (profile["age"] as! NSNumber).uint32Value)
        #expect(r.profile.bio == (profile["bio"] as! String))
        #expect(r.profile.nickname == (profile["nickname"] as! String))
        #expect(Array(r.matrix) == [1.5, -2.25, 3.125, 0.0])
        #expect(Array(r.nums) == [Int32.min, -1, 0, 7, Int32.max])
        #expect(Array(r.tags) == ["a", "b", "c"])
        #expect(Array(r.blobs) == [Data([1, 2, 3]), Data(), Data(repeating: 255, count: 300)])
        #expect(r.edges.count == 2)
        #expect(r.edges[0].value == 1)
        #expect(r.edges[0].children[0].value == 11)
        #expect(r.edges[0].children[0].children.count == 0)
        var counts: [String: UInt32] = [:]
        for (k, v) in r.counts { counts[k] = v }
        #expect(counts == ["one": 1, "two": 2])
        #expect(r.byId.count == 1)
        #expect(r.byId.key(at: 0) == 7)
        #expect(r.byId.value(at: 0).bio == "nested")
        #expect(r.avatar == Data([9, 8, 7]))
        #expect(r.motto == nil)
        #expect(r.stamp == -1_700_000_000_000)
        #expect(r.ratio == 3.141592653589793)
        #expect(r.flag)
        #expect(r.small == 200)
        #expect(r.wide == 65535)
        #expect(r.offset == -32768)
        #expect(r.preciseId == UInt64.max)
        // extension-block fields (v2 / v3)
        #expect(r.email == "user@example.com")
        #expect(r.score == 99.5)
        #expect(Array(r.tags2) == ["v3"])
        #expect(r.vip == true)
    }

    @Test func handWrittenValueEncodesToTheRustBytes() throws {
        let (hex, _) = try userVector()
        #expect(dataToHex(sampleUser().encode()) == hex)
    }

    @Test func ownedRoundtripIsByteIdentical() throws {
        let (hex, _) = try userVector()
        let data = hexToData(hex)
        let owned = try #require(User.fromBytes(data))
        #expect(owned == sampleUser())
        #expect(dataToHex(owned.encode()) == hex)
        #expect(dataToHex(User.fromBytes(owned.encode())!.encode()) == hex)
    }

    @Test func lazyDecodeRejectsOnlyShortBuffersAndForeignKeys() throws {
        let (hex, _) = try userVector()
        #expect(UserReader.decode(hexToData(hex).prefix(10)) == nil)
        var foreign = hexToData(hex)
        foreign[0] ^= 0xFF
        #expect(UserReader.decode(foreign) == nil)
    }
}

@Suite("Rust <-> Swift conformance (enum-keyed / array-valued maps, fixed string arrays, nested arrays)")
struct TallyConformance {
    private func sampleTally() -> Tally {
        Tally(usage: [.blue: 3, .red: 1], labels: ["a": ["x", "y"], "b": []], aliases: ["al", "as"], grid: [["g"], [], ["h", "i"]])
    }

    @Test func decodeMatchesRustEncodedBytes() throws {
        let (hex, _) = try tallyVector()
        let r = try #require(TallyReader.decode(hexToData(hex)))
        #expect(r.usage.count == 2)
        // entries are sorted by enum value: red (0) before blue (2)
        #expect(r.usage.key(at: 0) == .red)
        #expect(r.usage.value(at: 0) == 1)
        #expect(r.usage.key(at: 1) == .blue)
        #expect(r.usage.value(at: 1) == 3)
        #expect(r.labels.count == 2)
        #expect(r.labels.key(at: 0) == "a")
        #expect(Array(r.labels.value(at: 0)) == ["x", "y"])
        #expect(r.labels.key(at: 1) == "b")
        #expect(r.labels.value(at: 1).count == 0)
        #expect(Array(r.aliases) == ["al", "as"])
        #expect(r.grid.count == 3)
        // nested dynamic array: each element is a slot pointing at an inner string-list region
        let s = r.grid.slot(at: 2)
        let inner = StringListView(msg: r.msg, start: Int(s.0), count: Int(s.1))
        #expect(Array(inner) == ["h", "i"])
    }

    @Test func handWrittenValueEncodesToTheRustBytes() throws {
        let (hex, _) = try tallyVector()
        #expect(dataToHex(sampleTally().encode()) == hex)
        let owned = try #require(Tally.fromBytes(hexToData(hex)))
        #expect(owned == sampleTally())
    }
}

@Suite("validate: strict check of untrusted buffers")
struct ValidateTests {
    private let malformed = "extension block v3 missing or malformed"

    @Test func acceptsCanonicalEncodings() throws {
        let (hex, _) = try userVector()
        let (tallyHex, _) = try tallyVector()
        #expect(User.validate(hexToData(hex)) == nil)
        #expect(Tally.validate(hexToData(tallyHex)) == nil)
        #expect(Legacy.validate(Legacy(id: 1, note: "n").encode()) == nil)
    }

    @Test func checksTheHeader() throws {
        let (hex, _) = try userVector()
        let data = hexToData(hex)
        #expect(User.validate(data.prefix(10)) == .tooShort(need: 176, got: 10))
        var bytes = [UInt8](data); bytes[0] ^= 0xFF
        #expect(User.validate(Data(bytes)) == .keyMismatch(expected: 1, got: 1 ^ 0xFF))
        var bytes2 = [UInt8](data); bytes2[4] ^= 0xFF
        #expect(User.validate(Data(bytes2)) == .fixedLenMismatch(expected: 168, got: 168 ^ 0xFF))
    }

    @Test func reportsSlotBoundsUtf8AndEnumValuesWithTheirPath() throws {
        let (hex, _) = try userVector()
        let payload = hex.count / 2 - 8

        var bytes = [UInt8](hexToData(hex))
        putU32(&bytes, try posOf("User", "name"), .max)
        let oob = User.validate(Data(bytes))
        #expect(oob == .nested(field: "name", index: nil, .slotOutOfBounds(off: Int(UInt32.max), len: 6, payload: payload)))
        #expect(oob?.description == "at `name`: slot out of bounds (off=\(Int(UInt32.max)) len=6, payload=\(payload))")

        var badUtf8 = [UInt8](hexToData(hex))
        let nameSlot = try posOf("User", "name")
        let nameOff = Int(getU32(badUtf8, nameSlot))
        badUtf8[8 + nameOff] = 0xFF
        #expect(User.validate(Data(badUtf8)) == .nested(field: "name", index: nil, .invalidUtf8))

        var badEnum = [UInt8](hexToData(hex))
        putU32(&badEnum, try posOf("User", "color"), 99)
        #expect(User.validate(Data(badEnum)) == .nested(field: "color", index: nil, .unknownEnumValue(type: "Color", value: 99)))
    }

    @Test func nestsThePathThroughArraysMapValuesAndNestedArrays() throws {
        let (hex, _) = try userVector()
        let payload = hex.count / 2 - 8

        // byId[0] -> Profile.bio
        var byId = [UInt8](hexToData(hex))
        let byIdField = try field("User", "by_id")
        let byIdSlot = try posOf("User", "by_id")
        let region = Int(getU32(byId, byIdSlot))
        let bioSlot = 8 + region + byIdField.mapValueOffset! + (try field("Profile", "bio")).offset
        putU32(&byId, bioSlot, .max)
        #expect(User.validate(Data(byId)) == .nested(field: "byId", index: 0, .nested(field: "bio", index: nil, .slotOutOfBounds(off: Int(UInt32.max), len: 6, payload: payload))))

        // tags[1]
        var tags = [UInt8](hexToData(hex))
        let tagsField = try field("User", "tags")
        let tagsSlot = try posOf("User", "tags")
        let tagsRegion = Int(getU32(tags, tagsSlot))
        putU32(&tags, 8 + tagsRegion + tagsField.elementSize!, .max)
        let e = User.validate(Data(tags))
        #expect(e == .nested(field: "tags", index: 1, .slotOutOfBounds(off: Int(UInt32.max), len: 1, payload: payload)))

        // grid[1] inside the nested dynamic array
        let (tallyHex, _) = try tallyVector()
        var grid = [UInt8](hexToData(tallyHex))
        let gridSlot = try posOf("Tally", "grid")
        let gridRegion = Int(getU32(grid, gridSlot))
        let inner = Int(getU32(grid, 8 + gridRegion + 2 * 8))
        putU32(&grid, 8 + inner + 1 * 8, .max)
        #expect(Tally.validate(Data(grid)) == .nested(field: "grid", index: 1, .slotOutOfBounds(off: Int(UInt32.max), len: 1, payload: tallyHex.count / 2 - 8)))
    }

    @Test func checksTheExtensionChainFromTheTail() throws {
        let (hex, _) = try userVector()

        var badSince = [UInt8](hexToData(hex))
        badSince[badSince.count - 6] = 9
        #expect(User.validate(Data(badSince)) == .extBlock(version: 3))

        var tooShort = [UInt8](hexToData(hex))
        putU32(&tooShort, tooShort.count - 4, 7)
        #expect(User.validate(Data(tooShort)) == .extBlock(version: 3))
        // the lazy reader degrades instead of trapping
        #expect(UserReader.decode(Data(tooShort))?.vip == nil)

        // header promises v3 but only v2 was written
        let data = hexToData(hex)
        let m = try #require(RawMessage(decoding: data, key: 1, fixedLen: 168))
        let v3Start = try #require(m.findExtBlock([2, 3], 3, 3, 16))
        #expect(User.validate(data.prefix(8 + v3Start)) == .extBlock(version: 3))
    }

    @Test func findExtBlockStepsOverNewerBlocksAnOlderReaderDoesNotKnow() throws {
        let (hex, _) = try userVector()
        let m = try #require(RawMessage(decoding: hexToData(hex), key: 1, fixedLen: 168))
        // an older reader (knows v2 only) reading a v3 buffer still finds its block behind v3
        #expect(m.findExtBlock([2], 3, 2, 20) != nil)
        // a block newer than the header claims is inconsistent
        #expect(m.findExtBlock([2], 2, 2, 20) == nil)
    }

    @Test func ignoresReservedFields() throws {
        let legacy = Legacy(id: 1, note: "n")
        var bytes = [UInt8](legacy.encode())
        #expect(Legacy.validate(Data(bytes)) == nil)
        #expect(Legacy.fromBytes(Data(bytes)) == legacy)
        putU32(&bytes, try posOf("Legacy", "old_name"), .max)
        #expect(Legacy.validate(Data(bytes)) == nil)
    }
}

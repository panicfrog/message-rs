//! Swift backend emission rules that are independent of the runtime (the Swift package's own
//! tests in runtime/swift check the generated code against the Rust vectors).

use message_core::{
    plan_schema, EnumDescriptor, EnumVariant, FieldDescriptor, MessageDescriptor, SchemaSet,
    SimpleType, Type,
};

fn fixture_set() -> SchemaSet {
    let mut reserved = FieldDescriptor::new("old_flag", SimpleType::Bool.into());
    reserved.reserved = true;
    let mut set = SchemaSet {
        messages: vec![
            MessageDescriptor::new(4, "Legacy", vec![
                FieldDescriptor::new("id", SimpleType::U32.into()),
                reserved,
                FieldDescriptor::new("note", SimpleType::String.into()),
            ]),
            MessageDescriptor::new(9, "M", vec![
                FieldDescriptor::new("by_color", Type::map(Type::enumeration("Color"), SimpleType::U32.into())),
                FieldDescriptor::new("by_id", Type::map(SimpleType::U64.into(), SimpleType::String.into())),
                FieldDescriptor::new("by_name", Type::map(SimpleType::String.into(), SimpleType::Bytes.into())),
                FieldDescriptor::new("grid", Type::vec(Type::vec(SimpleType::String.into()))),
                FieldDescriptor::new("tags", Type::vec(SimpleType::String.into())),
            ]),
        ],
        enums: vec![EnumDescriptor {
            name: "Color".to_string(),
            doc: None,
            variants: vec![
                EnumVariant { name: "Red".to_string(), value: 0, doc: None },
                EnumVariant { name: "HTTPServer".to_string(), value: 7, doc: None },
            ],
        }],
    };
    plan_schema(&mut set).unwrap();
    set
}

#[test]
fn everything_is_generated_and_no_ir_is_shipped() {
    let swift = message_gen::generate_swift(&fixture_set());
    assert!(swift.contains("import Foundation\nimport MessageRuntime"), "{swift}");
    assert!(swift.contains("public struct LegacyReader: MessageReader {"));
    assert!(swift.contains("public struct Legacy: Equatable {"));
    assert!(swift.contains("func __writeLegacyBase(_ w: inout Writer, _ base: Int, _ value: Legacy) {"));
    assert!(swift.contains("func __validateLegacyBase(_ m: RawMessage, _ base: Int, _ hv: UInt8) throws {"));
    assert!(swift.contains("public func encode() -> Data {"));
    assert!(swift.contains("public static func validate(_ data: Data) -> ValidationError? {"));
    assert!(swift.contains("public static func fromBytes(_ data: Data) -> Legacy? {"));
    assert!(!swift.contains("SchemaIr") && !swift.contains("MESSAGE_SCHEMA"));
}

#[test]
fn reserved_fields_are_omitted_and_the_rest_is_laid_out_with_literals() {
    let swift = message_gen::generate_swift(&fixture_set());
    // id @0, reserved bool @4, note slot @8 -> fixed region 16
    assert!(swift.contains("w.writeU32(base + 0, value.id)"), "{swift}");
    assert!(swift.contains("w.writeStringSlot(base + 8, value.note)"));
    assert!(swift.contains("w.writeU16(4, 16)"));
    assert!(swift.contains("try nest(\"note\") {"));
    assert!(!swift.contains("oldFlag") && !swift.contains("old_flag"), "reserved field leaked:\n{swift}");
}

#[test]
fn types_map_to_swift_idioms() {
    let swift = message_gen::generate_swift(&fixture_set());
    // enum variants become lowerCamelCase, acronyms stay readable
    assert!(swift.contains("case red = 0"), "{swift}");
    assert!(swift.contains("case httpServer = 7"), "{swift}");
    assert!(swift.contains("extension Color: MessageScalar {"));
    // maps are dictionaries with the right key/value types; bytes are Data
    assert!(swift.contains("public var byColor: [Color: UInt32]"));
    assert!(swift.contains("public var byId: [UInt64: String]"));
    assert!(swift.contains("public var byName: [String: Data]"));
    assert!(swift.contains("public var grid: [[String]]"));
    // keys sort by wire value: enum rawValue, numeric, UTF-8 bytes for strings
    assert!(swift.contains("o.keys.sorted { $0.rawValue < $1.rawValue }"));
    assert!(swift.contains("o.keys.sorted()"));
    assert!(swift.contains("o.keys.sorted { $0.utf8.lexicographicallyPrecedes($1.utf8) }"));
    // readers: dictionary views, nested dynamic arrays as slot views
    assert!(swift.contains("public var byId: ByIdMapView"));
    assert!(swift.contains("public var grid: SlotListView"));
    assert!(swift.contains("public var byName: ByNameMapView"));
    // toOwned rebuilds dictionaries and nested arrays
    assert!(swift.contains("byColor.reduce(into: [Color: UInt32]()) { $0[$1.0] = $1.1 }"));
    assert!(swift.contains("grid.map { __s0 in return Array(StringListView(msg: msg, start: Int(__s0.0), count: Int(__s0.1))) }"));
    // validators carry the field path: region (labelled) and elements (nest)
    assert!(swift.contains("try checkRegion(m, base + 0, 8, \"byColor\")"), "{swift}");
    assert!(swift.contains("try nest(\"byColor\", i0) {"));
    assert!(swift.contains("try checkEnum(Color.self, m, e)"));
}

fn evolved_set() -> SchemaSet {
    // P evolves (since = 2), Q never does; H holds both as a field, array elements and map values
    let mut set = SchemaSet {
        messages: vec![
            MessageDescriptor::new(21, "P", vec![
                FieldDescriptor::new("a", SimpleType::U32.into()),
                FieldDescriptor::new("b", SimpleType::Bool.into()).optional().since(2),
            ]),
            MessageDescriptor::new(22, "Q", vec![FieldDescriptor::new("x", SimpleType::F32.into())]),
            MessageDescriptor::new(23, "H", vec![
                FieldDescriptor::new("p", Type::structure("P")),
                FieldDescriptor::new("ps", Type::vec(Type::structure("P"))),
                FieldDescriptor::new("qs", Type::vec(Type::structure("Q"))),
                FieldDescriptor::new("by", Type::map(SimpleType::U64.into(), Type::structure("P"))),
            ]),
        ],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    set
}

#[test]
fn struct_values_are_self_contained_instances_with_the_shape_bit() {
    let swift = message_gen::generate_swift(&evolved_set());
    assert!(swift.contains("func __writePInstance(_ w: inout Writer, _ value: P) -> Int {"), "{swift}");
    assert!(swift.contains("  __writePExt(&w, value)"));
    assert!(swift.contains("do { let s = __writePInstance(&w, value.p); w.writeSlot(base + 0, w.rel(s), w.length - s) }"));
    assert!(swift.contains("return PReader(msg: msg, origin: Int(o), end: Int(o) + Int(l), hv: INSTANCE_HV) }()"));
    assert!(swift.contains("w.writeSlot(base + 8, w.rel(r0), Int(UInt32(a0.count) | SHAPE_BIT))"));
    assert!(swift.contains("__writeQBase(&w, r0 + i0 * 4, v0)"));
    assert!(swift.contains("w.writeSlot(base + 24, w.rel(r), Int(UInt32(ks.count) | SHAPE_BIT))"));
    assert!(swift.contains("public var ps: StructEvoListView<PReader>"));
    assert!(swift.contains("func __validatePInst(_ m: RawMessage, _ lo: Int, _ hi: Int) throws {"));
    assert!(swift.contains("if let bs = m.findExtBlock(PReader.EXT_VERSIONS, INSTANCE_HV, 2, 8, lo: lo, hi: hi) { try __validatePV2(m, bs, INSTANCE_HV) }"));
    assert!(swift.contains("let (o0, n0, s0) = try checkRegionShape(m, base + 8, \"ps\")"));
}

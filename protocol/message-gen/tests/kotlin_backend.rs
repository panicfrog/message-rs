//! Kotlin backend emission rules that are independent of the runtime (the Gradle build's own
//! tests in runtime/kotlin check the generated code against the Rust vectors).

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

fn kotlin(set: &SchemaSet) -> String {
    message_gen::generate_kotlin(set, message_gen::RUNTIME_PACKAGE)
}

#[test]
fn everything_is_generated_and_no_ir_is_shipped() {
    let kt = kotlin(&fixture_set());
    assert!(kt.contains("package message.rs\n\nenum class Color"), "{kt}");
    assert!(kt.contains("class LegacyReader(override val msg: RawMessage, override val origin: Int, override val hv: Int) : MessageReader {"));
    assert!(kt.contains("data class Legacy("));
    assert!(kt.contains("private fun __writeLegacyBase(w: Writer, base: Int, value: Legacy) {"));
    assert!(kt.contains("private fun __validateLegacyBase(m: RawMessage, base: Int, hv: Int) {"));
    assert!(kt.contains("fun encode(): ByteArray {"));
    assert!(kt.contains("fun validate(bytes: ByteArray): ValidationError? = runValidation {"));
    assert!(kt.contains("fun fromBytes(bytes: ByteArray): Legacy? = LegacyReader.decode(bytes)?.toOwned()"));
    assert!(kt.contains("fun decode(bytes: ByteArray): LegacyReader? = LegacyReader.decode(bytes)"));
    assert!(!kt.contains("SchemaIr") && !kt.contains("MESSAGE_SCHEMA"));
    // same package as the runtime: nothing to import
    assert!(!kt.contains("import "), "{kt}");
}

#[test]
fn the_package_can_be_moved_with_explicit_runtime_imports() {
    let kt = message_gen::generate_kotlin(&fixture_set(), "com.example.app.schema");
    assert!(kt.contains("package com.example.app.schema\n\nimport message.rs."), "{kt}");
    assert!(kt.contains("import message.rs.ValidationError\n"));
    assert!(kt.contains("import message.rs.runValidation\n"));
    // the runtime symbols the generated code refers to
    assert!(kt.contains("import message.rs.MapEntry\n"));
}

#[test]
fn reserved_fields_are_omitted_and_the_rest_is_laid_out_with_literals() {
    let kt = kotlin(&fixture_set());
    // id @0, reserved bool @4, note slot @8 -> fixed region 16
    assert!(kt.contains("w.writeU32(base + 0, value.id)"), "{kt}");
    assert!(kt.contains("w.writeStringSlot(base + 8, value.note)"));
    assert!(kt.contains("w.writeU16(4, 16u)"));
    assert!(kt.contains("nest(\"note\") { checkStrSlot(m, base + 8) }"));
    assert!(!kt.contains("oldFlag") && !kt.contains("old_flag"), "reserved field leaked:\n{kt}");
}

#[test]
fn types_map_to_kotlin_idioms() {
    let kt = kotlin(&fixture_set());
    // enum variants become UPPER_SNAKE_CASE, acronyms stay readable
    assert!(kt.contains("    RED(0),\n    HTTP_SERVER(7);\n"), "{kt}");
    assert!(kt.contains("companion object : MessageEnumCompanion<Color> {"));
    // maps keep typed keys; bytes are ByteArray; arrays are lists whatever their arity
    assert!(kt.contains("val byColor: Map<Color, UInt>,"));
    assert!(kt.contains("val byId: Map<ULong, String>,"));
    assert!(kt.contains("val byName: Map<String, ByteArray>,"));
    assert!(kt.contains("val grid: List<List<String>>,"));
    // keys sort by wire value: enum value, numeric order, UTF-8 bytes for strings
    assert!(kt.contains("o.keys.sortedWith(UTF8_STRING_ORDER)"));
    assert!(kt.contains("o.keys.sorted()"));
    assert!(kt.contains("o.keys.sortedBy { it.value }"));
    // readers: typed map views, nested dynamic arrays as slot views
    assert!(kt.contains("val byId: ByIdMapView"));
    assert!(kt.contains("val grid: SlotListView"));
    assert!(kt.contains("val byName: ByNameMapView"));
    assert!(kt.contains("msg.viewAt(origin + 8) { o, n -> ByIdMapView(msg, o, n) }"));
    // toOwned rebuilds maps and nested arrays
    assert!(kt.contains("byColor = byColor.associate { it.key to it.value }"));
    assert!(kt.contains("grid = grid.map { s0 -> StringListView(msg, s0.offset.toInt(), s0.length.toInt()).toList() }"));
    // validators carry the field path: region (labelled) and elements (nest)
    assert!(kt.contains("val (o0, n0) = checkRegion(m, base + 0, 8, \"byColor\")"), "{kt}");
    assert!(kt.contains("nest(\"byColor\", i0) { checkEnum(Color, m, e) }"));
    assert!(kt.contains("nest(\"byName\", i0) { checkBytesSlot(m, e + 8) }"));
    assert!(kt.contains("checkRegion(m, base + 32, 8, \"tags\")\n"));
}

#[test]
fn field_names_become_lower_camel_case_and_only_keywords_are_escaped() {
    let mut set = SchemaSet {
        messages: vec![MessageDescriptor::new(
            7,
            "P",
            vec![
                FieldDescriptor::new("value", SimpleType::U8.into()),
                FieldDescriptor::new("data", SimpleType::String.into()),
                FieldDescriptor::new("out", SimpleType::Bytes.into()),
                FieldDescriptor::new("object", SimpleType::U32.into()),
                FieldDescriptor::new("class", SimpleType::I64.into()),
                FieldDescriptor::new("by_id", SimpleType::U16.into()),
            ],
        )],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    let kt = kotlin(&set);
    // soft keywords are fine as property names; hard keywords get backticks
    assert!(kt.contains("val value: UByte get() = msg.readU8(origin + 0)"), "{kt}");
    assert!(kt.contains("val data: String"));
    assert!(kt.contains("val out: ByteArray"));
    assert!(kt.contains("val `object`: UInt"));
    assert!(kt.contains("val `class`: Long"));
    assert!(kt.contains("val byId: UShort"));
    assert!(kt.contains("value = value,"));
}

#[test]
fn extension_fields_degrade_and_bytes_compare_by_content() {
    let note = FieldDescriptor::new("note", SimpleType::String.into()).optional().since(2);
    let count = FieldDescriptor::new("count", SimpleType::U32.into()).since(2);
    let mut set = SchemaSet {
        messages: vec![MessageDescriptor::new(
            11,
            "E",
            vec![
                FieldDescriptor::new("blob", SimpleType::Bytes.into()),
                FieldDescriptor::new("blobs", Type::vec(SimpleType::Bytes.into())),
                FieldDescriptor::new("flags", Type::vec(SimpleType::Bool.into())),
                note,
                count,
            ],
        )],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    let kt = kotlin(&set);
    // v2 block: bitmap 4 | note slot @4 | count u32 @12 -> fixed region 16
    assert!(kt.contains("val EXT_VERSIONS = intArrayOf(2)"), "{kt}");
    assert!(kt.contains("val bs = msg.findExtBlock(EXT_VERSIONS, hv, 2, 16) ?: return null"));
    assert!(kt.contains("val bs = msg.findExtBlock(EXT_VERSIONS, hv, 2, 16) ?: return 0u"));
    assert!(kt.contains("val blk = w.reserve(16)"));
    assert!(kt.contains("w.writeU8(6, 2u)"));
    assert!(kt.contains("__validateEV2(m, checkExtBlock(m, EReader.EXT_VERSIONS, m.version, 2, 16), m.version)"));
    // bytes inside the type compare by content; plain fields keep data class equality
    assert!(kt.contains("contentEquals(blob, other.blob) &&"));
    assert!(kt.contains("contentEquals(blobs, other.blobs) &&"));
    assert!(kt.contains("flags == other.flags"));
    assert!(kt.contains("var h = contentHashCode(blob)"));
    assert!(kt.contains("h = 31 * h + contentHashCode(blobs)"));
    // scalar elements need the region check only, bool included
    assert!(kt.contains("checkRegion(m, base + 16, 1, \"flags\")"));
}

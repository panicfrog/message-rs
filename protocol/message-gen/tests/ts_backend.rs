//! TypeScript backend emission rules that are independent of the runtime.

use message_core::{
    plan_schema, EnumDescriptor, EnumVariant, FieldDescriptor, MessageDescriptor, SchemaSet,
    SimpleType, Type,
};

fn reserved_set() -> SchemaSet {
    let mut old_flag = FieldDescriptor::new("old_flag", SimpleType::Bool.into());
    old_flag.reserved = true;
    let mut set = SchemaSet {
        messages: vec![MessageDescriptor::new(
            4,
            "Legacy",
            vec![
                FieldDescriptor::new("id", SimpleType::U32.into()),
                old_flag,
                FieldDescriptor::new("note", SimpleType::String.into()),
            ],
        )],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    set
}

#[test]
fn reserved_fields_are_omitted_from_the_ts_api() {
    let ts = message_gen::generate_ts(&reserved_set());
    assert!(ts.contains("get id()") && ts.contains("get note()"), "{ts}");
    assert!(ts.contains("  id: number;") && ts.contains("  note: string;"));
    assert!(
        !ts.contains("oldFlag") && !ts.contains("old_flag"),
        "reserved field leaked into the TS API:\n{ts}"
    );
}

#[test]
fn everything_is_generated_and_no_ir_is_shipped() {
    let ts = message_gen::generate_ts(&reserved_set());
    assert!(!ts.contains("__MESSAGE_SCHEMA"));
    assert!(!ts.contains("encodeMessage") && !ts.contains("validateMessage"));
    assert!(ts.contains("function __writeLegacyBase(w: Writer, base: number, value: Legacy): void {"));
    assert!(ts.contains("export function encode(value: Legacy): Uint8Array {"));
    assert!(ts.contains("export function validate(buf: Uint8Array): ValidationError | null {"));
    assert!(ts.contains("function __validateLegacyBase(dv: DataView, base: number, hv: number, path: ValidationPath): void {"));
    assert!(ts.contains("export function decodeBy(buf: Uint8Array) {"));
    assert!(ts.contains("type MessageCodec,\n"));
    assert!(ts.contains("export const codec: MessageCodec<Legacy> = { key: 4, encode, fromBytes };"));
    // literal layout: header + fixed region (id @0, reserved bool @4, note slot @8 -> 16)
    assert!(ts.contains("const w = new Writer(24);"));
    assert!(ts.contains("w.u32(base + 0, value.id);"));
    assert!(ts.contains("w.u16(4, 16);"));
    assert!(ts.contains("vStr(dv, base + 8, path, 'note');"));
}

#[test]
fn map_keys_are_sorted_by_their_wire_value() {
    let mut set = SchemaSet {
        messages: vec![MessageDescriptor::new(
            9,
            "M",
            vec![
                FieldDescriptor::new("by_color", Type::map(Type::enumeration("Color"), SimpleType::U32.into())),
                FieldDescriptor::new("by_id", Type::map(SimpleType::U64.into(), SimpleType::U32.into())),
                FieldDescriptor::new("by_name", Type::map(SimpleType::String.into(), SimpleType::U32.into())),
                FieldDescriptor::new("by_flag", Type::map(SimpleType::Bool.into(), SimpleType::U32.into())),
                FieldDescriptor::new("by_num", Type::map(SimpleType::I32.into(), SimpleType::String.into())),
            ],
        )],
        enums: vec![EnumDescriptor {
            name: "Color".to_string(),
            doc: None,
            variants: vec![
                EnumVariant { name: "Red".to_string(), value: 0, doc: None },
                EnumVariant { name: "Blue".to_string(), value: 2, doc: None },
            ],
        }],
    };
    plan_schema(&mut set).unwrap();
    let ts = message_gen::generate_ts(&set);

    // keys resolve to their wire value, entries sort by it, values are looked up by the original key
    assert!(ts.contains("Number.isNaN(Number(k)) ? enumNum(Color, k) : Number(k), k] as [number, string]"));
    assert!(ts.contains("[BigInt(k), k] as [bigint, string]"));
    assert!(ts.contains("es.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));"));
    assert!(ts.contains("const ks = Object.keys(o).sort();"));
    assert!(ts.contains("k === 'true' ? 1 : 0"));
    assert!(ts.contains("w.i32(e, es[i][0]);"), "{ts}");
    assert!(ts.contains("w.u64(e, es[i][0]);"));
    assert!(ts.contains("w.u8(e, es[i][0]);"));
    assert!(ts.contains("vEnum(Color, 'Color', dv, e, path, 'byColor', i0);"));
    assert!(ts.contains("vStr(dv, e + 4, path, 'byNum', i0);"));
    // scalar keys and values need nothing beyond the region check
    assert!(ts.contains("vRegion(dv, base + 24, 8, path, 'byFlag');"));
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
    let ts = message_gen::generate_ts(&evolved_set());
    // every struct gets an instance writer; evolved ones append their extension chain
    assert!(ts.contains("function __writePInstance(w: Writer, value: P): number {"), "{ts}");
    assert!(ts.contains("  __writePExt(w, value);"));
    // a struct field is a slot to an instance
    assert!(ts.contains("{ const s = __writePInstance(w, value.p); w.slot(base + 0, w.rel(s), w.length - s); }"));
    assert!(ts.contains("{ const [o, l] = readSlot(this.dv, this.origin + 0); return new PReader(this.dv, o, o + l, INSTANCE_HV); }"));
    // evolved elements: per-element instance slots + shape bit; unevolved: open footprints
    assert!(ts.contains("w.slot(base + 8, w.rel(r0), (a0.length | SHAPE_BIT) >>> 0);"));
    assert!(ts.contains("__writeQBase(w, r0 + i0 * 4, a0[i0]);"));
    // evolved map values: instance pair (u64 key + slot -> 16) on write, both pairs on read
    assert!(ts.contains("w.slot(base + 24, w.rel(r), (es.length | SHAPE_BIT) >>> 0);"));
    assert!(ts.contains("const e = this.start + i * (this.inst ? 16 : 16);"));
    // validators: instances checked in their extent, chains only when offered
    assert!(ts.contains("function __validatePInst(dv: DataView, lo: number, hi: number, path: ValidationPath): void {"));
    assert!(ts.contains("const bs = findExtBlockIn(dv, PReader.EXT_VERSIONS, INSTANCE_HV, 2, 8, lo, hi);"));
    assert!(ts.contains("const [o0, n0, s0] = vRegionShape(dv, base + 8, path, 'ps');"));
}

//! TypeScript backend emission rules that are independent of the runtime.

use message_core::{plan_schema, FieldDescriptor, MessageDescriptor, SchemaSet, SimpleType};

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
    assert!(ts.contains("get id()"), "{}", ts);
    assert!(ts.contains("get note()"));
    assert!(ts.contains("  id: number;") && ts.contains("  note: string;"));
    assert!(!ts.contains("oldFlag"), "reserved field leaked into the TS API:\n{ts}");
    assert!(ts.contains("\"reserved\": true"), "the IR must still carry the flag (encode skips it)");
}

use message_core::{
    plan_schema, FieldDescriptor, MessageDescriptor, SchemaSet, SimpleType, Type,
};
use message_gen::check_evolution;

fn base_set() -> SchemaSet {
    let mut set = SchemaSet {
        messages: vec![MessageDescriptor::new(
            1,
            "User",
            vec![
                FieldDescriptor::new("id", SimpleType::U64.into()),
                FieldDescriptor::new("name", SimpleType::String.into()),
                FieldDescriptor::new("email", SimpleType::String.into()).optional().since(2),
            ],
        )],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    set
}

#[test]
fn identical_sets_pass() {
    assert!(check_evolution(&base_set(), &base_set()).is_empty());
}

#[test]
fn appending_field_with_larger_since_passes() {
    let mut set = base_set();
    let mut grown = base_set();
    grown.messages[0].fields.push(
        FieldDescriptor::new("score", SimpleType::F64.into()).since(3),
    );
    plan_schema(&mut grown).unwrap();
    let _ = &mut set;
    assert!(check_evolution(&set, &grown).is_empty(), "legal appends must pass");
}

#[test]
fn appending_base_field_is_rejected() {
    let old = base_set();
    let mut grown = base_set();
    grown.messages[0]
        .fields
        .insert(1, FieldDescriptor::new("sneaky", SimpleType::U8.into()));
    plan_schema(&mut grown).unwrap();
    let errs = check_evolution(&old, &grown);
    assert!(!errs.is_empty(), "inserting a base field must be rejected: {errs:?}");
}

#[test]
fn new_field_with_small_since_is_rejected() {
    let old = base_set();
    let mut grown = base_set();
    grown.messages[0]
        .fields
        .push(FieldDescriptor::new("late", SimpleType::U8.into()).since(2));
    plan_schema(&mut grown).unwrap();
    let errs = check_evolution(&old, &grown);
    assert!(
        errs.iter().any(|e| e.contains("since")),
        "a new field with since=2 does not exceed the existing max version 2 and must be rejected: {errs:?}"
    );
}

#[test]
fn type_change_is_rejected() {
    let old = base_set();
    let mut changed = base_set();
    changed.messages[0].fields[0].ty = Type::Simple(SimpleType::I64);
    plan_schema(&mut changed).unwrap();
    let errs = check_evolution(&old, &changed);
    assert!(errs.iter().any(|e| e.contains("field definition changed")), "{errs:?}");
}

#[test]
fn deletion_is_rejected() {
    let old = base_set();
    let mut shrunk = base_set();
    shrunk.messages[0].fields.truncate(2);
    plan_schema(&mut shrunk).unwrap();
    let errs = check_evolution(&old, &shrunk);
    assert!(errs.iter().any(|e| e.contains("field count shrank")), "{errs:?}");
}

#[test]
fn precise_flip_is_rejected() {
    let old = base_set();
    let mut flipped = base_set();
    flipped.messages[0].fields[0].precise = true;
    plan_schema(&mut flipped).unwrap();
    let errs = check_evolution(&old, &flipped);
    assert!(
        errs.iter().any(|e| e.contains("field definition changed")),
        "flipping precise changes number <-> bigint on the TS side and must be rejected: {errs:?}"
    );
    let back = check_evolution(&flipped, &old);
    assert!(!back.is_empty(), "flipping precise back must be rejected too: {back:?}");
}

#[test]
fn marking_a_field_reserved_passes() {
    let old = base_set();
    let mut reserved = base_set();
    reserved.messages[0].fields[1].reserved = true;
    plan_schema(&mut reserved).unwrap();
    assert!(
        check_evolution(&old, &reserved).is_empty(),
        "reserving an existing field is the sanctioned deletion and must pass"
    );
}

#[test]
fn un_reserving_a_field_is_rejected() {
    let mut reserved = base_set();
    reserved.messages[0].fields[1].reserved = true;
    plan_schema(&mut reserved).unwrap();
    let revived = base_set();
    let errs = check_evolution(&reserved, &revived);
    assert!(errs.iter().any(|e| e.contains("un-reserved")), "{errs:?}");
}

#[test]
fn message_removal_is_rejected() {
    let old = base_set();
    let mut empty = base_set();
    empty.messages.clear();
    let errs = check_evolution(&old, &empty);
    assert!(errs.iter().any(|e| e.contains("was removed")), "{errs:?}");
}

#[test]
fn enum_variant_change_is_rejected() {
    let mut with_enum = base_set();
    with_enum.enums.push(message_core::EnumDescriptor {
        name: "Color".to_string(),
        doc: None,
        variants: vec![message_core::EnumVariant {
            name: "Red".to_string(),
            value: 0,
            doc: None,
        }],
    });
    let mut changed = with_enum.clone();
    changed.enums[0].variants.push(message_core::EnumVariant {
        name: "Blue".to_string(),
        value: 1,
        doc: None,
    });
    let errs = check_evolution(&with_enum, &changed);
    assert!(errs.iter().any(|e| e.contains("variants changed")), "{errs:?}");
}

/// Holder with a struct field, struct array and struct-valued map; `Inner` optionally evolved.
fn nested_set(inner_evolved: bool) -> SchemaSet {
    let mut inner = vec![
        FieldDescriptor::new("a", SimpleType::U32.into()),
        FieldDescriptor::new("s", SimpleType::String.into()),
    ];
    if inner_evolved {
        inner.push(FieldDescriptor::new("b", SimpleType::Bool.into()).optional().since(2));
    }
    let mut set = SchemaSet {
        messages: vec![
            MessageDescriptor::new(1, "Holder", vec![
                FieldDescriptor::new("inner", Type::structure("Inner")),
                FieldDescriptor::new("list", Type::vec(Type::structure("Inner"))),
                FieldDescriptor::new("by_id", Type::map(SimpleType::U64.into(), Type::structure("Inner"))),
            ]),
            MessageDescriptor::new(2, "Inner", inner),
        ],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    set
}

#[test]
fn nested_struct_evolution_passes_and_flips_map_entries() {
    let (old, new) = (nested_set(false), nested_set(true));
    let errs = check_evolution(&old, &new);
    assert!(errs.is_empty(), "evolving a referenced struct must pass: {errs:?}");
    let by_id = |s: &SchemaSet| {
        let f = s.message("Holder").unwrap().fields.iter().find(|f| f.name == "by_id").unwrap().clone();
        (f.element_size, f.map_value_offset)
    };
    assert_eq!(by_id(&old), (Some(24), Some(8)), "open entries: u64 key + 12-byte fixed region");
    assert_eq!(by_id(&new), (Some(16), Some(8)), "instance entries: u64 key + slot");
}

#[test]
fn other_entry_layout_changes_are_rejected() {
    // forge a stride change without the value struct gaining its first extension block
    let old = nested_set(false);
    let mut new = nested_set(false);
    let f = new.messages[0].fields.iter_mut().find(|f| f.name == "by_id").unwrap();
    f.element_size = Some(16);
    let errs = check_evolution(&old, &new);
    assert!(errs.iter().any(|e| e.contains("by_id") && e.contains("elementSize")), "{errs:?}");
}

#[test]
fn array_stride_changes_are_rejected() {
    let old = nested_set(true);
    let mut new = nested_set(true);
    let f = new.messages[0].fields.iter_mut().find(|f| f.name == "list").unwrap();
    f.element_size = Some(8);
    let errs = check_evolution(&old, &new);
    assert!(errs.iter().any(|e| e.contains("list") && e.contains("elementSize")), "{errs:?}");
}

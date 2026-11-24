use message_core::{
    plan_schema, set_to_json_string, EnumDescriptor, EnumVariant, FieldDescriptor, MessageDescriptor,
    SchemaError, SchemaSet, SimpleType, Type, SINCE_BASE,
};

fn msg(key: u32, name: &str, fields: Vec<FieldDescriptor>) -> MessageDescriptor {
    MessageDescriptor::new(key, name, fields)
}

fn f(name: &str, ty: Type) -> FieldDescriptor {
    FieldDescriptor::new(name, ty)
}

fn planned(mut set: SchemaSet) -> SchemaSet {
    plan_schema(&mut set).expect("plan ok");
    set
}

fn field<'a>(set: &'a SchemaSet, msg: &str, name: &str) -> &'a FieldDescriptor {
    set.message(msg)
        .unwrap()
        .fields
        .iter()
        .find(|f| f.name == name)
        .unwrap()
}

#[test]
fn scalar_layout_follows_natural_alignment() {
    let set = planned(SchemaSet {
        messages: vec![msg(
            1,
            "Scalars",
            vec![
                f("a", SimpleType::U8.into()),
                f("b", SimpleType::U32.into()),
                f("c", SimpleType::U16.into()),
                f("d", SimpleType::U64.into()),
            ],
        )],
        enums: vec![],
    });
    let m = set.message("Scalars").unwrap();
    assert_eq!(m.layout.bitmap_len, 0);
    assert_eq!(m.layout.fixed_len, 24);
    assert_eq!(m.layout.max_align, 8);
    let offs: Vec<(usize, usize)> = m.fields.iter().map(|f| (f.offset, f.size)).collect();
    assert_eq!(offs, vec![(0, 1), (4, 4), (8, 2), (16, 8)]);
}

#[test]
fn bitmap_leads_fixed_region_and_is_u32_aligned() {
    let set = planned(SchemaSet {
        messages: vec![msg(
            1,
            "Opts",
            vec![
                f("a", SimpleType::U8.into()).optional(),
                f("b", SimpleType::U64.into()).optional(),
                f("c", SimpleType::U32.into()),
                f("d", SimpleType::Bool.into()).optional(),
            ],
        )],
        enums: vec![],
    });
    let m = set.message("Opts").unwrap();
    assert_eq!(m.layout.bitmap_len, 4);
    let a = field(&set, "Opts", "a");
    let b = field(&set, "Opts", "b");
    let c = field(&set, "Opts", "c");
    let d = field(&set, "Opts", "d");
    assert_eq!((a.offset, a.size, a.optional_bit), (4, 1, Some(0)));
    assert_eq!((b.offset, b.size, b.optional_bit), (8, 8, Some(1)));
    assert_eq!((c.offset, c.size, c.optional_bit), (16, 4, None));
    assert_eq!((d.offset, d.size, d.optional_bit), (20, 1, Some(2)));
    assert_eq!(m.layout.fixed_len, 24);
}

#[test]
fn slots_are_8_bytes_and_4_aligned() {
    let set = planned(SchemaSet {
        messages: vec![msg(
            1,
            "Slots",
            vec![
                f("name", SimpleType::String.into()),
                f("flag", SimpleType::Bool.into()),
                f("blob", SimpleType::Bytes.into()),
                f("list", Type::vec(SimpleType::U16.into())),
            ],
        )],
        enums: vec![],
    });
    let m = set.message("Slots").unwrap();
    let offs: Vec<(usize, usize, bool)> =
        m.fields.iter().map(|f| (f.offset, f.size, f.slot)).collect();
    assert_eq!(offs, vec![(0, 8, true), (8, 1, false), (12, 8, true), (20, 8, true)]);
    assert_eq!(m.layout.fixed_len, 28);
}

#[test]
fn struct_field_is_a_slot_to_a_self_contained_instance() {
    let mut set = SchemaSet {
        messages: vec![
            msg(
                2,
                "Inner",
                vec![
                    f("x", SimpleType::U32.into()).optional(),
                    f("y", SimpleType::U64.into()),
                ],
            ),
            msg(
                1,
                "Outer",
                vec![f("tag", SimpleType::U8.into()), f("inner", Type::structure("Inner"))],
            ),
        ],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    let inner = set.message("Inner").unwrap();
    assert_eq!(inner.layout.fixed_len, 16);
    assert_eq!(inner.layout.max_align, 8);
    // the parent never inlines the child: tag@0, inner slot@4 (align 4) -> 12
    let outer = set.message("Outer").unwrap();
    let inner_f = field(&set, "Outer", "inner");
    assert_eq!((inner_f.offset, inner_f.size, inner_f.slot), (4, 8, true));
    assert_eq!(outer.layout.fixed_len, 12);
    assert_eq!(outer.layout.max_align, 4);
}

#[test]
fn fixed_array_of_scalars_is_inlined_and_fixed_array_of_strings_is_a_slot() {
    let set = planned(SchemaSet {
        messages: vec![msg(
            1,
            "Arrays",
            vec![
                f("m", Type::array(SimpleType::F32.into(), Some(4))),
                f("names", Type::array(SimpleType::String.into(), Some(3))),
                f("u", SimpleType::U8.into()),
            ],
        )],
        enums: vec![],
    });
    let m = set.message("Arrays").unwrap();
    assert_eq!(field(&set, "Arrays", "m").offset, 0);
    assert_array_field(&set, "Arrays", "m", 16, false);
    assert_array_field(&set, "Arrays", "names", 8, true);
    assert_eq!(field(&set, "Arrays", "u").offset, 24);
    // u8 ends at 25; rounded up by max_align=4 -> 28
    assert_eq!(m.layout.fixed_len, 28);
}

fn assert_array_field(set: &SchemaSet, m: &str, name: &str, size: usize, slot: bool) {
    let fd = field(set, m, name);
    assert_eq!(fd.size, size, "{m}.{name} size");
    assert_eq!(fd.slot, slot, "{m}.{name} slot");
}

#[test]
fn fixed_array_of_structs_is_a_slot_with_the_open_stride() {
    let mut set = SchemaSet {
        messages: vec![
            msg(2, "Pt", vec![f("x", SimpleType::F32.into()), f("y", SimpleType::F32.into())]),
            msg(1, "Shape", vec![f("pts", Type::array(Type::structure("Pt"), Some(3)))]),
        ],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    assert_eq!(set.message("Pt").unwrap().layout.fixed_len, 8);
    let pts = field(&set, "Shape", "pts");
    assert_eq!((pts.size, pts.slot, pts.element_size), (8, true, Some(8)));
    assert_eq!(set.message("Shape").unwrap().layout.fixed_len, 8);
}

#[test]
fn versioned_fields_go_to_ext_blocks_and_base_is_frozen() {
    let mut set = SchemaSet {
        messages: vec![
            msg(
                1,
                "User",
                vec![
                    f("id", SimpleType::U64.into()),
                    f("name", SimpleType::String.into()),
                    f("profile", Type::structure("Profile")),
                    f("tags", Type::vec(SimpleType::String.into())).since(2),
                    f("avatar", Type::vec(SimpleType::U8.into())).optional().since(2),
                    f("score", SimpleType::F64.into()).since(3),
                ],
            ),
            msg(3, "Profile", vec![f("bio", SimpleType::String.into())]),
        ],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    let u = set.message("User").unwrap();
    // base: id@0(8) name@8(8) profile@16(8) -> 24
    assert_eq!(u.layout.fixed_len, 24);
    assert_eq!(u.layout.ext_blocks.len(), 2);
    let v2 = &u.layout.ext_blocks[0];
    let v3 = &u.layout.ext_blocks[1];
    assert_eq!((v2.version, v2.bitmap_len, v2.fixed_len), (2, 4, 20));
    let tags = field(&set, "User", "tags");
    let avatar = field(&set, "User", "avatar");
    let score = field(&set, "User", "score");
    assert_eq!(tags.offset, 4);
    assert_eq!(avatar.offset, 12);
    assert_eq!(avatar.optional_bit, Some(0));
    assert_eq!(score.offset, 0);
    assert_eq!((v3.version, v3.bitmap_len, v3.fixed_len), (3, 0, 8));
}

#[test]
fn evolved_structs_can_be_referenced_and_flip_map_entries_to_instance_slots() {
    let inner = |evolved: bool| {
        let mut fields = vec![f("a", SimpleType::U32.into()), f("s", SimpleType::String.into())];
        if evolved {
            fields.push(f("b", SimpleType::U32.into()).since(2));
        }
        msg(2, "Inner", fields)
    };
    let set_of = |evolved: bool| SchemaSet {
        messages: vec![
            msg(
                1,
                "Outer",
                vec![
                    f("inner", Type::structure("Inner")),
                    f("list", Type::vec(Type::structure("Inner"))),
                    f("by_id", Type::map(SimpleType::U64.into(), Type::structure("Inner"))),
                ],
            ),
            inner(evolved),
        ],
        enums: vec![],
    };
    let flat = planned(set_of(false));
    let evolved = planned(set_of(true));
    for set in [&flat, &evolved] {
        // the parent's own layout never depends on the child's evolution
        assert_eq!(field(set, "Outer", "inner").offset, 0);
        assert_eq!(set.message("Outer").unwrap().layout.fixed_len, 24);
        // array stride is the open footprint (Inner's frozen fixed region) either way
        assert_eq!(field(set, "Outer", "list").element_size, Some(12));
    }
    // map entries: open pair (u64 key + 12-byte value -> 24) vs instance pair (key + slot -> 16)
    let open = field(&flat, "Outer", "by_id");
    assert_eq!((open.element_size, open.map_value_offset), (Some(24), Some(8)));
    let inst = field(&evolved, "Outer", "by_id");
    assert_eq!((inst.element_size, inst.map_value_offset), (Some(16), Some(8)));
    assert_eq!(evolved.message("Inner").unwrap().layout.ext_blocks.len(), 1);
}

#[test]
fn reference_cycles_between_structs_are_legal() {
    let mut set = SchemaSet {
        messages: vec![
            msg(1, "A", vec![f("b", Type::structure("B"))]),
            msg(2, "B", vec![f("a", Type::structure("A")), f("n", SimpleType::U8.into())]),
        ],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    assert_eq!(set.message("A").unwrap().layout.fixed_len, 8);
    assert_eq!(set.message("B").unwrap().layout.fixed_len, 12);
}

#[test]
fn recursion_through_dynamic_array_is_allowed() {
    let mut set = SchemaSet {
        messages: vec![msg(
            1,
            "Node",
            vec![
                f("value", SimpleType::U32.into()),
                f("children", Type::vec(Type::structure("Node"))),
            ],
        )],
        enums: vec![],
    };
    plan_schema(&mut set).unwrap();
    let n = set.message("Node").unwrap();
    // value@0(4) children@4(8) -> 12
    assert_eq!(n.layout.fixed_len, 12);
    assert_eq!(field(&set, "Node", "children").element_size, Some(12));
}

#[test]
fn map_entry_stride_and_slot() {
    let mut set = SchemaSet {
        messages: vec![msg(
            1,
            "M",
            vec![
                f("counts", Type::map(SimpleType::String.into(), SimpleType::U32.into())),
                f("by_id", Type::map(SimpleType::U64.into(), Type::structure("V"))),
            ],
        )],
        enums: vec![],
    };
    set.messages.push(msg(
        2,
        "V",
        vec![f("s", SimpleType::String.into()), f("n", SimpleType::U8.into())],
    ));
    plan_schema(&mut set).unwrap();
    // string key: slot(8) @0; u32 value @8 -> 12, align 4 -> 12
    assert_eq!(field(&set, "M", "counts").element_size, Some(12));
    // V fixed_len = 12 (slot8 + u8@8 -> 9 -> round 4)
    assert_eq!(set.message("V").unwrap().layout.fixed_len, 12);
    // unevolved V is written open: u64 key @0(8), V fixed region @8(12), align 8 -> 24
    assert_eq!(field(&set, "M", "by_id").element_size, Some(24));
    assert!(field(&set, "M", "counts").slot);
    let l = message_core::map_entry_layouts(&SimpleType::U64.into(), &Type::structure("V"), &set).unwrap();
    assert_eq!((l.open_value_offset, l.open_stride), (8, 24));
    assert_eq!((l.inst_value_offset, l.inst_stride), (8, 16));
}

#[test]
fn bounded_ext_walk_never_leaves_the_instance() {
    use message_core::{find_ext_block_bounded, INSTANCE_HV};
    // [junk 4][instance: fixed 4 | v2 block: 4 bytes + trailer(2, 10)]
    let mut buf = vec![0xAAu8; 4];
    buf.extend_from_slice(&[1, 2, 3, 4]);
    buf.extend_from_slice(&[9, 9, 9, 9, 2, 0, 10, 0, 0, 0]);
    let (lo, hi) = (4, buf.len());
    assert_eq!(find_ext_block_bounded(&buf, &[2], INSTANCE_HV, 2, 4, lo, hi), Some((8, 12)));
    // a chain whose block_len claims to reach below the instance start is absent
    buf[hi - 4] = 15;
    assert_eq!(find_ext_block_bounded(&buf, &[2], INSTANCE_HV, 2, 0, lo, hi), None);
    // instances without a chain degrade
    assert_eq!(find_ext_block_bounded(&buf, &[2], INSTANCE_HV, 2, 0, 4, 8), None);
}

#[test]
fn shape_bit_and_count_share_the_slot_length() {
    assert_eq!(message_core::SHAPE_BIT | 3, 0x8000_0003);
    assert_eq!(0x8000_0003u32 & message_core::LEN_MASK, 3);
}

#[test]
fn json_ir_is_stable_and_human_readable() {
    let mut set = SchemaSet {
        messages: vec![msg(
            1,
            "Point",
            vec![
                f("x", SimpleType::F64.into()).doc("horizontal"),
                f("y", SimpleType::F64.into()),
            ],
        )],
        enums: vec![EnumDescriptor {
            name: "Color".to_string(),
            doc: None,
            variants: vec![
                EnumVariant { name: "Red".to_string(), value: 0, doc: None },
                EnumVariant { name: "Green".to_string(), value: 1, doc: None },
            ],
        }],
    };
    plan_schema(&mut set).unwrap();
    let json = set_to_json_string(&set);
    let expected = r#"{
  "messages": [
    {
      "key": 1,
      "name": "Point",
      "fields": [
        {
          "name": "x",
          "type": "f64",
          "since": 1,
          "doc": "horizontal",
          "offset": 0,
          "size": 8
        },
        {
          "name": "y",
          "type": "f64",
          "since": 1,
          "offset": 8,
          "size": 8
        }
      ],
      "layout": {
        "bitmapLen": 0,
        "fixedLen": 16,
        "maxAlign": 8
      }
    }
  ],
  "enums": [
    {
      "name": "Color",
      "variants": [
        {
          "name": "Red",
          "value": 0
        },
        {
          "name": "Green",
          "value": 1
        }
      ]
    }
  ]
}"#;
    assert_eq!(json, expected);
    let back: SchemaSet = serde_json::from_str(&json).unwrap();
    assert_eq!(back, set);
}

#[test]
fn duplicate_key_and_zero_key_are_rejected() {
    let mut dup = SchemaSet {
        messages: vec![msg(7, "A", vec![]), msg(7, "B", vec![])],
        enums: vec![],
    };
    assert!(matches!(plan_schema(&mut dup), Err(SchemaError::DuplicateKey(7))));
    let mut zero = SchemaSet { messages: vec![msg(0, "Z", vec![])], enums: vec![] };
    assert!(matches!(plan_schema(&mut zero), Err(SchemaError::ZeroKey)));
}

#[test]
fn unknown_type_is_rejected() {
    let mut set = SchemaSet {
        messages: vec![msg(1, "A", vec![f("x", Type::structure("Missing"))])],
        enums: vec![],
    };
    assert!(matches!(plan_schema(&mut set), Err(SchemaError::UnknownType(n)) if n == "Missing"));
}

#[test]
fn invalid_since_and_map_key_are_rejected() {
    let mut bad_since = SchemaSet {
        messages: vec![msg(1, "A", vec![f("x", SimpleType::U8.into()).since(0)])],
        enums: vec![],
    };
    assert!(matches!(
        plan_schema(&mut bad_since),
        Err(SchemaError::InvalidSince(n, SINCE_BASE)) if n == "x"
    ));
    let mut bad_key = SchemaSet {
        messages: vec![msg(
            1,
            "M",
            vec![f("m", Type::map(Type::vec(SimpleType::U8.into()), SimpleType::U32.into()))],
        )],
        enums: vec![],
    };
    assert!(matches!(plan_schema(&mut bad_key), Err(SchemaError::InvalidMapKey(_))));
}

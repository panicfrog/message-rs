//! Enum-keyed maps and array-valued maps: encode order is canonical (sorted) and the views read
//! both sides back; the planner must agree with the derive-computed entry stride.

use message_core::StructEvoListView;
use message_fixtures::{sample_tally, Color, Profile, Tally};

#[test]
fn enum_keyed_and_array_valued_maps_roundtrip() {
    let t = sample_tally();
    let buf = t.encode();
    let r = Tally::decode(&buf).unwrap();

    assert_eq!(r.usage().len(), 2);
    assert_eq!(r.usage().key(0), Color::Red, "entries are sorted by enum value");
    assert_eq!(r.usage().value(0), 1);
    assert_eq!(r.usage().key(1), Color::Blue);
    assert_eq!(r.usage().value(1), 3);

    assert_eq!(r.labels().len(), 2);
    assert_eq!(r.labels().key(0), "a");
    assert_eq!(r.labels().value(0).len(), 2);
    assert_eq!(r.labels().value(0).get(1), "y");
    assert_eq!(r.labels().key(1), "b");
    assert_eq!(r.labels().value(1).len(), 0);

    assert_eq!(r.aliases().len(), 2);
    assert_eq!(r.aliases().get(1), "as");
    assert_eq!(r.grid().len(), 3);
    let inner = |i: usize| {
        let (o, l) = r.grid().slot_of(i);
        message_core::StringListView::new(r.payload(), o as usize, l as usize)
    };
    assert_eq!(inner(0).len(), 1);
    assert_eq!(inner(0).get(0), "g");
    assert_eq!(inner(1).len(), 0);
    assert_eq!(inner(2).get(1), "i");

    // nested dynamic array of an evolved struct: the inner region slot carries the shape bit, so
    // the inner region is per-element instance slots (regression: the derive once wrote it open;
    // the outer slot is a plain slot list and never carries the bit)
    assert_eq!(r.groups().len(), 3);
    let group = |i: usize| {
        let (o, l) = r.groups().slot_of(i);
        assert!(l & message_core::SHAPE_BIT != 0, "inner region slot must carry the shape bit");
        StructEvoListView::<Profile>::new(r.payload(), o as usize, l)
    };
    assert!(group(0).is_instance_mode());
    assert_eq!(group(0).len(), 2);
    assert_eq!(group(0).get(1).age(), 2);
    assert_eq!(group(0).get(1).verified(), Some(true));
    assert_eq!(group(0).get(0).verified(), Some(false));
    assert_eq!(group(1).len(), 0);
    assert_eq!(group(2).get(0).bio(), "bio3");
    assert_eq!(Tally::validate(&buf), Ok(()));

    assert_eq!(r.to_owned(), t);
    assert_eq!(r.to_owned().encode(), buf, "canonical re-encode");
}

#[test]
fn tally_layout_is_consistent_across_derive_and_planner() {
    let set = message_core::collect_schema().unwrap();
    let usage = set
        .message("Tally")
        .unwrap()
        .fields
        .iter()
        .find(|f| f.name == "usage")
        .unwrap();
    // Color key (i32) @0, u32 value @4 -> stride 8
    assert_eq!((usage.element_size, usage.map_value_offset), (Some(8), Some(4)));
    assert!(matches!(usage.ty, message_core::Type::Map { .. }));
}

#[test]
fn evolved_struct_values_use_the_instance_entry_layout() {
    let set = message_core::collect_schema().unwrap();
    let by_id = set.message("User").unwrap().fields.iter().find(|f| f.name == "by_id").unwrap();
    // Profile has a v2 block, so today's writers emit u64 key @0 + value slot @8 -> stride 16
    assert_eq!((by_id.element_size, by_id.map_value_offset), (Some(16), Some(8)));
    let l = message_core::map_entry_layouts(
        &message_core::SimpleType::U64.into(),
        &message_core::Type::structure("Profile"),
        &set,
    )
    .unwrap();
    // the open pair is still what older (pre-v2 Profile) writers produced: 8 + 24-byte fixed region
    assert_eq!((l.open_value_offset, l.open_stride), (8, 32));
    assert_eq!((l.inst_value_offset, l.inst_stride), (8, 16));
}

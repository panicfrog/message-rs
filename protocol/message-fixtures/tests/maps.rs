//! Enum-keyed maps and array-valued maps: encode order is canonical (sorted) and the views read
//! both sides back; the planner must agree with the derive-computed entry stride.

use message_fixtures::{sample_tally, Color, Tally};

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

//! `#[msg(reserved)]`: the field keeps its slot in the layout and is still encoded from the
//! struct, but the Reader exposes nothing for it and `to_owned()` yields `Default::default()`.

use message_core::{read_slot, slot_slice, Message, HEADER_LEN};
use message_fixtures::Legacy;

fn sample() -> Legacy {
    Legacy { id: 7, old_flag: true, old_name: "deprecated".to_string(), note: "kept".to_string() }
}

#[test]
fn reserved_fields_are_laid_out_and_encoded() {
    let buf = sample().encode();
    let d = Legacy::__descriptor();
    let flag = d.fields.iter().find(|f| f.name == "old_flag").unwrap();
    assert!(flag.reserved);
    assert_eq!(buf[HEADER_LEN + flag.offset], 1, "the struct's value still lands in the fixed region");
    let name = d.fields.iter().find(|f| f.name == "old_name").unwrap();
    assert!(name.reserved && name.slot);
    let payload = &buf[HEADER_LEN..];
    let (o, l) = read_slot(payload, name.offset);
    assert_eq!(std::str::from_utf8(slot_slice(payload, o, l)).unwrap(), "deprecated");
}

#[test]
fn reserved_fields_have_no_accessor_and_default_in_to_owned() {
    let buf = sample().encode();
    let r = Legacy::decode(&buf).unwrap();
    assert_eq!(r.id(), 7);
    assert_eq!(r.note(), "kept");
    let owned = r.to_owned();
    assert_eq!(owned.old_flag, false);
    assert_eq!(owned.old_name, "");
    assert_eq!(owned.id, 7);
    assert_eq!(owned.note, "kept");
    let zeroed = Legacy { old_flag: false, old_name: String::new(), ..sample() };
    assert_eq!(owned.encode(), zeroed.encode(), "re-encoding writes the defaults into the reserved slots");
}

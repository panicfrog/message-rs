use message_fixtures::{sample_user, Color, User};

#[test]
fn roundtrip_all_types() {
    let u = sample_user();
    let buf = u.encode();
    let r = User::decode(&buf).expect("decode failed");

    // zero-copy per-field reads
    assert_eq!(r.id(), 42);
    assert_eq!(r.name(), "山田");
    assert_eq!(r.color(), Color::Green);
    assert_eq!(r.profile().age(), 30);
    assert_eq!(r.profile().bio(), "hello 世界");
    assert_eq!(r.profile().nickname(), Some("yamada"));
    assert_eq!(r.matrix().get(2), 3.125);
    assert_eq!(r.matrix().len(), 4);
    assert_eq!(r.nums().get(3), 7);
    assert_eq!(r.nums().get(0), i32::MIN);
    assert_eq!(r.nums().len(), 5);
    assert_eq!(r.tags().get(1), "b");
    assert_eq!(r.blobs().get(0), &[1u8, 2u8, 3u8][..]);
    assert_eq!(r.blobs().get(1), &[] as &[u8]);
    assert_eq!(r.blobs().get(2).len(), 300);
    assert_eq!(r.edges().len(), 2);
    assert_eq!(r.edges().get(0).value(), 1);
    assert_eq!(r.edges().get(0).children().get(0).value(), 11);
    assert_eq!(r.counts().len(), 2);
    let mut pairs: Vec<(String, u32)> = (0..r.counts().len())
        .map(|i| (r.counts().key(i).to_string(), r.counts().value(i)))
        .collect();
    pairs.sort();
    assert_eq!(pairs, vec![("one".to_string(), 1), ("two".to_string(), 2)]);
    assert_eq!(r.by_id().len(), 1);
    assert_eq!(r.by_id().key(0), 7);
    assert_eq!(r.by_id().value(0).age(), 25);
    assert_eq!(r.by_id().value(0).bio(), "nested");
    assert_eq!(r.avatar(), Some(&[9u8, 8, 7][..]));
    assert_eq!(r.motto(), None);
    assert_eq!(r.stamp(), -1700000000000);
    assert_eq!(r.ratio(), 3.141592653589793);
    assert!(r.flag());
    assert_eq!(r.small(), 200);
    assert_eq!(r.wide(), 65535);
    assert_eq!(r.offset(), -32768);
    assert_eq!(r.precise_id(), u64::MAX);
    // extension-block fields
    assert_eq!(r.email(), Some("user@example.com"));
    assert_eq!(r.score(), 99.5);
    assert_eq!(r.tags2().get(0), "v3");
    assert_eq!(r.vip(), Some(true));

    // to_owned restores everything
    assert_eq!(r.to_owned(), u);
}

#[test]
fn encode_is_canonical_and_deterministic() {
    let u = sample_user();
    assert_eq!(u.encode(), u.encode(), "encoding the same value twice must be byte-identical");
    let first = u.encode();
    let r = User::decode(&first).unwrap();
    assert_eq!(
        r.to_owned().encode(),
        first,
        "re-encoding after a roundtrip must be byte-identical to the original"
    );
}

#[test]
fn derive_layout_matches_planner() {
    message_core::check_consistency().expect("derive/planner layout drift");
}

#[test]
fn registry_collects_all_types() {
    let set = message_core::collect_schema().unwrap();
    let names: Vec<&str> = set.messages.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"User"));
    assert!(names.contains(&"Profile"));
    assert!(names.contains(&"Node"));
    assert!(set.enums.iter().any(|e| e.name == "Color"));
    let user = set.message("User").unwrap();
    assert_eq!(user.key, 1);
    // the color field should be relabeled as an enum
    let color = user.fields.iter().find(|f| f.name == "color").unwrap();
    assert!(matches!(color.ty, message_core::Type::Enum { .. }));
}

#[test]
fn json_ir_dump_is_readable() {
    let set = message_core::collect_schema().unwrap();
    let json = message_core::set_to_json_string(&set);
    assert!(json.contains("\"name\": \"User\""));
    assert!(json.contains("\"fixedLen\""));
}

//! Cross-version compatibility: the base is frozen forever and extension blocks are appended
//! physically. UserV1 keeps exactly User's base fields (names/order/types) and key, simulating
//! the pre-evolution schema. Both directions must read:
//! - old reader, new buffer: trailing extension blocks are ignored, base fields stay correct;
//! - new reader, old buffer: the extension chain is empty, since-fields degrade to defaults.

use message_derive::Message;
use message_fixtures::{sample_user, Node, Profile, User};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0001)]
pub struct UserV1 {
    pub id: u64,
    pub name: String,
    pub color: message_fixtures::Color,
    pub profile: Profile,
    pub matrix: [f32; 4],
    pub nums: Vec<i32>,
    pub tags: Vec<String>,
    pub blobs: Vec<Vec<u8>>,
    pub edges: Vec<Node>,
    pub counts: HashMap<String, u32>,
    pub by_id: HashMap<u64, Profile>,
    pub avatar: Option<Vec<u8>>,
    pub motto: Option<String>,
    pub stamp: i64,
    pub ratio: f64,
    pub flag: bool,
    pub small: u8,
    pub wide: u16,
    pub offset: i16,
    pub precise_id: u64,
}

#[test]
fn old_reader_reads_new_buffer() {
    let u = sample_user();
    let buf = u.encode();
    let old = UserV1::decode(&buf).expect("the old reader must decode new data (ignoring extension blocks)");
    assert_eq!(old.id(), 42);
    assert_eq!(old.name(), "山田");
    assert_eq!(old.nums().len(), 5);
    assert_eq!(old.tags().get(1), "b");
    assert_eq!(old.blobs().get(0), &[1u8, 2, 3][..]);
    assert_eq!(old.edges().get(0).children().get(0).value(), 11);
    assert_eq!(old.by_id().value(0).bio(), "nested");
    assert_eq!(old.avatar(), Some(&[9u8, 8, 7][..]));
    assert_eq!(old.precise_id(), u64::MAX);
    // base fully restored (extension-block data dropped)
    let expected = UserV1 {
        id: u.id,
        name: u.name.clone(),
        color: u.color,
        profile: u.profile.clone(),
        matrix: u.matrix,
        nums: u.nums.clone(),
        tags: u.tags.clone(),
        blobs: u.blobs.clone(),
        edges: u.edges.clone(),
        counts: u.counts.clone(),
        by_id: u.by_id.clone(),
        avatar: u.avatar.clone(),
        motto: u.motto.clone(),
        stamp: u.stamp,
        ratio: u.ratio,
        flag: u.flag,
        small: u.small,
        wide: u.wide,
        offset: u.offset,
        precise_id: u.precise_id,
    };
    assert_eq!(old.to_owned(), expected);
}

#[test]
fn new_reader_reads_old_buffer_with_defaults() {
    let v1 = UserV1 {
        id: 7,
        name: "old".to_string(),
        color: message_fixtures::Color::Red,
        profile: Profile { age: 1, bio: "b".to_string(), nickname: None },
        matrix: [0.0; 4],
        nums: vec![5],
        tags: vec![],
        blobs: vec![],
        edges: vec![],
        counts: HashMap::new(),
        by_id: HashMap::new(),
        avatar: None,
        motto: None,
        stamp: 1,
        ratio: 0.5,
        flag: false,
        small: 1,
        wide: 2,
        offset: 3,
        precise_id: 4,
    };
    let buf = v1.encode();
    let r = User::decode(&buf).expect("the new reader must decode old data");
    assert_eq!(r.header_version(), 0);
    // base fields are all correct
    assert_eq!(r.id(), 7);
    assert_eq!(r.name(), "old");
    assert_eq!(r.profile().age(), 1);
    assert_eq!(r.nums().get(0), 5);
    // since-fields degrade to defaults
    assert_eq!(r.email(), None);
    assert_eq!(r.score(), 0.0);
    assert_eq!(r.tags2().len(), 0);
    assert_eq!(r.vip(), None);
}

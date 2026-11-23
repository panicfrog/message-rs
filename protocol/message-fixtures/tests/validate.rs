//! `Xxx::validate`: strict check of untrusted buffers. After `Ok` no accessor panics; every
//! `Err` names the field path where the problem was found.

use message_core::{read_slot, FieldDescriptor, Message, MessageDescriptor, HEADER_LEN};
use message_derive::Message;
use message_fixtures::{sample_tally, sample_user, Color, Profile, Tally, User};
use std::collections::HashMap;

fn field<'a>(d: &'a MessageDescriptor, name: &str) -> &'a FieldDescriptor {
    d.fields.iter().find(|f| f.name == name).unwrap()
}

/// Absolute buffer position of a base field.
fn pos_of(m: &MessageDescriptor, name: &str) -> usize {
    HEADER_LEN + field(m, name).offset
}

fn put_u32(buf: &mut [u8], pos: usize, v: u32) {
    buf[pos..pos + 4].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn canonical_encodings_validate() {
    assert_eq!(User::validate(&sample_user().encode()), Ok(()));
    assert_eq!(Tally::validate(&sample_tally().encode()), Ok(()));
    assert_eq!(message_fixtures::Legacy::validate(&message_fixtures::Legacy { id: 1, old_flag: true, old_name: "x".into(), note: "n".into() }.encode()), Ok(()));
}

#[test]
fn header_is_checked() {
    let buf = sample_user().encode();
    let short = User::validate(&buf[..10]);
    assert!(matches!(short, Err(message_core::ValidationError::TooShort { .. })), "{short:?}");
    let mut bad_key = buf.clone();
    bad_key[0] ^= 0xFF;
    assert!(matches!(User::validate(&bad_key), Err(message_core::ValidationError::KeyMismatch { .. })));
    let mut bad_len = buf.clone();
    bad_len[4] ^= 0xFF;
    assert!(matches!(User::validate(&bad_len), Err(message_core::ValidationError::FixedLenMismatch { .. })));
}

#[test]
fn slot_out_of_bounds_is_reported_with_its_field() {
    let mut buf = sample_user().encode();
    put_u32(&mut buf, pos_of(User::__descriptor(), "name"), u32::MAX);
    let err = User::validate(&buf).unwrap_err();
    let payload = buf.len() - HEADER_LEN;
    assert_eq!(err.to_string(), format!("at `name`: slot out of bounds (off=4294967295 len=6, payload={payload})"), "{err:?}");
}

#[test]
#[should_panic(expected = "slot out of bounds")]
fn lazy_accessors_panic_on_the_same_buffer() {
    let mut buf = sample_user().encode();
    put_u32(&mut buf, pos_of(User::__descriptor(), "name"), u32::MAX);
    let r = User::decode(&buf).unwrap();
    let _ = r.name();
}

#[test]
fn invalid_utf8_is_reported() {
    let mut buf = sample_user().encode();
    let name = pos_of(User::__descriptor(), "name");
    let (o, _) = read_slot(&buf, name);
    buf[HEADER_LEN + o as usize] = 0xFF;
    let err = User::validate(&buf).unwrap_err();
    assert_eq!(err.to_string(), "at `name`: invalid utf-8", "{err:?}");
}

#[test]
fn unknown_enum_value_is_reported() {
    let mut buf = sample_user().encode();
    put_u32(&mut buf, pos_of(User::__descriptor(), "color"), 99);
    let err = User::validate(&buf).unwrap_err();
    assert_eq!(err.to_string(), "at `color`: unknown Color value 99", "{err:?}");
}

#[test]
fn nested_errors_carry_the_path() {
    let user = User::__descriptor();
    let payload = sample_user().encode().len() - HEADER_LEN;

    // map value struct field: by_id[0] -> Profile.bio
    let by_id = field(user, "by_id");
    let mut buf = sample_user().encode();
    let (o, _) = read_slot(&buf, HEADER_LEN + by_id.offset);
    let bio_slot = HEADER_LEN + o as usize + by_id.map_value_offset.unwrap() + field(Profile::__descriptor(), "bio").offset;
    put_u32(&mut buf, bio_slot, u32::MAX);
    let err = User::validate(&buf).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("at `by_id`[0]: at `bio`: slot out of bounds (off=4294967295 len=6, payload={payload})"),
        "{err:?}"
    );

    // array element slot: tags[1]
    let mut buf = sample_user().encode();
    let tags = field(user, "tags");
    let (o, _) = read_slot(&buf, HEADER_LEN + tags.offset);
    put_u32(&mut buf, HEADER_LEN + o as usize + tags.element_size.unwrap(), u32::MAX);
    let err = User::validate(&buf).unwrap_err();
    assert!(err.to_string().starts_with("at `tags`[1]: slot out of bounds"), "{err:?}");

    // element count overflow: nums claims more elements than the payload can hold
    let mut buf = sample_user().encode();
    let nums = field(user, "nums");
    put_u32(&mut buf, HEADER_LEN + nums.offset + 4, u32::MAX);
    let err = User::validate(&buf).unwrap_err();
    assert!(err.to_string().starts_with("at `nums`: slot out of bounds"), "{err:?}");
}

#[test]
fn extension_chain_is_checked() {
    let malformed = "extension block v3 missing or malformed";
    let versions = <User as Message>::EXT_VERSIONS;

    // trailer of the last block names a version that is not there
    let mut buf = sample_user().encode();
    let n = buf.len();
    buf[n - 6..n - 4].copy_from_slice(&9u16.to_le_bytes());
    assert_eq!(User::validate(&buf).unwrap_err().to_string(), malformed);

    // last block shorter than its fixed region (and the lazy reader degrades instead of panicking)
    let mut buf = sample_user().encode();
    let n = buf.len();
    buf[n - 4..n].copy_from_slice(&7u32.to_le_bytes());
    assert_eq!(User::validate(&buf).unwrap_err().to_string(), malformed);
    assert!(User::decode(&buf).unwrap().vip().is_none());

    // v2's trailer is broken: stepping over v3 lands on garbage
    let mut buf = sample_user().encode();
    let (v3_start, _) = message_core::find_ext_block(&buf[HEADER_LEN..], versions, 3, 3, 0).unwrap();
    buf[HEADER_LEN + v3_start - 6] = 9;
    assert_eq!(User::validate(&buf).unwrap_err().to_string(), "extension block v2 missing or malformed");

    // header promises v3 but only v2 was written
    let mut buf = sample_user().encode();
    let (v3_start, _) = message_core::find_ext_block(&buf[HEADER_LEN..], versions, 3, 3, 0).unwrap();
    buf.truncate(HEADER_LEN + v3_start);
    assert_eq!(User::validate(&buf).unwrap_err().to_string(), malformed);
}

/// A reader that only knows v2 (simulates the schema before v3 was appended).
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0001)]
struct UserV2 {
    pub id: u64,
    pub name: String,
    pub color: Color,
    pub profile: Profile,
    pub matrix: [f32; 4],
    pub nums: Vec<i32>,
    pub tags: Vec<String>,
    pub blobs: Vec<Vec<u8>>,
    pub edges: Vec<message_fixtures::Node>,
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
    #[msg(precise)]
    pub precise_id: u64,
    #[msg(since = 2)]
    pub email: Option<String>,
    #[msg(since = 2)]
    pub score: f64,
}

fn v2_value() -> UserV2 {
    let u = sample_user();
    UserV2 {
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
        email: None,
        score: 0.0,
    }
}

#[test]
fn older_reader_finds_its_blocks_behind_a_newer_writer() {
    let buf = sample_user().encode(); // written by a v3 writer
    assert_eq!(UserV2::validate(&buf), Ok(()), "a newer writer's buffer is valid for an older reader");
    let r = UserV2::decode(&buf).unwrap();
    assert_eq!(r.header_version(), 3);
    assert_eq!(r.email(), Some("user@example.com"), "v2 data must be read even though a v3 block sits behind it");
    assert_eq!(r.score(), 99.5);
}

#[test]
fn newer_reader_validates_an_older_writer() {
    let buf = v2_value().encode(); // written by a v2 writer
    assert_eq!(User::validate(&buf), Ok(()));
    let r = User::decode(&buf).unwrap();
    assert_eq!(r.header_version(), 2);
    assert_eq!(r.email(), None);
    assert_eq!(r.vip(), None);
}

#[test]
fn reserved_fields_are_not_validated() {
    // Reserved = deleted from the API: nothing reads those bytes, so they are not checked
    // (a TS writer zeroes them, which may not even be a valid value for the original type).
    let mut buf = message_fixtures::Legacy { id: 1, old_flag: true, old_name: "x".into(), note: "n".into() }.encode();
    let old_name = field(message_fixtures::Legacy::__descriptor(), "old_name");
    put_u32(&mut buf, HEADER_LEN + old_name.offset, u32::MAX);
    assert_eq!(message_fixtures::Legacy::validate(&buf), Ok(()));
}

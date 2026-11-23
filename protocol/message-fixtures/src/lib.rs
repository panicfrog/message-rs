//! Test message definitions.

use message_derive::{Message, MessageEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MessageEnum)]
pub enum Color {
    Red,
    Green,
    Blue,
}

/// Inline nested struct (with an optional bitmap and string slots).
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0002)]
pub struct Profile {
    pub age: u32,
    pub bio: String,
    pub nickname: Option<String>,
}

/// Recursion through a dynamic array.
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0003)]
pub struct Node {
    pub value: u32,
    pub children: Vec<Node>,
}

/// Reserved placeholders: laid out and encoded from the struct, but hidden from the Reader API
/// and reset to `Default` by `to_owned()`.
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0004)]
pub struct Legacy {
    pub id: u32,
    #[msg(reserved)]
    pub old_flag: bool,
    #[msg(reserved)]
    pub old_name: String,
    pub note: String,
}

/// Enum-keyed map (entries sorted by enum value) and array-valued map.
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0005)]
pub struct Tally {
    pub usage: std::collections::HashMap<Color, u32>,
    pub labels: std::collections::HashMap<String, Vec<String>>,
}

/// Sample data for [`Tally`] (shared by tests and the conformance vectors).
pub fn sample_tally() -> Tally {
    let mut usage = std::collections::HashMap::new();
    usage.insert(Color::Blue, 3u32);
    usage.insert(Color::Red, 1u32);
    let mut labels = std::collections::HashMap::new();
    labels.insert("a".to_string(), vec!["x".to_string(), "y".to_string()]);
    labels.insert("b".to_string(), vec![]);
    Tally { usage, labels }
}

/// Main fixture covering the full V1 type set.
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 0x0001)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub color: Color,
    pub profile: Profile,
    pub matrix: [f32; 4],
    pub nums: Vec<i32>,
    pub tags: Vec<String>,
    pub blobs: Vec<Vec<u8>>,
    pub edges: Vec<Node>,
    pub counts: std::collections::HashMap<String, u32>,
    pub by_id: std::collections::HashMap<u64, Profile>,
    pub avatar: Option<Vec<u8>>,
    pub motto: Option<String>,
    pub stamp: i64,
    pub ratio: f64,
    pub flag: bool,
    pub small: u8,
    pub wide: u16,
    pub offset: i16,
    /// Precise u64: maps to bigint on the TS side.
    #[msg(precise)]
    pub precise_id: u64,
    /// v2 extension block
    #[msg(since = 2)]
    pub email: Option<String>,
    #[msg(since = 2)]
    pub score: f64,
    /// v3 extension block
    #[msg(since = 3)]
    pub tags2: Vec<String>,
    #[msg(since = 3)]
    pub vip: Option<bool>,
}

/// Sample data covering the full type set (shared by tests).
pub fn sample_user() -> User {
    User {
        id: 42,
        name: "山田".to_string(),
        color: Color::Green,
        profile: Profile {
            age: 30,
            bio: "hello 世界".to_string(),
            nickname: Some("yamada".to_string()),
        },
        matrix: [1.5, -2.25, 3.125, 0.0],
        nums: vec![i32::MIN, -1, 0, 7, i32::MAX],
        tags: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        blobs: vec![vec![1, 2, 3], vec![], vec![255; 300]],
        edges: vec![
            Node { value: 1, children: vec![Node { value: 11, children: vec![] }] },
            Node { value: 2, children: vec![] },
        ],
        counts: {
            let mut m = std::collections::HashMap::new();
            m.insert("one".to_string(), 1u32);
            m.insert("two".to_string(), 2u32);
            m
        },
        by_id: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                7u64,
                Profile { age: 25, bio: "nested".to_string(), nickname: None },
            );
            m
        },
        avatar: Some(vec![9, 8, 7]),
        motto: None,
        stamp: -1700000000000,
        ratio: 3.141592653589793,
        flag: true,
        small: 200,
        wide: 65535,
        offset: -32768,
        precise_id: 0xFFFF_FFFF_FFFF_FFFF,
        email: Some("user@example.com".to_string()),
        score: 99.5,
        tags2: vec!["v3".to_string()],
        vip: Some(true),
    }
}

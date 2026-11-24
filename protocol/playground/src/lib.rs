//! Playground: freely edit the message definitions here, then:
//!
//! - verify in Rust: `cargo test -p playground`
//! - export JSON IR: `cargo run -p playground --bin export-schema -- export playground-snapshot.json`
//! - evolution gate: `cargo run -p playground --bin export-schema -- check playground-snapshot.json`
//! - single-type debug: `cargo run -p playground --bin export-schema -- ir --key 100`
//! - generate TS: `cargo run -p playground --bin export-schema -- ts /path/to/schema.ts`
//! - generate Swift: `cargo run -p playground --bin export-schema -- swift /path/to/Schema.swift`
//! - generate Kotlin: `cargo run -p playground --bin export-schema -- kotlin /path/to/Schema.kt [--package com.example]`
//!
//! Rules cheat sheet:
//! - every type needs a unique non-zero key;
//! - evolution is append-only; mark new fields `#[msg(since = N)]` with N above the current max;
//! - to "delete" a field use a `#[msg(reserved)]` placeholder instead of removing it;
//! - mark u64 fields needing >2^53 precision (especially for TS) as `#[msg(precise)]`.

use message_derive::{Message, MessageEnum};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, MessageEnum)]
pub enum Status {
    Offline = 0,
    Online = 1,
    Busy = 2,
}

/// The simplest possible message.
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 100)]
pub struct Ping {
    pub seq: u64,
    pub sent_at: i64,
}

/// Slightly richer: nesting, optionals, arrays, maps, versioned extensions.
#[derive(Debug, Clone, PartialEq, Message)]
#[msg(key = 101)]
pub struct ChatMessage {
    pub id: u64,
    pub from: String,
    pub text: String,
    pub status: Status,
    pub mentions: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub reply_to: Option<u64>,
    /// appended in v2
    #[msg(since = 2)]
    pub edited_at: Option<i64>,
    /// appended in v3
    #[msg(since = 3)]
    pub reactions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_roundtrip() {
        let p = Ping { seq: 7, sent_at: 1700000000 };
        let buf = p.encode();
        let r = Ping::decode(&buf).unwrap();
        assert_eq!(r.seq(), 7);
        assert_eq!(r.sent_at(), 1700000000);
        assert_eq!(r.to_owned(), p);
    }

    #[test]
    fn chat_roundtrip() {
        let mut m = HashMap::new();
        m.insert("client".to_string(), "ios".to_string());
        let msg = ChatMessage {
            id: 42,
            from: "山田".to_string(),
            text: "hello".to_string(),
            status: Status::Online,
            mentions: vec!["@a".to_string()],
            metadata: m,
            reply_to: Some(41),
            edited_at: None,
            reactions: vec!["👍".to_string()],
        };
        let buf = msg.encode();
        let r = ChatMessage::decode(&buf).unwrap();
        assert_eq!(r.id(), 42);
        assert_eq!(r.from(), "山田");
        assert_eq!(r.status(), Status::Online);
        assert_eq!(r.mentions().get(0), "@a");
        assert_eq!(r.metadata().value(0), "ios");
        assert_eq!(r.reply_to(), Some(41));
        // extension-block fields
        assert_eq!(r.edited_at(), None);
        assert_eq!(r.reactions().get(0), "👍");
        assert_eq!(r.to_owned(), msg);
    }
}

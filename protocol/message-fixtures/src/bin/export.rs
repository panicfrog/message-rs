// Fixtures export binary: forwards the generic message-gen commands (export/check/ir/ts)
// and additionally provides `vectors` to emit cross-language conformance vectors.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).cloned().unwrap_or_else(|| "export".to_string());
    if cmd == "vectors" {
        let out = args
            .get(2)
            .cloned()
            .unwrap_or_else(|| "vectors/user.json".to_string());
        std::process::exit(export_vectors(&out));
    }
    let rest: Vec<String> = args.iter().skip(2).cloned().collect();
    std::process::exit(message_gen::run(&cmd, &rest));
}

fn export_vectors(out: &str) -> i32 {
    use serde_json::{json, Value};

    fn node_json(n: &message_fixtures::Node) -> Value {
        json!({
            "value": n.value,
            "children": n.children.iter().map(node_json).collect::<Vec<_>>(),
        })
    }

    let u = message_fixtures::sample_user();
    let hex: String = u.encode().iter().map(|b| format!("{b:02x}")).collect();
    let expected = json!({
        "id": u.id,
        "name": u.name,
        "color": u.color as i32,
        "profile": {
            "age": u.profile.age,
            "bio": u.profile.bio,
            "nickname": u.profile.nickname,
        },
        "matrix": u.matrix,
        "nums": u.nums,
        "tags": u.tags,
        "blobs": u.blobs,
        "edges": u.edges.iter().map(node_json).collect::<Vec<_>>(),
        "counts": { "one": 1u32, "two": 2u32 },
        "by_id": {
            "7": { "age": 25u32, "bio": "nested", "nickname": null },
        },
        "avatar": u.avatar,
        "motto": null,
        "stamp": u.stamp,
        "ratio": u.ratio,
        "flag": u.flag,
        "small": u.small,
        "wide": u.wide,
        "offset": u.offset,
        // u64 beyond number precision is carried as a string (the encoder restores it via BigInt(str))
        "precise_id": u.precise_id.to_string(),
        "email": u.email,
        "score": u.score,
        "tags2": u.tags2,
        "vip": u.vip,
    });
    let doc = json!({
        "key": 1u32,
        "name": "User",
        "hex": hex,
        "expected": expected,
    });
    let t = message_fixtures::sample_tally();
    let tally = json!({
        "key": 5u32,
        "name": "Tally",
        "hex": t.encode().iter().map(|b| format!("{b:02x}")).collect::<String>(),
        // enum map keys are the numeric value as a string (the TS object shape)
        "expected": {
            "usage": { "0": 1u32, "2": 3u32 },
            "labels": { "a": ["x", "y"], "b": [] },
        },
    });
    let tally_out = std::path::Path::new(out).with_file_name("tally.json");
    match serde_json::to_string_pretty(&doc)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(out, s).map_err(|e| e.to_string()))
        .and_then(|_| {
            serde_json::to_string_pretty(&tally)
                .map_err(|e| e.to_string())
                .and_then(|s| std::fs::write(&tally_out, s).map_err(|e| e.to_string()))
        }) {
        Ok(()) => {
            println!("vectors written to {out} and {}", tally_out.display());
            0
        }
        Err(e) => {
            eprintln!("write failed: {e}");
            1
        }
    }
}

//! The crate-level invariants from `lib.rs`, each as a numbered test over adversarial input.

mod common;

use common::{Lcg, gen_tool, tool};
use nasiko_tool_compact::{
    StreamDecoder, ToolDef, decode_calls, decode_tools, encode_tools, limits,
};
use serde_json::json;

fn catalog() -> Vec<ToolDef> {
    vec![tool(
        "t",
        json!({"type": "object", "properties": {"a": {"type": "string"}, "n": {"type": "integer"}}, "required": ["a"]}),
    )]
}

fn corpus() -> Vec<String> {
    let mut c: Vec<String> = [
        "",
        "<",
        "<<",
        "<<c",
        "<<call",
        "<<call ",
        "<<call t",
        "<<call t ",
        "<<call t {",
        "<<call t {\"a\"",
        "<<call t {\"a\":\"}>>\"}",
        "<<call t {\"a\":\"}>>\"}>>",
        "<<call t {\"a\":1}>>",
        "<<call t {}>>",
        "<<call t {\"a\":\"x\"}>>",
        "<<call t {\"a\":\"x\"}>>>>",
        "<<call t {\"a\":\"x\"}>> <<call t {\"a\":\"y\"}>>",
        "<<call t {\"a\":\"x\",\"n\":1.0}>>",
        "<<call t {\"a\":\"x\",\"n\":1e400}>>",
        "<<call t {\"a\":\"\\ud800\"}>>",
        "<<call t {\"a\":\"\\u0000\"}>>",
        "<<call t {\"a\":\"\u{0}\"}>>",
        "<<call t {\"a\":\"x\"}>>\u{feff}",
        "<<call t {\"a\":\"x\"} \t\n>>",
        "<<call\tt\t{\"a\":\"x\"}>>",
        "<<call t {\"a\":\"x\"}}>>",
        "<<call t {\"a\":\"x\"]>>",
        "<<call t [\"a\"]>>",
        "<<call t null>>",
        "<<call t \"a\">>",
        "<<call t 1>>",
        "<<call t {\"a\":\"x\" >>",
        "<<call t {\"a\":\"\\\"}>>",
        "<<call t {\"a\":\"\\\\\"}>>",
        "<<call t {\"a\":\"x\"}>><<call",
        "<<call t {\"a\":\"x\"}>><<call t",
        "\u{202e}<<call t {\"a\":\"x\"}>>",
        "e\u{301}<<call t {\"a\":\"e\u{301}\"}>>",
        "<<call t {\"a\":\"x\",\"n\":-0}>>",
        "<<call t {\"a\":\"x\",\"n\":18446744073709551616}>>",
        "<<call t {\"a\":\"x\",\"n\":-9223372036854775809}>>",
        "<<call t {\"a\":\"x\", }>>",
        "<<call t {,}>>",
        "<<call t {\"a\":\"x\"}>>\n\n<<call t {\"a\":\"y\"}>>\n",
        "<<call  t  {}  >>",
        "<<call t{}>>",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    c.push("<".repeat(100_000));
    c.push("<<call ".repeat(2_000));
    c.push(format!("<<call t {{\"a\":\"{}\"}}>>", "x".repeat(300_000)));
    c.push(format!(
        "<<call t {{\"a\":{}{}}}>>",
        "[".repeat(1_000),
        "]".repeat(1_000)
    ));
    c.push(format!("<<call t {{\"a\":{}}}>>", "{\"b\":".repeat(500)));
    c.push(format!("<<call {} {{}}>>", "n".repeat(10_000)));
    c
}

#[test]
fn i1_adversarial_input_never_panics_at_any_split() {
    let cat = catalog();
    for text in corpus() {
        let whole = decode_calls(&text, &cat);
        // Split positions: every char boundary for short inputs, a stride for the big ones.
        let stride = if text.len() > 2_000 {
            text.len() / 37 + 1
        } else {
            1
        };
        let mut i = 0;
        while i <= text.len() {
            if text.is_char_boundary(i) {
                let mut d = StreamDecoder::new(&cat).unwrap();
                let res = d
                    .push(&text[..i])
                    .and_then(|()| d.push(&text[i..]))
                    .and_then(|()| d.finish());
                assert_eq!(res, whole, "split {i} of {} bytes", text.len());
            }
            i += stride;
        }
    }
}

#[test]
fn i2_failure_releases_nothing() {
    let cat = catalog();
    for text in corpus() {
        if let Ok(d) = decode_calls(&text, &cat) {
            // Successful decodes carry only validated calls.
            for call in &d.calls {
                assert_eq!(call.name, "t");
                assert!(call.arguments.get("a").is_some_and(|v| v.is_string()));
            }
        }
        // Streaming: an error on push means finish also errors, with the same error.
        let mut d = StreamDecoder::new(&cat).unwrap();
        if let Err(e) = d.push(&text) {
            assert_eq!(d.finish(), Err(e));
        }
    }
}

#[test]
fn i3_encoding_is_deterministic_and_independent_of_input_map_order() {
    let mut rng = Lcg::new(1);
    for i in 0..200 {
        let tools: Vec<ToolDef> = (0..3).map(|j| gen_tool(&mut rng, i * 10 + j)).collect();
        let a = encode_tools(&tools).unwrap();
        let b = encode_tools(&tools).unwrap();
        assert_eq!(a, b);
        // Re-serializing the parameters through a JSON round trip (which may reorder keys in
        // either direction depending on serde_json features) must not change the output.
        let reparsed: Vec<ToolDef> = tools
            .iter()
            .map(|t| ToolDef {
                parameters: t
                    .parameters
                    .as_ref()
                    .map(|p| serde_json::from_str(&p.to_string()).unwrap()),
                ..t.clone()
            })
            .collect();
        assert_eq!(encode_tools(&reparsed).unwrap(), a);
    }
}

#[test]
fn i4_streaming_equals_one_shot_on_the_corpus() {
    let cat = catalog();
    let mut rng = Lcg::new(5);
    for text in corpus() {
        let whole = decode_calls(&text, &cat);
        for _ in 0..5 {
            let mut d = StreamDecoder::new(&cat).unwrap();
            let mut start = 0;
            let mut res = Ok(());
            while start < text.len() {
                let mut end = (start + 1 + rng.below(4_000) as usize).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                if let Err(e) = d.push(&text[start..end]) {
                    res = Err(e);
                    break;
                }
                start = end;
            }
            let res = res.and_then(|()| d.finish());
            assert_eq!(res, whole);
        }
    }
}

#[test]
fn i5_encode_never_returns_a_line_that_does_not_roundtrip() {
    let mut rng = Lcg::new(2);
    let mut accepted = 0;
    for _ in 0..1_000 {
        let tools: Vec<ToolDef> = (0..5).map(|j| gen_tool(&mut rng, j)).collect();
        if let Ok(compact) = encode_tools(&tools) {
            accepted += 1;
            assert_eq!(decode_tools(&compact).unwrap(), tools);
        }
    }
    assert!(
        accepted > 900,
        "generator should mostly produce supported catalogs: {accepted}"
    );
}

#[test]
fn i6_every_limit_constant_is_positive_and_consistent() {
    // Evaluated at compile time: a wrong relation between limits fails the build of this test.
    const _: () = {
        assert!(limits::MAX_ARGS_BYTES <= limits::MAX_TOTAL_ARGS_BYTES);
        assert!(limits::MAX_TOTAL_ARGS_BYTES <= limits::MAX_RESPONSE_BYTES);
        assert!(limits::MAX_PROPERTIES <= limits::MAX_SCHEMA_NODES);
        assert!(limits::MAX_DESCRIPTION_BYTES < limits::MAX_SCHEMA_BYTES);
        assert!(limits::MAX_COMPACT_BYTES <= limits::MAX_SCHEMA_BYTES);
        assert!(limits::MAX_NAME_LEN > 0);
        assert!(limits::MAX_CALLS > 0);
        assert!(limits::MAX_DEPTH > 0);
        assert!(limits::MAX_TOOLS > 0);
    };
}

#[test]
fn i7_the_crate_has_no_io_or_environment_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let deps: Vec<&str> = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap()
        .split("[dev-dependencies]")
        .next()
        .unwrap()
        .lines()
        .filter(|l| l.contains('='))
        .map(|l| l.split('=').next().unwrap().trim())
        .collect();
    assert_eq!(deps, vec!["serde", "serde_json", "thiserror"]);
    for file in [
        include_str!("../src/lib.rs"),
        include_str!("../src/stream.rs"),
        include_str!("../src/schema.rs"),
        include_str!("../src/catalog.rs"),
        include_str!("../src/encode.rs"),
        include_str!("../src/validate.rs"),
        include_str!("../src/render.rs"),
        include_str!("../src/parse.rs"),
        include_str!("../src/call.rs"),
        include_str!("../src/json.rs"),
        include_str!("../src/lexeme.rs"),
    ] {
        assert!(!file.contains("std::env"));
        assert!(!file.contains("std::fs"));
        assert!(!file.contains("std::net"));
        assert!(!file.contains("std::time"));
    }
}

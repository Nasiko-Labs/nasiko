//! Deterministic property tests (seeded LCG, no extra dependencies).
mod common;

use common::*;
use nasiko_tool_compact::{
    Error, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools, render_call,
};
use serde_json::{Map, Value, json};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

fn stream(tools: &[ToolDef], chunks: &[&str]) -> Result<Vec<ToolCall>, Error> {
    let mut d = StreamDecoder::new(tools)?;
    let mut out = vec![];
    for c in chunks {
        out.extend(d.push(c)?);
    }
    d.finish()?;
    Ok(out)
}

fn random_chunks<'a>(r: &mut Rng, s: &'a str) -> Vec<&'a str> {
    let mut out = vec![];
    let mut start = 0;
    for (i, _) in s.char_indices().skip(1) {
        if r.chance(20) {
            out.push(&s[start..i]);
            start = i;
        }
    }
    out.push(&s[start..]);
    out
}

#[test]
fn malformed_model_output_never_panics_and_chunking_never_matters() {
    let tools = [calendar(), email()];
    let atoms = [
        "<<call ",
        "<<call",
        "<<",
        "<",
        ">>",
        ">",
        " ",
        "\n",
        "{",
        "}",
        "[",
        "]",
        "\"",
        "\\",
        "\\\"",
        ":",
        ",",
        "send_email",
        "create_calendar_event",
        "nope",
        "\"to\"",
        "\"subject\"",
        "\"body\"",
        "\"title\"",
        "\"start\"",
        "\"x >> y\"",
        "1",
        "-0",
        "1e999",
        "null",
        "true",
        "नमस्ते",
        "😀",
        "\u{0}",
        "\r\n",
        "2026-10-05T15:00:00Z",
        "{}",
        "[]",
    ];
    let mut r = Rng(0xC0FFEE);
    let mut calls_seen = 0;
    for _ in 0..20_000 {
        let n = 1 + r.below(24);
        let text: String = (0..n).map(|_| atoms[r.below(atoms.len())]).collect();
        let batch = decode_calls(&text, &tools);
        let chunks = random_chunks(&mut r, &text);
        let streamed = stream(&tools, &chunks);
        assert_eq!(batch, streamed, "text={text:?} chunks={chunks:?}");
        if let Ok(c) = batch {
            calls_seen += c.len();
        }
    }
    // Pure noise should essentially never forge a valid call.
    assert!(
        calls_seen < 50,
        "suspiciously many calls from noise: {calls_seen}"
    );
}

#[test]
fn mutated_valid_calls_never_panic_and_stay_consistent() {
    let tools = [calendar(), email()];
    let seed = r#"<<call create_calendar_event {"title":"Meet","start":"2026-10-05T15:00:00Z","attendees":["a@b.c"],"visibility":"public"}>>"#;
    let mut r = Rng(42);
    for _ in 0..20_000 {
        let mut chars: Vec<char> = seed.chars().collect();
        for _ in 0..1 + r.below(3) {
            let i = r.below(chars.len());
            match r.below(3) {
                0 => {
                    chars.remove(i);
                }
                1 => chars.insert(i, ['"', '}', '>', '\\', '{', ','][r.below(6)]),
                _ => chars[i] = ['x', ' ', '"', '>', '['][r.below(5)],
            }
        }
        let text: String = chars.into_iter().collect();
        let batch = decode_calls(&text, &tools);
        let chunks = random_chunks(&mut r, &text);
        assert_eq!(batch, stream(&tools, &chunks), "{text:?}");
        // Whatever comes back as Ok must satisfy the schema we wrote down.
        if let Ok(calls) = batch {
            for c in calls {
                let v = args(&c);
                if c.name == "create_calendar_event" {
                    assert!(v["title"].is_string() && v["start"].is_string(), "{text:?}");
                    if let Some(vis) = v.get("visibility") {
                        assert!(vis == "public" || vis == "private", "{text:?}");
                    }
                }
            }
        }
    }
}

// ── schema round trip ───────────────────────────────────────────────────────

/// Random schema in the supported subset, paired with a random valid value.
fn gen_ty(r: &mut Rng, depth: usize) -> (Value, Value) {
    let kinds = if depth == 0 { 6 } else { 9 };
    match r.below(kinds) {
        0 => (
            json!({"type":"string"}),
            json!(["", "a", "x >> y", "q\"uote", "नमस्ते"][r.below(5)]),
        ),
        1 => (json!({"type":"integer"}), json!(r.below(1000) as i64 - 500)),
        2 => (json!({"type":"number"}), json!(r.below(1000) as f64 / 8.0)),
        3 => (json!({"type":"boolean"}), json!(r.chance(50))),
        4 => (
            json!({"type":"string","format":"date-time"}),
            json!("2026-10-05T15:00:00+05:30"),
        ),
        5 => {
            let vals = ["red", "green", "blue", "a-b", "c.d"];
            let n = 2 + r.below(3);
            let e: Vec<&str> = vals[..n].to_vec();
            (json!({"type":"string","enum":e}), json!(e[r.below(n)]))
        }
        6 => {
            let (s, v) = gen_ty(r, depth - 1);
            let len = r.below(3);
            (
                json!({"type":"array","items":s}),
                Value::Array(vec![v; len]),
            )
        }
        _ => gen_object(r, depth - 1),
    }
}

fn gen_object(r: &mut Rng, depth: usize) -> (Value, Value) {
    let mut props = Map::new();
    let mut required = vec![];
    let mut value = Map::new();
    let names = ["alpha", "beta", "gamma", "delta", "eps_1", "_z"];
    for name in names.iter().take(r.below(5)) {
        let (mut s, v) = gen_ty(r, depth);
        if r.chance(40) {
            s.as_object_mut().unwrap().insert(
                "description".into(),
                json!(["d", "has \"q\", ) (", "ü"][r.below(3)]),
            );
        }
        props.insert((*name).into(), s);
        if r.chance(50) {
            required.push(json!(name));
            value.insert((*name).into(), v);
        } else if r.chance(50) {
            value.insert((*name).into(), v);
        }
    }
    // Shuffle `required` so declaration order differs from alphabetical.
    if required.len() > 1 && r.chance(50) {
        required.reverse();
    }
    let mut schema = json!({"type":"object","properties":props});
    if !required.is_empty() {
        schema["required"] = Value::Array(required);
    }
    (schema, Value::Object(value))
}

#[test]
fn random_supported_schemas_round_trip_through_encode_decode_and_calls() {
    let mut r = Rng(7);
    for i in 0..3_000 {
        let (params, value) = gen_object(&mut r, 2);
        let desc = r
            .chance(60)
            .then(|| "Does a thing, (really) - ok".to_string());
        let params = (params["properties"]
            .as_object()
            .is_some_and(|p| !p.is_empty()))
        .then_some(params);
        let t = ToolDef {
            name: format!("tool_{i}"),
            description: desc,
            parameters: params,
        };

        let c = encode_tools(std::slice::from_ref(&t)).unwrap();
        assert!(
            c.native.is_empty(),
            "generator stays inside the supported subset: {t:?}\n{:?}",
            c.bypassed
        );
        assert_eq!(
            decode_tools(&c).unwrap(),
            vec![t.clone()],
            "{}",
            c.signatures
        );

        let text = render_call(&t.name, &value);
        let calls = decode_calls(&text, std::slice::from_ref(&t))
            .unwrap_or_else(|e| panic!("{e}\n{text}\n{t:?}"));
        assert_eq!(args(&calls[0]), value, "{text}");
    }
}

#[test]
fn empty_required_list_means_the_same_as_no_required_list() {
    let with = json!({"type":"object","properties":{"a":{"type":"string"}},"required":[]});
    let without = json!({"type":"object","properties":{"a":{"type":"string"}}});
    let a = encode_tools(&[tool("t", None, Some(with))]).unwrap();
    let b = encode_tools(&[tool("t", None, Some(without.clone()))]).unwrap();
    assert!(a.native.is_empty());
    assert_eq!(a.signatures, b.signatures);
    assert_eq!(decode_tools(&a).unwrap()[0].parameters, Some(without));
}

//! Seeded property tests (a fixed-seed generator keeps them deterministic and dependency-free).
//!
//! 1. Any valid call survives render → decode unchanged.
//! 2. Any chunking of any output (valid or not) decodes exactly like the whole text.
//! 3. Any supported schema survives encode → `decode_tools` with the Keep policy.

mod common;

use common::*;
use nasiko_tool_compact::{
    DescriptionPolicy, EncodeOptions, StreamDecoder, StreamEvent, ToolCall, ToolDef, decode,
    decode_tools, encode_tools_with, render_calls,
};
use serde_json::{Map, Value, json};

const CASES: usize = 2_000;

/// SplitMix64: tiny, deterministic, good enough for test generation.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Strings that stress the scanner: markers, braces, quotes, escapes, unicode.
fn nasty_string(rng: &mut Rng) -> String {
    const PARTS: &[&str] = &[
        "a",
        "Design review",
        ">>",
        "<<call x {}>>",
        "}",
        "{",
        "\"",
        "\\",
        "\n",
        "'",
        "é✓",
        " ",
        "<<",
        ">",
        "]",
        "[",
        "null",
        "\\\"",
    ];
    (0..rng.below(6)).map(|_| *rng.pick(PARTS)).collect()
}

fn random_date_time(rng: &mut Rng) -> String {
    let zone = *rng.pick(&["Z", "+05:30", "-08:00"]);
    format!(
        "2026-{:02}-{:02}T{:02}:{:02}:00{zone}",
        1 + rng.below(12),
        1 + rng.below(28),
        rng.below(24),
        rng.below(60)
    )
}

/// A random value valid for the given (supported-subset) schema.
fn valid_value(schema: &Value, rng: &mut Rng) -> Value {
    if schema["type"].as_array().is_some() && rng.chance(30) {
        return Value::Null;
    }
    if let Some(values) = schema["enum"].as_array() {
        return rng.pick(values).clone();
    }
    let ty = match &schema["type"] {
        Value::Array(ts) => ts[0].as_str().unwrap_or("string"),
        other => other.as_str().unwrap_or("any"),
    };
    match ty {
        "string" => match schema["format"].as_str() {
            Some("date-time") => json!(random_date_time(rng)),
            Some("date") => json!(format!(
                "2026-{:02}-{:02}",
                1 + rng.below(12),
                1 + rng.below(28)
            )),
            Some("uuid") => json!("123e4567-e89b-12d3-a456-426614174000"),
            _ => json!(nasty_string(rng)),
        },
        "integer" => {
            let lo = schema["minimum"].as_i64().unwrap_or(-1000);
            let hi = schema["maximum"].as_i64().unwrap_or(1000);
            json!(lo + rng.below((hi - lo + 1) as usize) as i64)
        }
        "number" => json!(rng.below(50) as f64 / 100.0),
        "boolean" => json!(rng.chance(50)),
        "array" => {
            let items = schema.get("items").cloned().unwrap_or(json!({}));
            Value::Array(
                (0..rng.below(3))
                    .map(|_| valid_value(&items, rng))
                    .collect(),
            )
        }
        "object" => {
            let mut map = Map::new();
            let required: Vec<&str> = schema["required"]
                .as_array()
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if let Some(props) = schema["properties"].as_object() {
                for (k, s) in props {
                    if required.contains(&k.as_str()) || rng.chance(50) {
                        map.insert(k.clone(), valid_value(s, rng));
                    }
                }
            }
            Value::Object(map)
        }
        _ => json!(nasty_string(rng)),
    }
}

fn random_call(tools: &[ToolDef], rng: &mut Rng) -> ToolCall {
    let tool = rng.pick(tools);
    let Value::Object(arguments) = valid_value(tool.parameters.as_ref().unwrap(), rng) else {
        unreachable!()
    };
    ToolCall {
        name: tool.name.clone(),
        arguments,
    }
}

fn split_randomly<'a>(text: &'a str, rng: &mut Rng) -> Vec<&'a str> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut at = 1 + rng.below(rest.len().min(12));
        while !rest.is_char_boundary(at) {
            at += 1;
        }
        let (head, tail) = rest.split_at(at);
        chunks.push(head);
        rest = tail;
    }
    chunks
}

/// Calls + concatenated text, or the error kind.
fn run_stream(chunks: &[&str], tools: &[ToolDef]) -> Result<(String, Vec<ToolCall>), String> {
    let mut d = StreamDecoder::new(tools).unwrap();
    let mut events = Vec::new();
    for c in chunks {
        events.extend(d.push(c).map_err(|e| e.kind().to_string())?);
    }
    events.extend(d.finish().map_err(|e| e.kind().to_string())?);
    let mut text = String::new();
    let mut calls = Vec::new();
    for e in events {
        match e {
            StreamEvent::Text(t) => text.push_str(&t),
            StreamEvent::Call(c) => calls.push(c),
        }
    }
    Ok((text, calls))
}

fn run_whole(text: &str, tools: &[ToolDef]) -> Result<(String, Vec<ToolCall>), String> {
    decode(text, tools)
        .map(|d| (d.text, d.calls))
        .map_err(|e| e.kind().to_string())
}

#[test]
fn valid_calls_round_trip_through_render_and_decode() {
    let tools = all_tools();
    let mut rng = Rng(1);
    for _ in 0..CASES {
        let calls: Vec<ToolCall> = (0..1 + rng.below(3))
            .map(|_| random_call(&tools, &mut rng))
            .collect();
        let text = format!("Sure!\n{}\nDone.", render_calls(&calls));
        let decoded = decode(&text, &tools).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(decoded.calls, calls, "{text}");
        // Only the prose and the newlines `render_calls` puts between calls remain as text.
        let separators = "\n".repeat(calls.len() - 1);
        assert_eq!(decoded.text, format!("Sure!\n{separators}\nDone."));
    }
}

#[test]
fn any_chunking_decodes_like_the_whole_text() {
    let tools = all_tools();
    let mut rng = Rng(2);
    for _ in 0..CASES {
        let calls: Vec<ToolCall> = (0..rng.below(3))
            .map(|_| random_call(&tools, &mut rng))
            .collect();
        let mut text = format!(
            "{}{}{}",
            nasty_prose(&mut rng),
            render_calls(&calls),
            nasty_prose(&mut rng)
        );
        // Corrupt some outputs so error paths are covered too.
        if rng.chance(40) && !text.is_empty() {
            let mut cut = rng.below(text.len());
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            let junk = *rng.pick(&["", ">", "}", "\"", "<<call ", "x", "{"]);
            text.insert_str(cut, junk);
            if rng.chance(50) {
                text.truncate(cut);
            }
        }
        let whole = run_whole(&text, &tools);
        let chunks = split_randomly(&text, &mut rng);
        assert_eq!(run_stream(&chunks, &tools), whole, "{text:?} as {chunks:?}");
    }
}

/// Prose without the call marker (the marker always starts a call).
fn nasty_prose(rng: &mut Rng) -> String {
    nasty_string(rng).replace("<<call", "<< call")
}

/// A random schema node from the supported subset.
fn random_schema(rng: &mut Rng, depth: usize) -> Value {
    let mut node = match rng.below(if depth > 2 { 6 } else { 9 }) {
        0 => json!({"type": "string"}),
        1 => {
            json!({"type": "string", "format": rng.pick(&["date-time", "date", "time", "email", "uri", "uuid"])})
        }
        2 => json!({"type": "integer", "minimum": rng.below(5), "maximum": 5 + rng.below(5)}),
        3 => json!({"type": "number"}),
        4 => json!({"type": "boolean"}),
        5 => {
            json!({"type": "string", "enum": (0..1 + rng.below(4)).map(|_| enum_word(rng)).collect::<Vec<_>>()})
        }
        6 => json!({"type": "array", "items": random_schema(rng, depth + 1)}),
        7 => json!({"type": "object"}),
        _ => random_object(rng, depth + 1),
    };
    if rng.chance(30) && node["enum"].is_null() && node["type"] != "object" {
        let t = node["type"].clone();
        node["type"] = json!([t, "null"]);
    }
    if rng.chance(40) {
        node["description"] = json!(nasty_string(rng));
    }
    node
}

fn enum_word(rng: &mut Rng) -> String {
    rng.pick(&[
        "a",
        "b-c",
        "str",
        "null",
        "two words",
        "it's",
        "1",
        "true",
        "x/y",
        "é",
        "{",
        "a|b",
    ])
    .to_string()
}

fn random_object(rng: &mut Rng, depth: usize) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for i in 0..rng.below(4) {
        let name = format!(
            "{}{i}",
            rng.pick(&["f", "user-id", "x y", "_n", "it's", "Δ"])
        );
        if rng.chance(50) {
            required.push(json!(name));
        }
        props.insert(name, random_schema(rng, depth));
    }
    required.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    let mut obj = json!({"type": "object", "properties": props});
    if !required.is_empty() {
        obj["required"] = Value::Array(required);
    }
    if rng.chance(30) {
        obj["additionalProperties"] = json!(false);
    }
    obj
}

fn sorted_required(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Array(req)) = map.get_mut("required") {
                    req.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                }
                map.values_mut().for_each(walk);
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

#[test]
fn supported_schemas_survive_encode_and_decode_tools() {
    let keep = EncodeOptions {
        descriptions: DescriptionPolicy::Keep,
    };
    let mut rng = Rng(3);
    for i in 0..CASES {
        let tool = ToolDef {
            name: format!("tool_{i}"),
            description: rng.chance(80).then(|| nasty_string(&mut rng)),
            parameters: Some(random_object(&mut rng, 0)),
        };
        let compact = encode_tools_with(std::slice::from_ref(&tool), keep).unwrap();
        let back = decode_tools(&compact)
            .unwrap_or_else(|e| panic!("{e}\n{}", compact.definitions()))
            .remove(0);
        assert_eq!(back.name, tool.name);
        assert_eq!(back.description, tool.description.filter(|d| !d.is_empty()));
        assert_eq!(
            sorted_required(back.parameters.unwrap()),
            sorted_required(tool.parameters.unwrap()),
            "{}",
            compact.definitions()
        );
    }
}

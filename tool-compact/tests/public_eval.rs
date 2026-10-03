//! The official public sample (`compact-tools-eval-v1`, vendored in `tests/data`) end to end:
//! schema survival, round trips, and every decoder case — fed chunk by chunk, unchanged.

use nasiko_tool_compact::{
    DescriptionPolicy, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools,
    normalize, render_calls,
};
use serde_json::{Value, json};

fn eval() -> Value {
    serde_json::from_str(include_str!("data/public_eval.json")).unwrap()
}

fn tool_defs(eval: &Value, names: &[Value]) -> Vec<ToolDef> {
    names
        .iter()
        .map(|n| {
            let f = eval["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["function"]["name"] == *n)
                .unwrap()["function"]
                .clone();
            ToolDef {
                name: f["name"].as_str().unwrap().into(),
                description: f["description"].as_str().map(str::to_string),
                parameters: f.get("parameters").cloned(),
            }
        })
        .collect()
}

fn as_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| json!({"name": c.name, "arguments": serde_json::from_str::<Value>(&c.arguments).unwrap()}))
            .collect(),
    )
}

#[test]
fn schemas_survive_compaction() {
    let e = eval();
    let all: Vec<Value> = e["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].clone())
        .collect();
    let tools = tool_defs(&e, &all);
    let compact = encode_tools(&tools).unwrap();
    let back = decode_tools(&compact).unwrap();
    for (orig, got) in tools.iter().zip(&back) {
        assert_eq!(got, &normalize(orig, DescriptionPolicy::Verbatim).unwrap());
        // Nothing beyond the documented normalization: same properties, same required.
        let o = orig.parameters.as_ref().unwrap();
        let g = got.parameters.as_ref().unwrap();
        assert_eq!(o["properties"], g["properties"]);
        assert_eq!(o["required"], g["required"]);
    }
}

#[test]
fn every_case_compacts_and_round_trips() {
    let e = eval();
    for case in e["cases"].as_array().unwrap() {
        let tools = tool_defs(&e, case["tools"].as_array().unwrap());
        encode_tools(&tools).unwrap_or_else(|err| panic!("{}: {err}", case["id"]));
        let expected: Vec<ToolCall> = case["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| ToolCall {
                name: c["name"].as_str().unwrap().into(),
                arguments: c["arguments"].to_string(),
            })
            .collect();
        let rendered = render_calls(&expected);
        let back = decode_calls(&rendered, &tools).unwrap();
        assert_eq!(as_json(&back), case["expected"], "{}", case["id"]);
    }
}

#[test]
fn every_decoder_case_matches_expected() {
    let e = eval();
    for case in e["decoder_cases"].as_array().unwrap() {
        let tools = tool_defs(&e, case["tools"].as_array().unwrap());
        let mut decoder = StreamDecoder::new(&tools).unwrap();
        let mut calls = Vec::new();
        let mut result = Ok(());
        for chunk in case["chunks"].as_array().unwrap() {
            match decoder.feed(chunk.as_str().unwrap()) {
                Ok(events) => calls.extend(events),
                Err(err) => {
                    result = Err(err);
                    break;
                }
            }
        }
        if result.is_ok() {
            match decoder.finish() {
                Ok(events) => calls.extend(events),
                Err(err) => result = Err(err),
            }
        }
        let got = match result {
            Ok(()) => {
                let calls: Vec<ToolCall> = calls
                    .into_iter()
                    .filter_map(|e| match e {
                        nasiko_tool_compact::Event::Call(c) => Some(c),
                        nasiko_tool_compact::Event::Text(_) => None,
                    })
                    .collect();
                json!({"calls": as_json(&calls)})
            }
            Err(err) => json!({"error": err.code()}),
        };
        assert_eq!(got, case["expected"], "{}", case["id"]);
    }
}

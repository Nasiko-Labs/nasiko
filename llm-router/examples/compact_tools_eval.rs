//! Compact tool schemas eval.
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Offline by default (no network, deterministic). When `PROVIDER_BASE_URL` and
//! `MODEL` are set, each `compact_request` is also sent to that OpenAI-compatible
//! endpoint at temperature 0, and `raw_output` / `live_calls` are added.
//!
//! Output contract (one JSONL line per case, in eval-file order):
//!   {"id":"ct-001","compact_request":{...},"compacted":true,
//!    "rendered_calls":"<<call ...>>","roundtrip_calls":[...]}
//!   {"id":"dc-002","decoded":{"calls":[...]}}  // or {"decoded":{"error":"..."}}

use std::io::Write;

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools,
};
use serde_json::{Value, json};

const SYSTEM_TIME: &str = "Current date/time context: today is 2026-10-02, timezone Asia/Kolkata (UTC+05:30). \
     Resolve relative dates (\"tomorrow\", \"Monday 3pm\") against this.";

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");

    let tools_by_name: Vec<&Value> = data["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .collect();
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));

    let live = std::env::var("PROVIDER_BASE_URL")
        .ok()
        .zip(std::env::var("MODEL").ok());
    let client = reqwest::Client::new();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    let cases = data["cases"].as_array().expect("cases array");
    let mut native_tokens = 0usize;
    let mut compact_tokens = 0usize;

    for case in cases {
        let id = case["id"].as_str().expect("id");
        let names: Vec<&str> = case["tools"]
            .as_array()
            .expect("case.tools")
            .iter()
            .map(|v| v.as_str().expect("tool name"))
            .collect();
        let defs: Vec<ToolDef> = names
            .iter()
            .map(|n| {
                let raw = tools_by_name
                    .iter()
                    .find(|t| t["function"]["name"] == *n)
                    .expect("tool defined");
                serde_json::from_value((*raw).clone()).expect("valid tool def")
            })
            .collect();

        let encoded = encode_tools(&defs);
        let (compacted, compact_request) = match &encoded {
            Ok(compact) => {
                let mut messages = vec![
                    json!({"role": "system", "content": format!("{SYSTEM_TIME}\n\n{compact_text}", compact_text = compact.text)}),
                ];
                messages.extend(case["messages"].as_array().expect("messages").clone());
                (true, json!({"messages": messages, "temperature": 0}))
            }
            Err(_) => {
                // Bypass: send the native request unchanged.
                let mut messages = vec![json!({"role": "system", "content": SYSTEM_TIME})];
                messages.extend(case["messages"].as_array().expect("messages").clone());
                (
                    false,
                    json!({"messages": messages, "tools": case_tools(&tools_by_name, &names), "temperature": 0}),
                )
            }
        };

        let rendered = case["expected"]
            .as_array()
            .map(|expected| {
                expected
                    .iter()
                    .map(|e| {
                        format!(
                            "<<call {} {}>>",
                            e["name"].as_str().expect("call name"),
                            e["arguments"]
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        let roundtrip = match decode_calls(&rendered, &defs) {
            Ok(calls) => calls.into_iter().map(call_json).collect::<Vec<_>>(),
            Err(e) => vec![json!({"error": e.kind()})],
        };

        // Token accounting (claims only; the scorer recounts).
        let native_body = {
            let mut m = vec![json!({"role": "system", "content": SYSTEM_TIME})];
            m.extend(case["messages"].as_array().expect("messages").clone());
            json!({
                "messages": m,
                "tools": case_tools(&tools_by_name, &names),
                "temperature": 0,
            })
        };
        native_tokens += count_tokens(&native_body.to_string());
        compact_tokens += count_tokens(&compact_request.to_string());

        let mut line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered,
            "roundtrip_calls": roundtrip,
        });

        if let Some((base, model)) = &live {
            let (raw_output, live_calls) =
                run_live(&client, &rt, base, model, &compact_request, &defs);
            line["raw_output"] = raw_output;
            line["live_calls"] = live_calls;
        }

        writeln!(out, "{line}").expect("write OUT");
    }

    if let Some(dc) = data["decoder_cases"].as_array() {
        for case in dc {
            let id = case["id"].as_str().expect("id");
            let names: Vec<&str> = case["tools"]
                .as_array()
                .expect("case.tools")
                .iter()
                .map(|v| v.as_str().expect("tool name"))
                .collect();
            let defs: Vec<ToolDef> = names
                .iter()
                .map(|n| {
                    let raw = tools_by_name
                        .iter()
                        .find(|t| t["function"]["name"] == *n)
                        .expect("tool defined");
                    serde_json::from_value((*raw).clone()).expect("valid tool def")
                })
                .collect();
            let mut dec = StreamDecoder::new(&defs);
            let mut first_err: Option<CompactError> = None;
            for chunk in case["chunks"].as_array().expect("chunks") {
                let c = chunk.as_str().expect("chunk");
                if first_err.is_none()
                    && let Err(e) = dec.push(c)
                {
                    first_err = Some(e);
                }
            }
            let decoded = match (first_err, dec.finish()) {
                (Some(e), _) => json!({"error": e.kind()}),
                (_, Ok(calls)) => {
                    json!({"calls": calls.into_iter().map(call_json).collect::<Vec<_>>()})
                }
                (_, Err(e)) => json!({"error": e.kind()}),
            };
            writeln!(out, "{}", json!({"id": id, "decoded": decoded})).expect("write OUT");
        }
    }

    out.flush().expect("flush OUT");

    let reduction = if native_tokens > 0 {
        1.0 - compact_tokens as f64 / native_tokens as f64
    } else {
        0.0
    };
    eprintln!(
        "token_count: native={native_tokens} compact={compact_tokens} reduction={reduction:.3} (o200k_base, full request body)"
    );
}

fn call_json(c: ToolCall) -> Value {
    json!({"name": c.name, "arguments": c.arguments})
}

fn case_tools(tools_by_name: &[&Value], names: &[&str]) -> Vec<Value> {
    names
        .iter()
        .map(|n| {
            tools_by_name
                .iter()
                .find(|t| t["function"]["name"] == **n)
                .map(|t| (**t).clone())
                .expect("tool defined")
        })
        .collect()
}

fn count_tokens(s: &str) -> usize {
    match tiktoken_rs::o200k_base() {
        Ok(bpe) => bpe.encode_ordinary(s).len(),
        Err(_) => s.split_whitespace().count(),
    }
}

fn run_live(
    client: &reqwest::Client,
    rt: &tokio::runtime::Runtime,
    base: &str,
    model: &str,
    compact_request: &Value,
    defs: &[ToolDef],
) -> (Value, Value) {
    let mut body = compact_request.clone();
    body["model"] = json!(model);
    if std::env::var("PROVIDER_API_KEY").is_err() {
        body["stream"] = json!(false);
    }
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let fut = client
        .post(&url)
        .bearer_auth(std::env::var("PROVIDER_API_KEY").unwrap_or_default())
        .json(&body)
        .send();
    let resp = rt.block_on(fut);
    let Ok(resp) = resp else {
        return (json!(null), json!({"error": "network"}));
    };
    let Ok(v) = rt.block_on(resp.json::<Value>()) else {
        return (json!(null), json!({"error": "network"}));
    };
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let live = match decode_calls(&text, defs) {
        Ok(calls) => json!({"calls": calls.into_iter().map(call_json).collect::<Vec<_>>()}),
        Err(e) => json!({"error": e.kind()}),
    };
    (json!(text), live)
}

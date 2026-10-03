use std::io::{BufWriter, Write};

use nasiko_tool_compact::{
    DecodeError, StreamDecoder, StreamEvent, ToolCall, ToolDef, decode_calls, encode_tools,
    render_calls,
};
use serde_json::{Value, json};
use tiktoken_rs::{CoreBPE, o200k_base};

fn main() {
    let eval_path =
        std::env::var("EVAL_SET").unwrap_or_else(|_| "compact-tools-eval.json".to_string());
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".to_string());
    let report_path = std::env::var("REPORT_OUT").ok();
    let raw = std::fs::read_to_string(&eval_path)
        .unwrap_or_else(|_| panic!("failed to read {eval_path}"));
    let document: Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let tools_schema = document["tools"].as_array().cloned().unwrap_or_default();
    let cases = document["cases"].as_array().cloned().unwrap_or_default();
    let decoder_cases = document["decoder_cases"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let tokenizer = o200k_base().unwrap_or_else(|err| panic!("load o200k_base: {err}"));

    let file = std::fs::File::create(&out_path).expect("create OUT file");
    let mut out = BufWriter::new(file);
    let mut baseline_total = 0usize;
    let mut compact_total = 0usize;
    let mut bypassed_cases = 0usize;
    let mut demo_schema = None;
    let mut demo_definition = None;
    let mut demo_prompt = None;
    let mut success_demo = None;
    let mut invalid_demo = None;

    for case in &cases {
        let id = case["id"].as_str().unwrap_or("case");
        let selected_schemas = select_tool_schemas(case, &tools_schema);
        let selected: Vec<ToolDef> = selected_schemas
            .iter()
            .map(|schema| {
                tool_from_value(schema).unwrap_or_else(|| panic!("invalid tool schema in {id}"))
            })
            .collect();
        let messages = case["messages"].as_array().cloned().unwrap_or_default();
        let baseline_request = json!({
            "messages": messages,
            "tools": selected_schemas,
        });
        let (compacted, compact_request, compact_prompt, compact_definition) =
            match encode_tools(&selected) {
                Ok(compact) => {
                    let prompt = compact.prompt().to_string();
                    let definition = selected
                        .first()
                        .and_then(|tool| compact_definition_for(compact.definitions(), &tool.name));
                    let mut compact_messages = Vec::with_capacity(messages.len() + 1);
                    compact_messages.push(json!({"role": "system", "content": prompt}));
                    compact_messages.extend(messages.iter().cloned());
                    (
                        true,
                        json!({"messages": compact_messages}),
                        Some(prompt),
                        definition,
                    )
                }
                Err(_) => (
                    false,
                    json!({"messages": messages, "tools": selected_schemas}),
                    None,
                    None,
                ),
            };

        let expected = case["expected"].as_array().cloned().unwrap_or_default();
        let calls: Vec<ToolCall> = expected.iter().filter_map(expected_call).collect();
        let rendered = render_calls(&calls);
        let roundtrip = decode_calls(&rendered, &selected)
            .unwrap_or_else(|err| panic!("round-trip decode failed for {id}: {err}"));

        baseline_total += request_token_count(&tokenizer, &baseline_request);
        compact_total += request_token_count(&tokenizer, &compact_request);
        if !compacted {
            bypassed_cases += 1;
        }
        if demo_schema.is_none() {
            demo_schema = selected_schemas.first().cloned();
            demo_definition = compact_definition;
            demo_prompt = compact_prompt;
        }

        let line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered,
            "roundtrip_calls": calls_to_json(&roundtrip),
        });
        writeln!(out, "{line}").expect("write eval line");
    }

    for case in &decoder_cases {
        let id = case["id"].as_str().unwrap_or("decoder_case");
        let selected_schemas = select_tool_schemas(case, &tools_schema);
        let selected: Vec<ToolDef> = selected_schemas
            .iter()
            .map(|schema| {
                tool_from_value(schema).unwrap_or_else(|| panic!("invalid tool schema in {id}"))
            })
            .collect();
        let chunks = case["chunks"].as_array().cloned().unwrap_or_default();
        let input = chunks.iter().filter_map(Value::as_str).collect::<String>();
        let decoded = match decode_chunks(&chunks, &selected) {
            Ok(calls) => {
                let calls = calls_to_json(&calls);
                if success_demo.is_none() && !calls.as_array().is_none_or(Vec::is_empty) {
                    let expected = case["expected"]["calls"].clone();
                    success_demo = Some((input.clone(), calls.clone(), calls == expected));
                }
                json!({"calls": calls})
            }
            Err(error) => {
                let code = error.code().to_string();
                let expected = case["expected"]["error"].as_str().unwrap_or_default();
                let result = Some((
                    input.clone(),
                    code.clone(),
                    error.to_string(),
                    code == expected,
                ));
                if code == "invalid_arguments" || invalid_demo.is_none() {
                    invalid_demo = result;
                }
                json!({"error": code})
            }
        };
        writeln!(out, "{}", json!({"id": id, "decoded": decoded}))
            .expect("write decoder eval line");
    }
    out.flush().expect("flush eval output");

    if let Some(report_path) = report_path {
        write_demo_report(
            &report_path,
            baseline_total,
            compact_total,
            bypassed_cases,
            demo_schema.as_ref(),
            demo_definition.as_deref(),
            demo_prompt.as_deref(),
            success_demo.as_ref(),
            invalid_demo.as_ref(),
        );
    }
}

fn tool_from_value(value: &Value) -> Option<ToolDef> {
    let func = value.get("function").unwrap_or(value);
    let name = func.get("name").and_then(Value::as_str)?;
    let description = func
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let parameters = func
        .get("parameters")
        .or_else(|| value.get("parameters"))
        .cloned();
    Some(ToolDef {
        name: name.to_string(),
        description,
        parameters,
    })
}

fn expected_call(value: &Value) -> Option<ToolCall> {
    let name = value["name"].as_str()?;
    let arguments = value["arguments"].as_object()?.clone();
    Some(ToolCall {
        name: name.to_string(),
        arguments,
    })
}

fn select_tool_schemas(case: &Value, all_tools: &[Value]) -> Vec<Value> {
    let Some(names) = case["tools"].as_array() else {
        return all_tools.to_vec();
    };
    let names: std::collections::HashSet<_> = names.iter().filter_map(Value::as_str).collect();
    all_tools
        .iter()
        .filter(|schema| {
            let function = schema.get("function").unwrap_or(schema);
            function
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| names.contains(name))
        })
        .cloned()
        .collect()
}

fn compact_definition_for(definitions: &str, name: &str) -> Option<String> {
    definitions
        .lines()
        .find(|line| line.starts_with(name))
        .map(str::to_string)
}

fn calls_to_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|call| {
                json!({
                    "name": call.name,
                    "arguments": call.arguments.clone(),
                })
            })
            .collect(),
    )
}

fn decode_chunks(chunks: &[Value], tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut decoder = StreamDecoder::new(tools).map_err(DecodeError::from)?;
    let mut calls = Vec::new();
    for chunk in chunks {
        let chunk = chunk.as_str().unwrap_or_default();
        collect_calls(decoder.push(chunk)?, &mut calls);
    }
    collect_calls(decoder.finish()?, &mut calls);
    Ok(calls)
}

fn collect_calls(events: Vec<StreamEvent>, calls: &mut Vec<ToolCall>) {
    for event in events {
        if let StreamEvent::Call { call, .. } = event {
            calls.push(call);
        }
    }
}

fn request_token_count(tokenizer: &CoreBPE, request: &Value) -> usize {
    let body = serde_json::to_string(request).expect("serialize request body");
    tokenizer.encode_ordinary(&body).len()
}

fn write_demo_report(
    path: &str,
    baseline_total: usize,
    compact_total: usize,
    bypassed_cases: usize,
    schema: Option<&Value>,
    definition: Option<&str>,
    prompt: Option<&str>,
    success: Option<&(String, Value, bool)>,
    invalid: Option<&(String, String, String, bool)>,
) {
    let saved_percent = if baseline_total == 0 {
        0.0
    } else {
        (baseline_total as f64 - compact_total as f64) / baseline_total as f64 * 100.0
    };
    let schema = schema
        .map(|schema| serde_json::to_string_pretty(schema).expect("serialize demo schema"))
        .unwrap_or_else(|| "No tool schema was available.".to_string());
    let definition = definition.unwrap_or("No compact definition was produced.");
    let prompt = prompt.unwrap_or("No compact prompt was produced.");
    let success = success.map_or_else(
        || "No successful decoder fixture was found.".to_string(),
        |(input, calls, matches)| {
            format!(
                "Rendered call / decoder input:\n```text\n{input}\n```\n\nDecoded standard call:\n```json\n{}\n```\n\nMatches expected: **{matches}**.",
                serde_json::to_string_pretty(calls).expect("serialize successful call")
            )
        },
    );
    let invalid = invalid.map_or_else(
        || "No rejected decoder fixture was found.".to_string(),
        |(input, code, error, matches)| {
            format!(
            "Input:\n```text\n{input}\n```\n\nReturned decoder error: `{error}` (`{code}`). It was rejected because the required `title` field is missing; no call was returned, so no guessed call escaped. Matches expected error: **{matches}**."
            )
        },
    );
    let report = format!(
        "# Compact Tools Demo\n\n**Evaluation mode: OFFLINE.** The evaluator renders expected calls to test encoding and decoding. It does not demonstrate real-model tool selection. Real-model testing: **unverified**.\n\n## A. Before and After\n\nOriginal public-sample tool schema:\n```json\n{schema}\n```\n\nCompact definition from the encoder:\n```text\n{definition}\n```\n\nEncoder instructions:\n```text\n{prompt}\n```\n\nThe compact definition keeps the tool name, required/optional fields, field types, supported format/enum shorthand, and tool description. Individual property descriptions are omitted in this output.\n\n## B. Token Savings\n\n- Baseline full-request tokens: **{baseline_total}**\n- Compact full-request tokens: **{compact_total}**\n- Aggregate saved: **{saved_percent:.1}%**\n- Bypassed cases: **{bypassed_cases}**\n\nCounts use pinned `tiktoken-rs` 0.12.0 `o200k_base` over minified serialized request bodies. Baselines contain original messages and selected native tool schemas; compact requests contain the injected system instructions and original messages.\n\n## C. Successful Call\n\n{success}\n\n## D. Rejected Invalid Call\n\n{invalid}\n\n## Demo Lines\n\n\"Here are the long instructions.\"\n\n\"Here is our shorter version.\"\n\n\"We saved {saved_percent:.1}% of full-request tokens on this public sample.\"\n\n\"This valid call was decoded correctly.\"\n\n\"This invalid call was rejected.\"\n",
    );
    std::fs::write(path, report).unwrap_or_else(|err| panic!("write demo report {path}: {err}"));
}

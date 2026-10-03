use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use nasiko_tool_compact::{StreamDecoder, ToolCompactError, ToolDef, decode_calls, encode_tools};
use serde::Deserialize;
use serde_json::{Value, json};

const EMBEDDED_SAMPLE_JSON: &str = r#"{
  "schema_version": "compact-tools-eval@v1-sample",
  "purpose": "Public sample cases for compact tools token reduction & parsing evaluation",
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "create_calendar_event",
        "description": "Create an event in the user's calendar.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": {"type": "string", "description": "Event title"},
            "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
            "duration_min": {"type": "integer", "description": "Duration in minutes"},
            "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
            "visibility": {"type": "string", "enum": ["public", "private"]}
          },
          "required": ["title", "start"]
        }
      }
    },
    {
      "type": "function",
      "function": {
        "name": "send_email",
        "description": "Send an email to recipients.",
        "parameters": {
          "type": "object",
          "properties": {
            "to": {"type": "array", "items": {"type": "string"}},
            "subject": {"type": "string"},
            "body": {"type": "string"}
          },
          "required": ["to", "subject", "body"]
        }
      }
    }
  ],
  "cases": [
    {
      "id": "ct-001",
      "tools": ["create_calendar_event", "send_email"],
      "messages": [{"role": "user", "content": "Book a design review Monday 3pm IST with riya@example.com"}],
      "expected": [{"name": "create_calendar_event", "arguments": {"title": "Design review", "start": "2026-10-05T15:00:00+05:30", "attendees": ["riya@example.com"]}}]
    },
    {
      "id": "ct-002",
      "tools": ["create_calendar_event", "send_email"],
      "messages": [{"role": "user", "content": "Email sam@example.com that the build is green, and add a private 30 min retro tomorrow 10am IST"}],
      "expected": [
        {"name": "send_email", "arguments": {"to": ["sam@example.com"], "subject": "Build status", "body": "The build is green."}},
        {"name": "create_calendar_event", "arguments": {"title": "Retro", "start": "2026-10-04T10:00:00+05:30", "duration_min": 30, "visibility": "private"}}
      ],
      "match": {"free_text_fields": ["subject", "body", "title"]}
    },
    {
      "id": "ct-003",
      "tools": ["create_calendar_event"],
      "messages": [{"role": "user", "content": "What's the weather?"}],
      "expected": []
    }
  ],
  "decoder_cases": [
    {
      "id": "dc-001",
      "note": "single chunk tool call",
      "tools": ["create_calendar_event"],
      "chunks": ["<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\",\"attendees\":[\"riya@example.com\"]}>>"],
      "expected": {"calls": [{"name": "create_calendar_event", "arguments": {"title": "Design review", "start": "2026-10-05T15:00:00+05:30", "attendees": ["riya@example.com"]}}]}
    },
    {
      "id": "dc-002",
      "note": "marker split across stream chunks",
      "tools": ["create_calendar_event"],
      "chunks": ["<<ca", "ll create_calendar_event {\"title\":\"Ret", "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>", ">"],
      "expected": {"calls": [{"name": "create_calendar_event", "arguments": {"title": "Retro", "start": "2026-10-04T10:00:00+05:30"}}]}
    },
    {
      "id": "dc-003",
      "note": "unknown tool call error",
      "tools": ["create_calendar_event"],
      "chunks": ["<<call unknown_tool {\"param\":\"val\"}>>"],
      "expected": {"error": "unknown_tool"}
    },
    {
      "id": "dc-004",
      "note": "invalid arguments missing required field",
      "tools": ["create_calendar_event"],
      "chunks": ["<<call create_calendar_event {\"title\":\"Design review\"}>>"],
      "expected": {"error": "invalid_arguments"}
    }
  ]
}"#;

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    expected: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct EvalSet {
    #[serde(default)]
    #[allow(dead_code)]
    schema_version: Option<String>,
    #[serde(default)]
    tools: Value,
    #[serde(default)]
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

fn sort_json_keys(val: &Value) -> Value {
    match val {
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), sort_json_keys(v));
            }
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json_keys).collect()),
        other => other.clone(),
    }
}

fn parse_tool_registry(tools_val: &Value) -> HashMap<String, ToolDef> {
    let mut map = HashMap::new();
    match tools_val {
        Value::Array(arr) => {
            for item in arr {
                if let Ok(td) = serde_json::from_value::<ToolDef>(item.clone()) {
                    map.insert(td.function.name.clone(), td);
                } else if let Some(name) = item
                    .as_object()
                    .and_then(|o| o.get("name"))
                    .and_then(Value::as_str)
                {
                    let obj = item.as_object().unwrap();
                    let desc = obj
                        .get("description")
                        .and_then(Value::as_str)
                        .map(String::from);
                    let params = obj.get("parameters").cloned();
                    map.insert(name.to_string(), ToolDef::new(name, desc, params));
                }
            }
        }
        Value::Object(obj) => {
            for (name, val) in obj {
                if let Ok(td) = serde_json::from_value::<ToolDef>(val.clone()) {
                    map.insert(name.clone(), td);
                } else if let Some(v_obj) = val.as_object() {
                    let desc = v_obj
                        .get("description")
                        .and_then(Value::as_str)
                        .map(String::from);
                    let params = v_obj.get("parameters").cloned().or_else(|| {
                        if v_obj.contains_key("properties") || v_obj.contains_key("type") {
                            Some(val.clone())
                        } else {
                            None
                        }
                    });
                    map.insert(name.clone(), ToolDef::new(name, desc, params));
                }
            }
        }
        _ => {}
    }
    map
}

fn main() {
    // 1. Read input dataset from EVAL_SET env var or fallback to embedded fixture
    let eval_content = match std::env::var("EVAL_SET") {
        Ok(path) if !path.trim().is_empty() => match std::fs::read_to_string(&path) {
            Ok(content) => {
                eprintln!("Loaded evaluation set from EVAL_SET={path}");
                content
            }
            Err(err) => {
                eprintln!(
                    "Failed to read EVAL_SET={path}: {err}. Falling back to standard embedded sample fixture."
                );
                EMBEDDED_SAMPLE_JSON.to_string()
            }
        },
        _ => {
            eprintln!("EVAL_SET env var not set. Using standard embedded sample fixture.");
            EMBEDDED_SAMPLE_JSON.to_string()
        }
    };

    let eval_set: EvalSet =
        serde_json::from_str(&eval_content).expect("Valid evaluation set JSON format required");

    let tool_registry = parse_tool_registry(&eval_set.tools);

    // 2. Setup output sink (OUT env var or /tmp/out.jsonl or stdout)
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());
    let mut writer: Box<dyn Write> = match File::create(Path::new(&out_path)) {
        Ok(f) => {
            eprintln!("Writing deterministic evaluation output to: {out_path}");
            Box::new(BufWriter::new(f))
        }
        Err(err) => {
            eprintln!("Could not create output file at {out_path}: {err}. Falling back to stdout.");
            Box::new(BufWriter::new(io::stdout()))
        }
    };

    let model_name = std::env::var("MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let provider_base_url = std::env::var("PROVIDER_BASE_URL").ok();

    // 3. Initialize tokenizer for token savings measurement
    let bpe = tiktoken_rs::o200k_base().ok();
    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;
    let mut regular_cases_count = 0usize;

    // 4. Process regular cases (`ct-*`)
    for case in &eval_set.cases {
        let mut selected_tools = Vec::new();
        for tname in &case.tools {
            if let Some(tool) = tool_registry.get(tname) {
                selected_tools.push(tool.clone());
            }
        }

        // Try compact encoding
        let (compact_req, compacted) = match encode_tools(&selected_tools) {
            Ok(compact) => {
                let fixed_time_header = "Today is 2026-10-02, timezone Asia/Kolkata.";
                let system_prompt = format!(
                    "{fixed_time_header}\n\n{}\n\n{}",
                    compact.signatures, compact.instruction
                );

                let mut messages = Vec::new();
                messages.push(json!({
                    "role": "system",
                    "content": system_prompt
                }));
                for m in &case.messages {
                    messages.push(m.clone());
                }

                let mut req = BTreeMap::new();
                req.insert("messages".to_string(), Value::Array(messages));
                req.insert("model".to_string(), json!(model_name));
                req.insert("temperature".to_string(), json!(0.0));

                (req, true)
            }
            Err(_) => {
                // Compaction bypassed: fallback to native request
                let mut req = BTreeMap::new();
                req.insert("messages".to_string(), Value::Array(case.messages.clone()));
                req.insert("model".to_string(), json!(model_name));
                req.insert("tools".to_string(), json!(selected_tools));
                (req, false)
            }
        };

        // Render expected calls in compact format
        let mut rendered_lines = Vec::new();
        for exp in &case.expected {
            if let Some(name) = exp.get("name").and_then(Value::as_str) {
                let args_val = exp.get("arguments").cloned().unwrap_or(json!({}));
                let sorted_args = sort_json_keys(&args_val);
                let args_json =
                    serde_json::to_string(&sorted_args).unwrap_or_else(|_| "{}".to_string());
                rendered_lines.push(format!("<<call {name} {args_json}>>"));
            }
        }
        let rendered_calls = rendered_lines.join("\n");

        // Decode rendered calls back for roundtrip verification
        let roundtrip_calls = if rendered_calls.trim().is_empty() {
            Vec::new()
        } else {
            decode_calls(&rendered_calls, &selected_tools).unwrap_or_default()
        };

        // Measure tokens with o200k_base
        if let Some(ref tokenizer) = bpe {
            let mut baseline_req = BTreeMap::new();
            baseline_req.insert("messages".to_string(), Value::Array(case.messages.clone()));
            baseline_req.insert("model".to_string(), json!(model_name));
            baseline_req.insert("tools".to_string(), json!(selected_tools));

            let baseline_str = serde_json::to_string(&baseline_req).unwrap_or_default();
            let compact_str = serde_json::to_string(&compact_req).unwrap_or_default();

            let base_tokens = tokenizer.encode_with_special_tokens(&baseline_str).len();
            let comp_tokens = tokenizer.encode_with_special_tokens(&compact_str).len();

            total_baseline_tokens += base_tokens;
            total_compact_tokens += comp_tokens;
            regular_cases_count += 1;
        }

        // Construct 100% deterministic output line
        let mut out_line = BTreeMap::new();
        out_line.insert("id".to_string(), json!(case.id));
        out_line.insert(
            "compact_request".to_string(),
            sort_json_keys(&Value::Object(compact_req.into_iter().collect())),
        );
        out_line.insert("compacted".to_string(), json!(compacted));
        out_line.insert("rendered_calls".to_string(), json!(rendered_calls));
        out_line.insert(
            "roundtrip_calls".to_string(),
            sort_json_keys(&json!(roundtrip_calls)),
        );

        // Live mode optional execution
        if let Some(ref base_url) = provider_base_url {
            let (raw_output, live_calls) = execute_live_call(
                base_url,
                &model_name,
                &out_line["compact_request"],
                &selected_tools,
            );
            out_line.insert("raw_output".to_string(), json!(raw_output));
            out_line.insert("live_calls".to_string(), live_calls);
        }

        let line_json = serde_json::to_string(&out_line).expect("JSON serialization succeeds");
        writeln!(writer, "{line_json}").expect("Writing JSONL line succeeds");
    }

    // 5. Process decoder cases (`dc-*`)
    for dcase in &eval_set.decoder_cases {
        let mut selected_tools = Vec::new();
        for tname in &dcase.tools {
            if let Some(tool) = tool_registry.get(tname) {
                selected_tools.push(tool.clone());
            }
        }

        let mut decoder = StreamDecoder::new(selected_tools);
        let mut emitted_calls = Vec::new();
        let mut decode_error = None;

        for chunk in &dcase.chunks {
            match decoder.push_chunk(chunk) {
                Ok(calls) => emitted_calls.extend(calls),
                Err(err) => {
                    decode_error = Some(err);
                    break;
                }
            }
        }

        if decode_error.is_none() {
            match decoder.finish() {
                Ok(remaining) => emitted_calls.extend(remaining),
                Err(err) => decode_error = Some(err),
            }
        }

        let mut decoded_val = BTreeMap::new();
        if let Some(err) = decode_error {
            let error_code = match err {
                ToolCompactError::UnknownTool(_) => "unknown_tool",
                ToolCompactError::InvalidArguments(_) => "invalid_arguments",
                ToolCompactError::ParseError(_) => "parse_error",
                ToolCompactError::UnsupportedSchema(_) => "unsupported_schema",
            };
            decoded_val.insert("error".to_string(), json!(error_code));
        } else {
            decoded_val.insert("calls".to_string(), sort_json_keys(&json!(emitted_calls)));
        }

        let mut out_line = BTreeMap::new();
        out_line.insert("id".to_string(), json!(dcase.id));
        out_line.insert(
            "decoded".to_string(),
            Value::Object(decoded_val.into_iter().collect()),
        );

        let line_json = serde_json::to_string(&out_line).expect("JSON serialization succeeds");
        writeln!(writer, "{line_json}").expect("Writing JSONL line succeeds");
    }

    writer.flush().expect("Flushing output succeeds");

    // 6. Print Token Savings Report
    if regular_cases_count > 0 && total_baseline_tokens > 0 {
        let tokens_saved = total_baseline_tokens.saturating_sub(total_compact_tokens);
        let pct = (tokens_saved as f64 / total_baseline_tokens as f64) * 100.0;
        let avg_saved = tokens_saved as f64 / regular_cases_count as f64;

        eprintln!("\n{}", "=".repeat(78));
        eprintln!("              Compact Tools Token Savings Report (o200k_base)");
        eprintln!("{}", "=".repeat(78));
        eprintln!("Total Evaluated Cases:   {}", regular_cases_count);
        eprintln!("Baseline Native Tokens:  {}", total_baseline_tokens);
        eprintln!("Compact Format Tokens:   {}", total_compact_tokens);
        eprintln!("Tokens Saved:            {} ({:.2}%)", tokens_saved, pct);
        eprintln!("Average Savings / Case:  {:.1} tokens", avg_saved);
        eprintln!("{}\n", "=".repeat(78));
    }
}

fn execute_live_call(
    base_url: &str,
    model: &str,
    request_body: &Value,
    tools: &[ToolDef],
) -> (String, Value) {
    let endpoint = if base_url.ends_with("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{}/chat/completions", base_url.trim_end_matches('/'))
    };

    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "live-eval".to_string());

    let mut body = request_body.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".to_string(), json!(model));
        obj.insert("temperature".to_string(), json!(0.0));
    }

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return (e.to_string(), json!({"error": "runtime_failure"})),
    };

    let client = reqwest::Client::new();
    let res = rt.block_on(async {
        client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .json::<Value>()
            .await
    });

    match res {
        Ok(resp_json) => {
            let raw_text = resp_json
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            match decode_calls(&raw_text, tools) {
                Ok(calls) => (raw_text, json!({"calls": calls})),
                Err(err) => {
                    let err_str = match err {
                        ToolCompactError::UnknownTool(_) => "unknown_tool",
                        ToolCompactError::InvalidArguments(_) => "invalid_arguments",
                        ToolCompactError::ParseError(_) => "parse_error",
                        ToolCompactError::UnsupportedSchema(_) => "unsupported_schema",
                    };
                    (raw_text, json!({"error": err_str}))
                }
            }
        }
        Err(err) => (err.to_string(), json!({"error": "network_failure"})),
    }
}

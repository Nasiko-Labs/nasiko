//! Deterministic compact-tool evaluation runner.
//!
//! Offline mode is the default. Set `PROVIDER_BASE_URL` and `MODEL` to additionally
//! send each compact request to an OpenAI-compatible `/chat/completions` endpoint.

use std::{collections::BTreeMap, env, fs, io::Write};

use nasiko_tool_compact::{
    CompactTools, Error as CompactError, FunctionDef, StreamDecoder, ToolCall, ToolDef,
    decode_calls, encode_tools, render_calls,
};
use serde_json::{Map, Value, json};

const REFERENCE_TIME: &str = "Today is 2026-10-02. Timezone: Asia/Kolkata.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".into());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/compact-tools-out.jsonl".into());
    let dataset: Value = serde_json::from_slice(
        &fs::read(&eval_path)
            .map_err(|error| format!("cannot read EVAL_SET at {eval_path}: {error}"))?,
    )?;
    let tool_index = load_tools(&dataset)?;
    let live = LiveConfig::from_env();
    let measure_tokens = env::var("MEASURE_TOKENS").ok().as_deref() == Some("1");
    let tokenizer = measure_tokens.then(tiktoken_rs::o200k_base).transpose()?;
    let mut token_totals = TokenTotals::default();
    let mut out = fs::File::create(&out_path)?;

    for case in dataset
        .get("cases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let selected = case_tools(case, &tool_index)?;
        let compact = match encode_tools(&selected) {
            Ok(compact) => compact,
            // The library deliberately rejects schema features it cannot preserve. The
            // evaluator mirrors router behavior: retain native tools and report a bypass
            // instead of failing the entire evaluation run.
            Err(CompactError::UnsupportedSchema { .. }) => {
                write_row(
                    &mut out,
                    &json!({
                        "id": case_id(case)?,
                        "compact_request": native_request(case, &selected),
                        "compacted": false,
                        "rendered_calls": "",
                        "roundtrip_calls": [],
                    }),
                )?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let expected = expected_calls(case)?;
        let rendered_calls = render_calls(&expected)?;
        let roundtrip_calls = decode_calls(&rendered_calls, &selected)?;
        let request = compact_request(case, &compact);
        if let Some(tokenizer) = &tokenizer {
            token_totals.add(
                tokenizer
                    .encode_with_special_tokens(&serde_json::to_string(&native_request(
                        case, &selected,
                    ))?)
                    .len(),
                tokenizer
                    .encode_with_special_tokens(&serde_json::to_string(&request)?)
                    .len(),
            );
        }
        let mut row = json!({
            "id": case_id(case)?,
            "compact_request": request,
            "compacted": true,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        });
        if let Some(live) = &live {
            add_live_result(&mut row, live, request, &selected).await;
        }
        write_row(&mut out, &row)?;
    }

    for decoder_case in dataset
        .get("decoder_cases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let selected = case_tools(decoder_case, &tool_index)?;
        let decoded = decode_chunks(decoder_case, &selected);
        write_row(
            &mut out,
            &json!({"id": case_id(decoder_case)?, "decoded": decoded}),
        )?;
    }
    if measure_tokens {
        eprintln!(
            "compact-tools token measurement (o200k_base): native={}, compact={}, reduction={:.1}%",
            token_totals.native,
            token_totals.compact,
            token_totals.reduction_percent()
        );
    }
    Ok(())
}

fn load_tools(dataset: &Value) -> Result<BTreeMap<String, ToolDef>, Box<dyn std::error::Error>> {
    let tools = dataset
        .get("tools")
        .ok_or("evaluation set is missing tools")?;
    let definitions: Vec<ToolDef> = match tools {
        Value::Array(_) => serde_json::from_value(tools.clone())?,
        Value::Object(by_name) => by_name
            .iter()
            .map(|(name, definition)| {
                if definition.get("function").is_some() {
                    serde_json::from_value(definition.clone())
                } else {
                    Ok(ToolDef {
                        kind: "function".into(),
                        function: FunctionDef {
                            name: name.clone(),
                            description: definition
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            parameters: definition
                                .get("parameters")
                                .cloned()
                                .or_else(|| definition.get("input_schema").cloned()),
                        },
                        extra: Map::new(),
                    })
                }
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()?,
        _ => return Err("evaluation set tools must be an array or object".into()),
    };
    Ok(definitions
        .into_iter()
        .map(|tool| (tool.function.name.clone(), tool))
        .collect())
}

fn case_tools(
    case: &Value,
    index: &BTreeMap<String, ToolDef>,
) -> Result<Vec<ToolDef>, Box<dyn std::error::Error>> {
    case.get("tools")
        .and_then(Value::as_array)
        .ok_or("case is missing tools")?
        .iter()
        .map(|name| {
            let name = name.as_str().ok_or("tool name must be a string")?;
            index
                .get(name)
                .cloned()
                .ok_or_else(|| format!("case references unknown tool {name}").into())
        })
        .collect()
}

fn expected_calls(case: &Value) -> Result<Vec<ToolCall>, Box<dyn std::error::Error>> {
    case.get("expected")
        .and_then(Value::as_array)
        .ok_or("case is missing expected calls")?
        .iter()
        .map(|call| {
            Ok(ToolCall {
                name: call
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("expected call is missing name")?
                    .to_string(),
                arguments: call
                    .get("arguments")
                    .cloned()
                    .ok_or("expected call is missing arguments")?,
            })
        })
        .collect()
}

fn compact_request(case: &Value, compact: &CompactTools) -> Value {
    let mut messages = vec![json!({
        "role": "system",
        "content": format!("{REFERENCE_TIME}\n{}", compact.prompt),
    })];
    messages.extend(
        case.get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    json!({"messages": messages})
}

fn native_request(case: &Value, tools: &[ToolDef]) -> Value {
    json!({
        "messages": case.get("messages").cloned().unwrap_or_else(|| json!([])),
        "tools": tools,
    })
}

fn decode_chunks(case: &Value, tools: &[ToolDef]) -> Value {
    let result = (|| -> Result<Vec<ToolCall>, CompactError> {
        let chunks = case
            .get("chunks")
            .and_then(Value::as_array)
            .ok_or_else(|| CompactError::MalformedCall("decoder case is missing chunks".into()))?;
        let mut decoder = StreamDecoder::new(tools)?;
        let mut calls = Vec::new();
        for chunk in chunks {
            let chunk = chunk.as_str().ok_or_else(|| {
                CompactError::MalformedCall("decoder chunk must be a string".into())
            })?;
            calls.extend(decoder.push(chunk)?);
        }
        calls.extend(decoder.finish()?);
        Ok(calls)
    })();
    match result {
        Ok(calls) => json!({"calls": calls}),
        Err(error) => json!({"error": error_code(&error)}),
    }
}

fn error_code(error: &CompactError) -> &'static str {
    match error {
        CompactError::UnknownTool(_) => "unknown_tool",
        CompactError::InvalidArguments { .. }
        | CompactError::InvalidJson(_)
        | CompactError::MalformedCall(_)
        | CompactError::UnsupportedSchema { .. } => "invalid_arguments",
    }
}

#[derive(Default)]
struct TokenTotals {
    native: usize,
    compact: usize,
}

impl TokenTotals {
    fn add(&mut self, native: usize, compact: usize) {
        self.native += native;
        self.compact += compact;
    }

    fn reduction_percent(&self) -> f64 {
        if self.native == 0 {
            0.0
        } else {
            100.0 * (1.0 - self.compact as f64 / self.native as f64)
        }
    }
}

fn case_id(case: &Value) -> Result<&str, Box<dyn std::error::Error>> {
    case.get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "case is missing id".into())
}

fn write_row(out: &mut fs::File, row: &Value) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(out, "{}", serde_json::to_string(row)?)?;
    Ok(())
}

struct LiveConfig {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl LiveConfig {
    fn from_env() -> Option<Self> {
        let base = env::var("PROVIDER_BASE_URL").ok()?;
        let model = env::var("MODEL").ok()?;
        let endpoint = if base.trim_end_matches('/').ends_with("chat/completions") {
            base
        } else {
            format!("{}/chat/completions", base.trim_end_matches('/'))
        };
        Some(Self {
            endpoint,
            model,
            api_key: env::var("PROVIDER_API_KEY").ok(),
        })
    }
}

async fn add_live_result(row: &mut Value, live: &LiveConfig, request: Value, tools: &[ToolDef]) {
    let mut request = request;
    request["model"] = json!(live.model);
    request["temperature"] = json!(0);
    let mut builder = reqwest::Client::new().post(&live.endpoint).json(&request);
    if let Some(api_key) = &live.api_key {
        builder = builder.bearer_auth(api_key);
    }
    match builder.send().await {
        Ok(response) => match response.json::<Value>().await {
            Ok(response) => {
                let raw_output = response
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                row["raw_output"] = json!(raw_output);
                row["live_calls"] = match decode_calls(&raw_output, tools) {
                    Ok(calls) => json!({"calls": calls}),
                    Err(error) => json!({"error": error_code(&error)}),
                };
            }
            Err(error) => {
                row["raw_output"] = json!("");
                row["live_calls"] = json!({"error": format!("response_parse: {error}")});
            }
        },
        Err(error) => {
            row["raw_output"] = json!("");
            row["live_calls"] = json!({"error": format!("request: {error}")});
        }
    }
}

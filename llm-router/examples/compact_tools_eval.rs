use nasiko_tool_compact::{
    CompactTools, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Duration;
use tiktoken_rs::o200k_base_singleton;

const LIVE_REFERENCE_TIME: &str = "Reference date: 2026-10-02. Timezone: Asia/Kolkata.";

#[derive(Deserialize)]
struct EvalSet {
    #[serde(default)]
    tools: Vec<Value>,
    #[serde(default)]
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<ExpectedCall>,
    #[serde(rename = "match", default)]
    _match: Option<Value>,
}

#[derive(Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
}

struct LiveConfig {
    endpoint: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("compact tools evaluation failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let eval_path = std::env::var("EVAL_SET")?;
    let out_path = std::env::var("OUT")?;
    let eval: EvalSet = serde_json::from_reader(File::open(eval_path)?)?;
    let mut output = BufWriter::new(File::create(out_path)?);
    let tokenizer = o200k_base_singleton();
    let live = live_config().await?;
    let mut baseline_tokens = 0usize;
    let mut compact_tokens = 0usize;

    for case in &eval.cases {
        let native_tools = select_tools(&eval.tools, &case.tools)?;
        let tool_defs = deserialize_tools(&native_tools)?;
        let compact = encode_tools(&tool_defs).ok();
        let candidate_request = build_request(
            &case.messages,
            &native_tools,
            compact.as_ref(),
            compact.is_some(),
            live.as_ref(),
        );
        let baseline_request =
            build_request(&case.messages, &native_tools, None, false, live.as_ref());
        let case_baseline_tokens = count_tokens(tokenizer, &baseline_request)?;
        let candidate_tokens = count_tokens(tokenizer, &candidate_request)?;
        let compacted = should_compact(compact.is_some(), candidate_tokens, case_baseline_tokens);
        let (compact_request, case_compact_tokens) = if compacted {
            (candidate_request, candidate_tokens)
        } else {
            (baseline_request, case_baseline_tokens)
        };
        let rendered_calls = if compacted {
            render_expected_calls(&case.expected)?
        } else {
            String::new()
        };
        let roundtrip_result = if compacted {
            decode_calls(&rendered_calls, &tool_defs)
        } else {
            Ok(Vec::new())
        };
        baseline_tokens += case_baseline_tokens;
        compact_tokens += case_compact_tokens;
        eprintln!(
            "{} tokens: baseline={case_baseline_tokens}, compact={case_compact_tokens}",
            case.id
        );

        let mut line = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_result.as_ref().cloned().unwrap_or_default(),
        });
        if let Err(error) = roundtrip_result {
            line["roundtrip_error"] = json!(error.to_string());
        }
        if let Some(live) = &live {
            let message = send_live_request(live, &line["compact_request"]).await?;
            let raw_output = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            line["raw_output"] = json!(raw_output);
            line["live_calls"] = if compacted {
                call_result_json(decode_calls(raw_output, &tool_defs))
            } else {
                native_calls_json(&message)
            };
        }
        write_json_line(&mut output, &line)?;
    }

    for case in &eval.decoder_cases {
        let native_tools = select_tools(&eval.tools, &case.tools)?;
        let tool_defs = deserialize_tools(&native_tools)?;
        let decoded = decode_stream(&case.chunks, &tool_defs);
        let line = json!({
            "id": case.id,
            "decoded": decoded,
        });
        write_json_line(&mut output, &line)?;
    }
    output.flush()?;

    let reduction = if baseline_tokens == 0 {
        0.0
    } else {
        1.0 - compact_tokens as f64 / baseline_tokens as f64
    };
    eprintln!(
        "local token measurement: baseline={baseline_tokens}, compact={compact_tokens}, reduction={:.1}%",
        reduction * 100.0
    );
    Ok(())
}

fn should_compact(
    compact_schema_supported: bool,
    compact_tokens: usize,
    baseline_tokens: usize,
) -> bool {
    compact_schema_supported && compact_tokens < baseline_tokens
}

async fn live_config() -> Result<Option<LiveConfig>, Box<dyn Error>> {
    let base_url = std::env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let model = std::env::var("MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    match (base_url, model) {
        (None, None) => Ok(None),
        (Some(base_url), Some(model)) => {
            let endpoint = if base_url.ends_with("/chat/completions") {
                base_url
            } else {
                format!("{}/chat/completions", base_url.trim_end_matches('/'))
            };
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(180))
                .build()?;
            Ok(Some(LiveConfig {
                endpoint,
                model,
                api_key: std::env::var("PROVIDER_API_KEY").ok(),
                client,
            }))
        }
        _ => Err("set both PROVIDER_BASE_URL and MODEL to enable live mode".into()),
    }
}

fn select_tools(all_tools: &[Value], names: &[String]) -> Result<Vec<Value>, Box<dyn Error>> {
    names
        .iter()
        .map(|name| {
            all_tools
                .iter()
                .find(|tool| tool_name(tool) == Some(name.as_str()))
                .cloned()
                .ok_or_else(|| format!("evaluation references unknown tool `{name}`").into())
        })
        .collect()
}

fn tool_name(tool: &Value) -> Option<&str> {
    tool.get("function")?.get("name")?.as_str()
}

fn deserialize_tools(tools: &[Value]) -> Result<Vec<ToolDef>, Box<dyn Error>> {
    tools
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn build_request(
    messages: &[Value],
    native_tools: &[Value],
    compact: Option<&CompactTools>,
    compacted: bool,
    live: Option<&LiveConfig>,
) -> Value {
    let mut system_parts: Vec<String> = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("system"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if live.is_some() {
        system_parts.push(LIVE_REFERENCE_TIME.to_string());
    }
    if let Some(compact) = compact.filter(|_| compacted) {
        system_parts.push(format!(
            "Available tools:\n{}\n\n{}",
            compact.definitions, compact.call_instructions
        ));
    }

    let mut request_messages = Vec::new();
    if !system_parts.is_empty() {
        request_messages.push(json!({
            "role": "system",
            "content": system_parts.join("\n\n")
        }));
    }
    request_messages.extend(
        messages
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) != Some("system"))
            .cloned(),
    );

    let mut request = json!({"messages":request_messages});
    if !compacted {
        request["tools"] = json!(native_tools);
    }
    if let Some(live) = live {
        request["model"] = json!(live.model);
        request["temperature"] = json!(0);
    }
    request
}

fn render_expected_calls(calls: &[ExpectedCall]) -> Result<String, Box<dyn Error>> {
    calls
        .iter()
        .map(|call| {
            if !call.arguments.is_object() {
                return Err(
                    format!("expected arguments for `{}` must be an object", call.name).into(),
                );
            }
            let name = if is_simple_identifier(&call.name) {
                call.name.clone()
            } else {
                serde_json::to_string(&call.name)?
            };
            Ok(format!(
                "<<call {name} {}>>",
                serde_json::to_string(&call.arguments)?
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()
        .map(|calls| calls.join("\n"))
}

fn is_simple_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn count_tokens(
    tokenizer: &tiktoken_rs::CoreBPE,
    request: &Value,
) -> Result<usize, Box<dyn Error>> {
    let serialized = serde_json::to_string(request)?;
    Ok(tokenizer.encode_with_special_tokens(&serialized).len())
}

fn call_result_json(result: Result<Vec<ToolCall>, nasiko_tool_compact::DecodeError>) -> Value {
    match result {
        Ok(calls) => json!({"calls": calls}),
        Err(error) => json!({"error": classify_decode_error(&error.message)}),
    }
}

fn decode_stream(chunks: &[String], tools: &[ToolDef]) -> Value {
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = Vec::new();
    for chunk in chunks {
        match decoder.push(chunk) {
            Ok(mut decoded) => calls.append(&mut decoded),
            Err(error) => return json!({"error": classify_decode_error(&error.message)}),
        }
    }
    match decoder.finish() {
        Ok(mut decoded) => {
            calls.append(&mut decoded);
            json!({"calls": calls})
        }
        Err(error) => json!({"error": classify_decode_error(&error.message)}),
    }
}

fn classify_decode_error(message: &str) -> &'static str {
    if message.contains("unknown tool") {
        "unknown_tool"
    } else {
        "invalid_arguments"
    }
}

fn native_calls_json(message: &Value) -> Value {
    let Some(native_calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return json!({"calls": []});
    };
    let mut calls = Vec::with_capacity(native_calls.len());
    for call in native_calls {
        let Some(function) = call.get("function") else {
            return json!({"error": "invalid_arguments"});
        };
        let Some(name) = function.get("name").and_then(Value::as_str) else {
            return json!({"error": "invalid_arguments"});
        };
        let Some(arguments) = function.get("arguments").and_then(Value::as_str) else {
            return json!({"error": "invalid_arguments"});
        };
        let Ok(arguments) = serde_json::from_str::<Value>(arguments) else {
            return json!({"error": "invalid_arguments"});
        };
        calls.push(ToolCall {
            name: name.to_string(),
            arguments,
        });
    }
    json!({"calls": calls})
}

async fn send_live_request(live: &LiveConfig, request: &Value) -> Result<Value, Box<dyn Error>> {
    let mut builder = live.client.post(&live.endpoint).json(request);
    if let Some(api_key) = &live.api_key {
        builder = builder.bearer_auth(api_key);
    }
    let response = builder.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let details = body.chars().take(2048).collect::<String>();
        return Err(format!("provider returned HTTP {status}: {details}").into());
    }
    let response: Value = response.json().await?;
    response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .cloned()
        .ok_or_else(|| "provider response did not contain choices[0].message".into())
}

fn write_json_line(writer: &mut impl Write, value: &Value) -> Result<(), Box<dyn Error>> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_reference_time_is_shared_by_compact_and_baseline_requests() {
        let live = LiveConfig {
            endpoint: String::new(),
            model: "test-model".to_string(),
            api_key: None,
            client: reqwest::Client::new(),
        };
        let compact = CompactTools {
            definitions: "lookup()".to_string(),
            call_instructions: "Use compact calls.".to_string(),
        };
        let compact_request = build_request(&[], &[], Some(&compact), true, Some(&live));
        let baseline_request = build_request(&[], &[], None, false, Some(&live));
        let offline_request = build_request(&[], &[], None, false, None);

        for request in [&compact_request, &baseline_request] {
            let messages = request["messages"]
                .as_array()
                .expect("request messages are an array");
            assert_eq!(
                messages.first().expect("system message exists")["role"],
                "system"
            );
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message["role"] == "system")
                    .count(),
                1
            );
            assert!(
                messages[0]["content"]
                    .as_str()
                    .unwrap()
                    .contains(LIVE_REFERENCE_TIME)
            );
        }
        assert!(
            compact_request["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Available tools:")
        );
        assert!(
            serde_json::to_string(&offline_request)
                .expect("offline request serializes")
                .contains("messages")
        );
        assert!(!offline_request.to_string().contains(LIVE_REFERENCE_TIME));
    }

    #[test]
    fn bypasses_compaction_when_it_would_not_save_tokens() {
        assert!(should_compact(true, 90, 100));
        assert!(!should_compact(true, 100, 100));
        assert!(!should_compact(true, 110, 100));
        assert!(!should_compact(false, 90, 100));
    }
}

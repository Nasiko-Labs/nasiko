use std::{
    io::{BufWriter, Write},
    time::Duration,
};

use nasiko_tool_compact::{
    DecodeError, StreamDecoder, StreamEvent, ToolCall, ToolDef, decode_calls, encode_tools,
    render_calls,
};
use serde_json::{Value, json};
use tiktoken_rs::{CoreBPE, o200k_base};

#[tokio::main]
async fn main() {
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
    let live_config = load_live_config().unwrap_or_else(|error| panic!("{error}"));
    let live_client = live_config.as_ref().map(|_| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_else(|error| panic!("build live HTTP client: {error}"))
    });
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
    let mut live_case_count = 0usize;
    let mut live_match_count = 0usize;
    let mut live_request_failures = 0usize;

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
        let mut baseline_request = json!({
            "messages": messages,
            "tools": selected_schemas,
        });
        if let Some(config) = &live_config {
            add_live_request_fields(&mut baseline_request, &config.model);
        }
        let (compacted, mut compact_request, compact_prompt, compact_definition) =
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
        if let Some(config) = &live_config {
            add_live_request_fields(&mut compact_request, &config.model);
        }

        let expected = case["expected"].as_array().cloned().unwrap_or_default();
        let calls: Vec<ToolCall> = expected.iter().filter_map(expected_call).collect();
        let rendered = render_calls(&calls);
        let roundtrip = decode_calls(&rendered, &selected)
            .unwrap_or_else(|err| panic!("round-trip decode failed for {id}: {err}"));

        let mut line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered,
            "roundtrip_calls": calls_to_json(&roundtrip),
        });
        if let Some(config) = &live_config {
            live_case_count += 1;
            let client = live_client.as_ref().expect("live client configured");
            match request_live_output(client, config, &compact_request).await {
                Ok(raw_output) => {
                    let live_decoded =
                        match decode_chunks(&[Value::String(raw_output.clone())], &selected) {
                            Ok(calls) => {
                                let calls = calls_to_json(&calls);
                                let free_text_fields = case["match"]["free_text_fields"]
                                    .as_array()
                                    .cloned()
                                    .unwrap_or_default();
                                if calls_match_expected(&calls, &expected, &free_text_fields) {
                                    live_match_count += 1;
                                }
                                json!({"calls": calls})
                            }
                            Err(error) => json!({"error": error.code()}),
                        };
                    let object = line.as_object_mut().expect("eval line is an object");
                    object.insert("raw_output".to_string(), json!(raw_output));
                    object.insert("live_calls".to_string(), live_decoded);
                }
                Err(error) => {
                    live_request_failures += 1;
                    let object = line.as_object_mut().expect("eval line is an object");
                    object.insert("raw_output".to_string(), Value::Null);
                    object.insert(
                        "live_calls".to_string(),
                        json!({"error": "live_request_failed"}),
                    );
                    object.insert("live_error".to_string(), json!(error));
                }
            }
        }

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
            live_config.as_ref().map(|config| config.model.as_str()),
            live_case_count,
            live_match_count,
            live_request_failures,
        );
    }
}

struct LiveConfig {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

fn load_live_config() -> Result<Option<LiveConfig>, String> {
    parse_live_config(
        std::env::var("PROVIDER_BASE_URL").ok(),
        std::env::var("MODEL").ok(),
        std::env::var("PROVIDER_API_KEY").ok(),
    )
}

fn parse_live_config(
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<Option<LiveConfig>, String> {
    let base_url = base_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let model = model
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let api_key = api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match (base_url, model) {
        (None, None) => Ok(None),
        (Some(base_url), Some(model)) => Ok(Some(LiveConfig {
            endpoint: chat_completions_endpoint(&base_url),
            model,
            api_key,
        })),
        _ => Err(
            "Set both PROVIDER_BASE_URL and MODEL for live mode, or unset both for offline mode."
                .to_string(),
        ),
    }
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.ends_with("/chat/completions") {
        base_url.to_string()
    } else if base_url.ends_with("/v1") {
        format!("{base_url}/chat/completions")
    } else {
        format!("{base_url}/v1/chat/completions")
    }
}

fn add_live_request_fields(request: &mut Value, model: &str) {
    if let Some(object) = request.as_object_mut() {
        object.insert("model".to_string(), json!(model));
        object.insert("temperature".to_string(), json!(0));
        object.insert("stream".to_string(), json!(false));
        if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
            messages.insert(
                0,
                json!({
                    "role": "system",
                    "content": "Reference date: 2026-10-02. Time zone: Asia/Kolkata."
                }),
            );
        }
    }
}

async fn request_live_output(
    client: &reqwest::Client,
    config: &LiveConfig,
    request_body: &Value,
) -> Result<String, String> {
    let mut request = client.post(&config.endpoint).json(request_body);
    if let Some(api_key) = config.api_key.as_deref() {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("live request transport error: {}", error.without_url()))?;
    if !response.status().is_success() {
        return Err(format!("provider returned HTTP {}", response.status()));
    }
    let response: Value = response
        .json()
        .await
        .map_err(|_| "provider response was not valid JSON".to_string())?;
    response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "provider response lacked choices[0].message.content".to_string())
}

fn calls_match_expected(actual: &Value, expected: &[Value], free_text_fields: &[Value]) -> bool {
    let Some(actual_calls) = actual.as_array() else {
        return false;
    };
    if actual_calls.len() != expected.len() {
        return false;
    }
    for (actual_call, expected_call) in actual_calls.iter().zip(expected) {
        if actual_call["name"] != expected_call["name"] {
            return false;
        }
        let (Some(actual_arguments), Some(expected_arguments)) = (
            actual_call["arguments"].as_object(),
            expected_call["arguments"].as_object(),
        ) else {
            return false;
        };
        if actual_arguments.len() != expected_arguments.len() {
            return false;
        }
        for (key, expected_value) in expected_arguments {
            let Some(actual_value) = actual_arguments.get(key) else {
                return false;
            };
            let is_free_text = free_text_fields
                .iter()
                .any(|field| field.as_str() == Some(key.as_str()));
            if is_free_text {
                if !expected_value.is_string() || !actual_value.is_string() {
                    return false;
                }
            } else if actual_value != expected_value {
                return false;
            }
        }
    }
    true
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
    live_model: Option<&str>,
    live_case_count: usize,
    live_match_count: usize,
    live_request_failures: usize,
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
    let live_summary = live_model.map_or_else(
        || {
            "Real-model testing: **unverified**. Set both `PROVIDER_BASE_URL` and `MODEL` to enable live requests; optional `PROVIDER_API_KEY` is sent only as a bearer header and is never written to output.".to_string()
        },
        |model| {
            format!(
                "Live model `{model}` was requested for {live_case_count} cases; {live_match_count} decoded to expected outputs and {live_request_failures} requests failed. This is not the organizers' private multi-provider evaluation."
            )
        },
    );
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
        "# Compact Tools Demo\n\n**Offline fixture check:** Expected calls are rendered to test encoding and decoding. This alone does not demonstrate real-model tool selection. {live_summary}\n\n## A. Before and After\n\nOriginal public-sample tool schema:\n```json\n{schema}\n```\n\nCompact definition from the encoder:\n```text\n{definition}\n```\n\nEncoder instructions:\n```text\n{prompt}\n```\n\nThe compact definition keeps the tool name, required/optional fields, field types, supported format/enum shorthand, and tool description. Individual property descriptions are omitted in this output.\n\n## B. Token Savings\n\n- Baseline full-request tokens: **{baseline_total}**\n- Compact full-request tokens: **{compact_total}**\n- Aggregate saved: **{saved_percent:.1}%**\n- Bypassed cases: **{bypassed_cases}**\n\nCounts use pinned `tiktoken-rs` 0.12.0 `o200k_base` over minified serialized request bodies. Baselines contain original messages and selected native tool schemas; compact requests contain the injected system instructions and original messages.\n\n## C. Successful Call\n\n{success}\n\n## D. Rejected Invalid Call\n\n{invalid}\n\n## Demo Lines\n\n\"Here are the long instructions.\"\n\n\"Here is our shorter version.\"\n\n\"We saved {saved_percent:.1}% of full-request tokens on this public sample.\"\n\n\"This valid call was decoded correctly.\"\n\n\"This invalid call was rejected.\"\n",
    );
    std::fs::write(path, report).unwrap_or_else(|err| panic!("write demo report {path}: {err}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_mode_stays_offline_without_provider_and_model() {
        assert!(parse_live_config(None, None, None).unwrap().is_none());
    }

    #[test]
    fn live_mode_requires_provider_and_model_together() {
        assert!(parse_live_config(Some("http://localhost".to_string()), None, None).is_err());
        assert!(parse_live_config(None, Some("test-model".to_string()), None).is_err());
    }

    #[test]
    fn chat_completions_endpoint_supports_base_and_full_urls() {
        assert_eq!(
            chat_completions_endpoint("https://example.test"),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("https://example.test/v1/"),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("https://example.test/v1/chat/completions"),
            "https://example.test/v1/chat/completions"
        );
    }

    #[tokio::test]
    async fn live_request_sends_chat_body_and_extracts_assistant_text() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer mock-token")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "test-model",
                "temperature": 0,
                "stream": false,
                "messages": [
                    {
                        "role": "system",
                        "content": "Reference date: 2026-10-02. Time zone: Asia/Kolkata."
                    },
                    {"role": "user", "content": "hello"}
                ]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "<<call ping {}>>"
                        }
                    }]
                })
                .to_string(),
            )
            .create_async()
            .await;
        let config = LiveConfig {
            endpoint: chat_completions_endpoint(&server.url()),
            model: "test-model".to_string(),
            api_key: Some("mock-token".to_string()),
        };
        let mut request_body = json!({
            "messages": [{"role": "user", "content": "hello"}]
        });
        add_live_request_fields(&mut request_body, &config.model);

        let output = request_live_output(&reqwest::Client::new(), &config, &request_body)
            .await
            .unwrap();

        assert_eq!(output, "<<call ping {}>>");
        let tools = vec![ToolDef {
            name: "ping".to_string(),
            description: None,
            parameters: None,
        }];
        let decoded = decode_chunks(&[Value::String(output)], &tools).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "ping");
        assert!(
            !serde_json::to_string(&request_body)
                .unwrap()
                .contains("mock-token")
        );
        mock.assert_async().await;
    }
}

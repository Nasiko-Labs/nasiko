//! Required compact-tools harness: deterministic offline by default; optional
//! OpenAI-compatible live mode. See ../docs/compact-tools.md.
use nasiko_tool_compact::{
    CompactError, INSTRUCTIONS, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools,
    encode_tools, render_calls,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env,
    error::Error,
    fs::File,
    io::{BufWriter, Write},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Dataset {
    tools: Vec<ToolDef>,
    cases: Vec<Case>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}
#[derive(Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ToolCall>,
}
#[derive(Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
}

fn select(names: &[String], catalog: &[ToolDef]) -> Result<Vec<ToolDef>, Box<dyn Error>> {
    names
        .iter()
        .map(|name| {
            catalog
                .iter()
                .find(|tool| tool.function.name == *name)
                .cloned()
                .ok_or_else(|| format!("unknown catalog tool: {name}").into())
        })
        .collect()
}
fn decoded(result: nasiko_tool_compact::Result<Vec<ToolCall>>) -> Value {
    match result {
        Ok(calls) => json!({"calls":calls}),
        Err(error) => json!({"error":error.code()}),
    }
}
fn native_request(messages: &[Value], tools: &[ToolDef]) -> Value {
    json!({"messages":messages,"tools":tools})
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(12 * 60);
    let data = match env::var("EVAL_SET") {
        Ok(path) => std::fs::read_to_string(path)?,
        Err(_) => include_str!("data/compact_tools_sample.json").into(),
    };
    let dataset: Dataset = serde_json::from_str(&data)?;
    let mut out = BufWriter::new(File::create(
        env::var("OUT").unwrap_or_else(|_| "/tmp/compact-tools-out.jsonl".into()),
    )?);
    let backend = match (env::var("PROVIDER_BASE_URL"), env::var("MODEL")) {
        (Ok(base), Ok(model)) => Some((base, model)),
        (Err(_), Err(_)) => None,
        _ => return Err("live mode needs both PROVIDER_BASE_URL and MODEL".into()),
    };
    let client = if backend.is_some() {
        Some(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(45))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        )
    } else {
        None
    };
    let tokenizer = tiktoken_rs::o200k_base()?;
    let (mut baseline_tokens, mut compact_tokens) = (0usize, 0usize);
    let (mut compacted_cases, mut live_ok, mut live_errors) = (0usize, 0usize, 0usize);
    for case in dataset.cases {
        let tools = select(&case.tools, &dataset.tools)?;
        let native = native_request(&case.messages, &tools);
        let (mut request, compacted, rendered, roundtrip) = match encode_tools(&tools) {
            Ok(compact) => {
                // Reconstruct the actual transmitted definitions to catch encoder
                // regressions before any eval output is claimed.
                if decode_tools(&compact)? != tools {
                    return Err("schema round trip failed".into());
                }
                let mut messages = vec![
                    json!({"role":"system","content":format!("{INSTRUCTIONS}\n{}",compact.definitions)}),
                ];
                messages.extend(case.messages.clone());
                let rendered = render_calls(&case.expected, &tools)?;
                let calls = decode_calls(&rendered, &tools)?;
                (json!({"messages":messages}), true, rendered, calls)
            }
            Err(CompactError::UnsupportedSchema(_) | CompactError::ResourceLimit) => {
                // No compact instruction or transformation for unsupported tools.
                // Offline native roundtrip is serialization, not model inference.
                let text = serde_json::to_string(&case.expected)?;
                let calls = serde_json::from_str::<Vec<ToolCall>>(&text)?;
                (native.clone(), false, text, calls)
            }
            Err(error) => return Err(error.into()),
        };
        baseline_tokens += tokenizer
            .encode_with_special_tokens(&serde_json::to_string(&native)?)
            .len();
        compact_tokens += tokenizer
            .encode_with_special_tokens(&serde_json::to_string(&request)?)
            .len();
        if compacted {
            compacted_cases += 1;
        }
        let mut row = json!({"id":case.id,"compact_request":request,"compacted":compacted,"rendered_calls":rendered,"roundtrip_calls":roundtrip});
        if let Some((base, model)) = &backend {
            // Common live-only settings are excluded from the offline token
            // comparison; the scorer supplies its own canonical native baseline.
            request["messages"].as_array_mut().ok_or("missing messages")?.insert(0,json!({"role":"system","content":"Today is 2026-10-02. Timezone: Asia/Kolkata."}));
            request["model"] = json!(model);
            request["temperature"] = json!(0);
            row["compact_request"] = request.clone();
            let result = live(
                client.as_ref().ok_or("missing HTTP client")?,
                base,
                &request,
                &tools,
                compacted,
                deadline.saturating_duration_since(Instant::now()),
            )
            .await;
            match result {
                Ok((raw, calls)) => {
                    if calls.get("error").is_none() {
                        live_ok += 1;
                    } else {
                        live_errors += 1;
                    }
                    row["raw_output"] = raw;
                    row["live_calls"] = calls;
                }
                Err(error) => {
                    live_errors += 1;
                    row["raw_output"] = Value::Null;
                    row["live_calls"] = json!({"error":error});
                }
            }
        }
        serde_json::to_writer(&mut out, &row)?;
        writeln!(out)?;
    }
    for case in dataset.decoder_cases {
        let tools = select(&case.tools, &dataset.tools)?;
        let result = (|| {
            let mut decoder = StreamDecoder::new(&tools)?;
            for chunk in case.chunks {
                decoder.push(&chunk)?;
            }
            decoder.finish()
        })();
        serde_json::to_writer(&mut out, &json!({"id":case.id,"decoded":decoded(result)}))?;
        writeln!(out)?;
    }
    out.flush()?;
    let reduction = if baseline_tokens == 0 {
        0.0
    } else {
        1.0 - compact_tokens as f64 / baseline_tokens as f64
    };
    eprintln!(
        "Full-request o200k_base tokens: native={baseline_tokens}, compact={compact_tokens}, reduction={:.2}%, compacted_cases={compacted_cases}",
        100.0 * reduction
    );
    if backend.is_some() {
        eprintln!(
            "Live valid/empty decoded outputs={live_ok}, errors={live_errors}; semantic accuracy requires separate scoring."
        );
    }
    Ok(())
}

async fn live(
    client: &reqwest::Client,
    base: &str,
    request: &Value,
    tools: &[ToolDef],
    compacted: bool,
    remaining: Duration,
) -> Result<(Value, Value), String> {
    if remaining.is_zero() {
        return Err("evaluation_timeout".into());
    }
    let mut url = reqwest::Url::parse(base).map_err(|_| "invalid_endpoint")?;
    if (url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_endpoint".into());
    }
    let path = url.path().trim_end_matches('/');
    if !path.ends_with("/chat/completions") {
        url.set_path(&format!("{path}/chat/completions"));
    }
    let mut send = client
        .post(url)
        .json(request)
        .timeout(remaining.min(Duration::from_secs(45)));
    if let Ok(key) = env::var("PROVIDER_API_KEY").or_else(|_| env::var("OPENAI_API_KEY")) {
        send = send.bearer_auth(key);
    }
    let mut response = send.send().await.map_err(|_| "network_error")?;
    if !response.status().is_success() {
        return Err(format!("http_{}", response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "network_error")? {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err("resource_limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid_response")?;
    let message = response
        .pointer("/choices/0/message")
        .ok_or("invalid_response")?;
    if compacted {
        let text = message["content"].as_str().ok_or("invalid_response")?;
        Ok((json!(text), decoded(decode_calls(text, tools))))
    } else {
        let mut calls = Vec::new();
        if let Some(native) = message["tool_calls"].as_array() {
            for call in native {
                let name = call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .ok_or("invalid_response")?;
                let args = call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .ok_or("invalid_response")?;
                calls.push(ToolCall {
                    name: name.into(),
                    arguments: serde_json::from_str(args).map_err(|_| "invalid_response")?,
                });
            }
        }
        Ok((message["content"].clone(), json!({"calls":calls})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn live_transport_decodes_real_http_text_without_credentials() {
        let mut server = mockito::Server::new_async().await;
        let mock=server.mock("POST","/v1/chat/completions").match_body(mockito::Matcher::PartialJson(json!({"model":"test","temperature":0}))).with_status(200).with_header("content-type","application/json").with_body(json!({"choices":[{"message":{"content":"<<call send {\"text\":\"hello >> 🦀\"}>>"}}]}).to_string()).create_async().await;
        let tools:Vec<ToolDef>=serde_json::from_value(json!([{"type":"function","function":{"name":"send","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}}])).unwrap();
        let (raw, result) = live(
            &reqwest::Client::new(),
            &format!("{}/v1", server.url()),
            &json!({"model":"test","temperature":0,"messages":[]}),
            &tools,
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert!(raw.as_str().unwrap().contains("<<call"));
        assert_eq!(
            result,
            json!({"calls":[{"name":"send","arguments":{"text":"hello >> 🦀"}}]})
        );
        mock.assert_async().await;
    }
    #[tokio::test]
    async fn live_errors_are_outputs_not_guessed_calls() {
        let client = reqwest::Client::new();
        assert_eq!(
            live(
                &client,
                "https://example.invalid/v1",
                &json!({}),
                &[],
                true,
                Duration::ZERO
            )
            .await
            .unwrap_err(),
            "evaluation_timeout"
        );
        assert_eq!(
            live(
                &client,
                "http://remote.example/v1",
                &json!({}),
                &[],
                true,
                Duration::from_secs(1)
            )
            .await
            .unwrap_err(),
            "invalid_endpoint"
        );
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(429)
            .with_body("private details")
            .create_async()
            .await;
        assert_eq!(
            live(
                &client,
                &server.url(),
                &json!({}),
                &[],
                true,
                Duration::from_secs(2)
            )
            .await
            .unwrap_err(),
            "http_429"
        );
        mock.assert_async().await;
    }
}

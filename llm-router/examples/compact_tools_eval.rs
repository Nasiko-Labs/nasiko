//! P1 Evaluation Harness: compact tool schemas and call decoding.
//!
//! # Usage
//! ```sh
//! EVAL_SET=/path/to/eval-set.json OUT=/path/to/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! # Modes
//! - **Default (Offline)**: Deterministic, no network, no LLM calls.
//!   Evaluates tool compaction, round-trip decoding, and incremental streaming.
//! - **Output Format**: Writes one JSONL line per case to `OUT`.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use nasiko_tool_compact::{StreamDecoder, ToolDef, decode_calls, encode_tools};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Input evaluation set file structure (`compact-tools-eval@v1-sample`).
#[derive(Debug, Clone, Deserialize)]
pub struct EvalSet {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub cases: Vec<TestCase>,
    #[serde(default)]
    pub decoder_cases: Vec<DecoderCase>,
}

/// A regular evaluation case testing compaction and round-trip decoding.
#[derive(Debug, Clone, Deserialize)]
pub struct TestCase {
    pub id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub expected: Vec<ExpectedCall>,
    #[serde(default)]
    pub r#match: Option<Value>,
}

/// An expected tool call with parsed arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedCall {
    pub name: String,
    pub arguments: Value,
}

/// A decoder evaluation case testing streaming chunks and error detection.
#[derive(Debug, Clone, Deserialize)]
pub struct DecoderCase {
    pub id: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub chunks: Vec<String>,
    #[serde(default)]
    pub expected: Option<Value>,
}

/// Configuration for live model evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

/// Detect live mode configuration from optional environment variables.
pub fn detect_live_config() -> Result<Option<LiveConfig>, String> {
    let base_url = std::env::var("PROVIDER_BASE_URL").ok();
    let model = std::env::var("MODEL").ok();
    detect_live_config_from(base_url.as_deref(), model.as_deref())
}

/// Pure helper for detecting live config from string slices (safe for testing).
pub fn detect_live_config_from(
    base_url_opt: Option<&str>,
    model_opt: Option<&str>,
) -> Result<Option<LiveConfig>, String> {
    let base_url = base_url_opt.map(str::trim).filter(|s| !s.is_empty());
    let model = model_opt.map(str::trim).filter(|s| !s.is_empty());

    match (base_url, model) {
        (Some(b), Some(m)) => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("PROVIDER_API_KEY"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            Ok(Some(LiveConfig {
                base_url: b.to_string(),
                model: m.to_string(),
                api_key,
            }))
        }
        (None, None) => Ok(None),
        (Some(_), None) => Err(
            "PROVIDER_BASE_URL is set but MODEL is missing. Both must be set to enable live mode."
                .to_string(),
        ),
        (None, Some(_)) => Err(
            "MODEL is set but PROVIDER_BASE_URL is missing. Both must be set to enable live mode."
                .to_string(),
        ),
    }
}

/// Normalize base URL to ensure it targets the `/chat/completions` endpoint.
pub fn resolve_endpoint_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

/// Output JSONL line for a regular case.
#[derive(Debug, Clone, Serialize)]
pub struct CaseOutput {
    pub id: String,
    pub compact_request: Value,
    pub compacted: bool,
    pub rendered_calls: String,
    pub roundtrip_calls: Vec<ExpectedCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_calls: Option<Value>,
}

/// Output JSONL line for a decoder case.
#[derive(Debug, Clone, Serialize)]
pub struct DecoderCaseOutput {
    pub id: String,
    pub decoded: Value,
}

/// Fixed reference time required by the official P1 specification (line 119).
pub const SYSTEM_REFERENCE_TIME: &str = "Today is 2026-10-02, timezone Asia/Kolkata.";

/// Run the evaluation harness on the given input and output paths.
pub async fn run_evaluation(
    eval_set_path: &Path,
    out_path: &Path,
    live_config: Option<&LiveConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(eval_set_path).map_err(|e| {
        format!(
            "failed to open EVAL_SET at {}: {e}",
            eval_set_path.display()
        )
    })?;
    let reader = BufReader::new(file);
    let eval_set: EvalSet = serde_json::from_reader(reader)
        .map_err(|e| format!("failed to parse EVAL_SET JSON: {e}"))?;

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let out_file = File::create(out_path)
        .map_err(|e| format!("failed to create OUT file at {}: {e}", out_path.display()))?;
    let mut writer = BufWriter::new(out_file);

    let http_client = if live_config.is_some() {
        Some(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()?,
        )
    } else {
        None
    };

    // Initialize o200k_base tokenizer for local token reduction measurement (hackathon line 150)
    let bpe = tiktoken_rs::o200k_base().ok();
    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;

    // 1. Process regular cases
    for case in &eval_set.cases {
        let output = process_case(case, &eval_set.tools, live_config, http_client.as_ref()).await;

        if let Some(ref tokenizer) = bpe {
            let tool_names: HashSet<&str> = case.tools.iter().map(String::as_str).collect();
            let case_tools: Vec<ToolDef> = eval_set
                .tools
                .iter()
                .filter(|t| tool_names.contains(t.function.name.as_str()))
                .cloned()
                .collect();
            let baseline_request = json!({
                "messages": case.messages,
                "tools": case_tools,
            });

            if let (Ok(base_str), Ok(comp_str)) = (
                serde_json::to_string(&baseline_request),
                serde_json::to_string(&output.compact_request),
            ) {
                let base_tokens = tokenizer.encode_with_special_tokens(&base_str).len();
                let comp_tokens = tokenizer.encode_with_special_tokens(&comp_str).len();
                total_baseline_tokens += base_tokens;
                total_compact_tokens += comp_tokens;
            }
        }

        let line = serde_json::to_string(&output)?;
        writeln!(writer, "{line}")?;
    }

    // 2. Process streaming decoder cases
    for dec_case in &eval_set.decoder_cases {
        let output = process_decoder_case(dec_case, &eval_set.tools);
        let line = serde_json::to_string(&output)?;
        writeln!(writer, "{line}")?;
    }

    writer.flush()?;

    if total_baseline_tokens > 0 {
        let reduction_pct =
            (1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64)) * 100.0;
        eprintln!(
            "\n--- Local Token Measurement (o200k_base) ---\n\
             Baseline prompt tokens: {total_baseline_tokens}\n\
             Compact prompt tokens:  {total_compact_tokens}\n\
             Token reduction:        {reduction_pct:.2}%\n\
             --------------------------------------------"
        );
    }

    Ok(())
}

/// Process a single compaction test case.
pub async fn process_case(
    case: &TestCase,
    all_tools: &[ToolDef],
    live_config: Option<&LiveConfig>,
    client: Option<&reqwest::Client>,
) -> CaseOutput {
    // Resolve tools referenced by name in this case
    let tool_names: HashSet<&str> = case.tools.iter().map(String::as_str).collect();
    let case_tools: Vec<ToolDef> = all_tools
        .iter()
        .filter(|t| tool_names.contains(t.function.name.as_str()))
        .cloned()
        .collect();

    // Check for unsupported schema features that require compaction bypass (line 155)
    let has_unsupported = has_unsupported_schema_features(&case_tools);

    let (compact_request, compacted) = if has_unsupported {
        // Bypass compaction: keep native uncompacted request (compacted: false)
        let req = json!({
            "messages": case.messages,
            "tools": case_tools,
        });
        (req, false)
    } else {
        match encode_tools(&case_tools) {
            Ok(compact) => {
                let system_prompt = format!("{SYSTEM_REFERENCE_TIME}\n\n{}", compact.prompt_text);
                let mut messages = Vec::with_capacity(case.messages.len() + 1);
                messages.push(json!({
                    "role": "system",
                    "content": system_prompt,
                }));
                messages.extend(case.messages.clone());

                let req = json!({
                    "messages": messages,
                });
                (req, true)
            }
            Err(_) => {
                // Fail-safe bypass if schema encoding encounters unsupported structures
                let req = json!({
                    "messages": case.messages,
                    "tools": case_tools,
                });
                (req, false)
            }
        }
    };

    // Render expected calls in compact syntax: <<call name {json args}>>
    let mut rendered_parts = Vec::with_capacity(case.expected.len());
    for exp in &case.expected {
        let args_str = serde_json::to_string(&exp.arguments).unwrap_or_else(|_| "{}".to_string());
        rendered_parts.push(format!("<<call {} {}>>", exp.name, args_str));
    }
    let rendered_calls = rendered_parts.join(" ");

    // Decode rendered calls back to standard tool calls
    let roundtrip_calls = if rendered_calls.is_empty() {
        Vec::new()
    } else {
        match decode_calls(&rendered_calls, &case_tools) {
            Ok(calls) => calls
                .into_iter()
                .map(|c| {
                    let parsed_args =
                        serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                    ExpectedCall {
                        name: c.function.name,
                        arguments: parsed_args,
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    };

    // Execute live model call if live configuration is active (line 149)
    let (raw_output, live_calls) = if let (Some(cfg), Some(http)) = (live_config, client) {
        let (raw, calls) = execute_live_call(http, cfg, &compact_request, &case_tools).await;
        (Some(raw), Some(calls))
    } else {
        (None, None)
    };

    CaseOutput {
        id: case.id.clone(),
        compact_request,
        compacted,
        rendered_calls,
        roundtrip_calls,
        raw_output,
        live_calls,
    }
}

/// Execute an OpenAI-compatible completion request in live mode (specification line 149).
pub async fn execute_live_call(
    client: &reqwest::Client,
    cfg: &LiveConfig,
    compact_request: &Value,
    case_tools: &[ToolDef],
) -> (String, Value) {
    let url = resolve_endpoint_url(&cfg.base_url);

    let mut payload = compact_request.clone();
    if let Value::Object(ref mut map) = payload {
        map.insert("model".to_string(), json!(cfg.model));
        map.insert("temperature".to_string(), json!(0.0));
    }

    let mut req_builder = client.post(&url).json(&payload);
    if let Some(key) = &cfg.api_key {
        req_builder = req_builder.bearer_auth(key);
    }

    let resp = match req_builder.send().await {
        Ok(r) => r,
        Err(_) => {
            return (String::new(), json!({ "error": "network_error" }));
        }
    };

    if !resp.status().is_success() {
        return (String::new(), json!({ "error": "api_error" }));
    }

    let body_json: Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => {
            return (String::new(), json!({ "error": "invalid_response" }));
        }
    };

    let content = body_json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("message"))
        .and_then(|msg| msg.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let live_calls = match decode_calls(&content, case_tools) {
        Ok(calls) => {
            let call_outputs: Vec<ExpectedCall> = calls
                .into_iter()
                .map(|c| {
                    let parsed_args =
                        serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                    ExpectedCall {
                        name: c.function.name,
                        arguments: parsed_args,
                    }
                })
                .collect();
            json!({ "calls": call_outputs })
        }
        Err(err) => json!({ "error": err.as_code() }),
    };

    (content, live_calls)
}

/// Process a streaming decoder case.
pub fn process_decoder_case(dec_case: &DecoderCase, all_tools: &[ToolDef]) -> DecoderCaseOutput {
    let tool_names: HashSet<&str> = dec_case.tools.iter().map(String::as_str).collect();
    let case_tools: Vec<ToolDef> = all_tools
        .iter()
        .filter(|t| tool_names.contains(t.function.name.as_str()))
        .cloned()
        .collect();

    let decoded = match StreamDecoder::decode_chunks(&case_tools, &dec_case.chunks) {
        Ok(calls) => {
            let call_outputs: Vec<ExpectedCall> = calls
                .into_iter()
                .map(|c| {
                    let parsed_args =
                        serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                    ExpectedCall {
                        name: c.function.name,
                        arguments: parsed_args,
                    }
                })
                .collect();
            json!({ "calls": call_outputs })
        }
        Err(err) => {
            json!({ "error": err.as_code() })
        }
    };

    DecoderCaseOutput {
        id: dec_case.id.clone(),
        decoded,
    }
}

/// Detect unsupported JSON Schema keywords that mandate compaction bypass (e.g. oneOf, anyOf, allOf, $ref).
fn has_unsupported_schema_features(tools: &[ToolDef]) -> bool {
    for tool in tools {
        if let Some(params) = &tool.function.parameters
            && contains_unsupported_keywords(params)
        {
            return true;
        }
    }
    false
}

fn contains_unsupported_keywords(val: &Value) -> bool {
    match val {
        Value::Object(map) => {
            for (key, v) in map {
                if key == "oneOf" || key == "anyOf" || key == "allOf" || key == "$ref" {
                    return true;
                }
                if contains_unsupported_keywords(v) {
                    return true;
                }
            }
            false
        }
        Value::Array(arr) => arr.iter().any(contains_unsupported_keywords),
        _ => false,
    }
}

#[tokio::main]
async fn main() {
    let eval_set_path = std::env::var("EVAL_SET").unwrap_or_default();
    let out_path = std::env::var("OUT").unwrap_or_default();

    if eval_set_path.is_empty() || out_path.is_empty() {
        eprintln!(
            "usage: EVAL_SET=<input_path> OUT=<output_path> [PROVIDER_BASE_URL=<url> MODEL=<model>] cargo run --release -p nasiko-llm-router --example compact_tools_eval"
        );
        std::process::exit(1);
    }

    let live_config = match detect_live_config() {
        Ok(cfg) => cfg,
        Err(err_msg) => {
            eprintln!("configuration error: {err_msg}");
            std::process::exit(1);
        }
    };

    if let Err(e) = run_evaluation(
        Path::new(&eval_set_path),
        Path::new(&out_path),
        live_config.as_ref(),
    )
    .await
    {
        eprintln!("evaluation failed: {e}");
        std::process::exit(1);
    }

    println!("Evaluation completed successfully. Results written to {out_path}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(nasiko_tool_compact::FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create calendar event".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string" }
                    },
                    "required": ["title", "start"]
                })),
            }),
            ToolDef::new(nasiko_tool_compact::FunctionDef {
                name: "send_email".into(),
                description: Some("Send an email".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" } },
                        "subject": { "type": "string" }
                    },
                    "required": ["to", "subject"]
                })),
            }),
        ]
    }

    #[tokio::test]
    async fn test_process_case_compacted_success() {
        let tools = test_tools();
        let case = TestCase {
            id: "ct-001".into(),
            tools: vec!["create_calendar_event".into()],
            messages: vec![json!({"role": "user", "content": "Book a review"})],
            expected: vec![ExpectedCall {
                name: "create_calendar_event".into(),
                arguments: json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30"}),
            }],
            r#match: None,
        };

        let out = process_case(&case, &tools, None, None).await;
        assert_eq!(out.id, "ct-001");
        assert!(out.compacted);
        assert!(out.rendered_calls.contains("<<call create_calendar_event"));
        assert_eq!(out.roundtrip_calls.len(), 1);
        assert_eq!(out.roundtrip_calls[0].name, "create_calendar_event");
        assert_eq!(out.roundtrip_calls[0].arguments["title"], "Review");
        assert!(out.raw_output.is_none());
        assert!(out.live_calls.is_none());
    }

    #[tokio::test]
    async fn test_process_case_unsupported_schema_bypassed() {
        let mut tools = test_tools();
        tools.push(ToolDef::new(nasiko_tool_compact::FunctionDef {
            name: "unsupported_tool".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "val": {
                        "oneOf": [{"type": "string"}, {"type": "integer"}]
                    }
                }
            })),
        }));

        let case = TestCase {
            id: "ct-unsupported".into(),
            tools: vec!["unsupported_tool".into()],
            messages: vec![json!({"role": "user", "content": "hello"})],
            expected: vec![],
            r#match: None,
        };

        let out = process_case(&case, &tools, None, None).await;
        assert_eq!(out.id, "ct-unsupported");
        assert!(!out.compacted); // Bypassed!
        assert!(out.compact_request.get("tools").is_some());
    }

    #[test]
    fn test_process_decoder_case_success() {
        let tools = test_tools();
        let dec_case = DecoderCase {
            id: "dc-002".into(),
            note: Some("split chunks".into()),
            tools: vec!["create_calendar_event".into()],
            chunks: vec![
                "<<ca".into(),
                "ll create_calendar_event {\"title\":\"Ret".into(),
                "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>".into(),
                ">".into(),
            ],
            expected: None,
        };

        let out = process_decoder_case(&dec_case, &tools);
        assert_eq!(out.id, "dc-002");
        let calls = out.decoded.get("calls").and_then(Value::as_array).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "create_calendar_event");
        assert_eq!(calls[0]["arguments"]["title"], "Retro");
    }

    #[test]
    fn test_process_decoder_case_error() {
        let tools = test_tools();
        let dec_case = DecoderCase {
            id: "dc-err".into(),
            note: Some("unknown tool".into()),
            tools: vec!["create_calendar_event".into()],
            chunks: vec!["<<call unknown_fn {}>>".into()],
            expected: None,
        };

        let out = process_decoder_case(&dec_case, &tools);
        assert_eq!(out.id, "dc-err");
        assert_eq!(out.decoded["error"], "unknown_tool");
    }

    #[test]
    fn test_resolve_endpoint_url() {
        assert_eq!(
            resolve_endpoint_url("http://localhost:8000/v1"),
            "http://localhost:8000/v1/chat/completions"
        );
        assert_eq!(
            resolve_endpoint_url("http://localhost:8000/v1/"),
            "http://localhost:8000/v1/chat/completions"
        );
        assert_eq!(
            resolve_endpoint_url("http://localhost:8000/v1/chat/completions"),
            "http://localhost:8000/v1/chat/completions"
        );
    }

    #[test]
    fn test_detect_live_config_variations() {
        // Both missing -> Ok(None)
        assert!(detect_live_config_from(None, None).unwrap().is_none());
        assert!(
            detect_live_config_from(Some(""), Some(""))
                .unwrap()
                .is_none()
        );

        // Both present -> Ok(Some)
        let cfg = detect_live_config_from(Some("http://proxy.internal"), Some("gpt-4o"))
            .unwrap()
            .unwrap();
        assert_eq!(cfg.base_url, "http://proxy.internal");
        assert_eq!(cfg.model, "gpt-4o");

        // Partial config -> Err
        assert!(detect_live_config_from(Some("http://proxy.internal"), None).is_err());
        assert!(detect_live_config_from(None, Some("gpt-4o")).is_err());
    }

    #[tokio::test]
    async fn test_process_case_live_mode_success() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock_response = json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "<<call create_calendar_event {\"title\":\"Live Review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>"
                    }
                }
            ]
        });

        let expected_prompt = format!(
            "{SYSTEM_REFERENCE_TIME}\n\ncreate_calendar_event(start:str, title:str) - Create calendar event\nTo call a tool, emit: <<call name {{json args}}>>"
        );

        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "mock-model",
                "temperature": 0.0,
                "messages": [
                    {
                        "role": "system",
                        "content": expected_prompt
                    },
                    {
                        "role": "user",
                        "content": "Book review"
                    }
                ]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create_async()
            .await;

        let tools = test_tools();
        let case = TestCase {
            id: "ct-live-01".into(),
            tools: vec!["create_calendar_event".into()],
            messages: vec![json!({"role": "user", "content": "Book review"})],
            expected: vec![],
            r#match: None,
        };

        let live_cfg = LiveConfig {
            base_url: url,
            model: "mock-model".into(),
            api_key: Some("test-key".into()),
        };

        let client = reqwest::Client::new();
        let out = process_case(&case, &tools, Some(&live_cfg), Some(&client)).await;

        mock.assert_async().await;
        assert_eq!(out.id, "ct-live-01");
        assert!(out.raw_output.is_some());
        assert!(
            out.raw_output
                .unwrap()
                .contains("<<call create_calendar_event")
        );

        assert!(out.live_calls.is_some());
        let live_calls = out.live_calls.unwrap();
        let calls = live_calls["calls"]
            .as_array()
            .expect("expected calls array");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "create_calendar_event");
        assert_eq!(calls[0]["arguments"]["title"], "Live Review");
    }

    #[tokio::test]
    async fn test_process_case_live_mode_unsupported_schema_preserves_tools() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let mock_response = json!({
            "id": "chatcmpl-unsupported",
            "object": "chat.completion",
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "<<call unsupported_tool {\"val\":\"rust\"}>>"
                    }
                }
            ]
        });

        let mut tools = test_tools();
        tools.push(ToolDef::new(nasiko_tool_compact::FunctionDef {
            name: "unsupported_tool".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "val": {
                        "oneOf": [{"type": "string"}, {"type": "integer"}]
                    }
                }
            })),
        }));

        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "mock-bypass-model",
                "temperature": 0.0,
                "messages": [
                    { "role": "user", "content": "Search query" }
                ],
                "tools": [
                    {
                        "type": "function",
                        "function": {
                            "name": "unsupported_tool",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "val": {
                                        "oneOf": [{"type": "string"}, {"type": "integer"}]
                                    }
                                }
                            }
                        }
                    }
                ]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create_async()
            .await;

        let case = TestCase {
            id: "ct-live-bypass".into(),
            tools: vec!["unsupported_tool".into()],
            messages: vec![json!({"role": "user", "content": "Search query"})],
            expected: vec![],
            r#match: None,
        };

        let live_cfg = LiveConfig {
            base_url: url,
            model: "mock-bypass-model".into(),
            api_key: None,
        };

        let client = reqwest::Client::new();
        let out = process_case(&case, &tools, Some(&live_cfg), Some(&client)).await;

        mock.assert_async().await;
        assert_eq!(out.id, "ct-live-bypass");
        assert!(!out.compacted);
        assert!(out.raw_output.is_some());
        assert!(out.live_calls.is_some());
    }

    #[tokio::test]
    async fn test_process_case_live_mode_api_error_handled_gracefully() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(500)
            .create_async()
            .await;

        let tools = test_tools();
        let case = TestCase {
            id: "ct-live-err".into(),
            tools: vec!["create_calendar_event".into()],
            messages: vec![json!({"role": "user", "content": "Book review"})],
            expected: vec![],
            r#match: None,
        };

        let live_cfg = LiveConfig {
            base_url: url,
            model: "mock-model".into(),
            api_key: None,
        };

        let client = reqwest::Client::new();
        let out = process_case(&case, &tools, Some(&live_cfg), Some(&client)).await;

        assert_eq!(out.id, "ct-live-err");
        assert_eq!(out.live_calls, Some(json!({ "error": "api_error" })));
    }

    #[test]
    fn test_token_measurement_o200k_base() {
        let bpe = tiktoken_rs::o200k_base().expect("failed to load o200k_base tokenizer");

        let tools = test_tools();
        let baseline_req = json!({
            "messages": [{"role": "user", "content": "Book review Monday"}],
            "tools": tools,
        });

        let compact = encode_tools(&tools).expect("failed to encode tools");
        let compact_req = json!({
            "messages": [
                {"role": "system", "content": format!("{SYSTEM_REFERENCE_TIME}\n\n{}", compact.prompt_text)},
                {"role": "user", "content": "Book review Monday"}
            ]
        });

        let base_str = serde_json::to_string(&baseline_req).unwrap();
        let comp_str = serde_json::to_string(&compact_req).unwrap();

        let base_tokens = bpe.encode_with_special_tokens(&base_str).len();
        let comp_tokens = bpe.encode_with_special_tokens(&comp_str).len();

        assert!(base_tokens > 0);
        assert!(comp_tokens > 0);
        assert!(comp_tokens < base_tokens);
        let reduction = 1.0 - (comp_tokens as f64 / base_tokens as f64);
        assert!(
            reduction >= 0.30,
            "expected >= 30% token reduction, got {:.2}%",
            reduction * 100.0
        );
    }
}

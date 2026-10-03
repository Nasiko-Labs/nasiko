//! Deterministic evaluator for `nasiko-tool-compact`.
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Set `PROVIDER_BASE_URL` and `MODEL` to additionally run compact requests
//! against an OpenAI-compatible `/chat/completions` endpoint. `PROVIDER_API_KEY`
//! (or `OPENAI_API_KEY`) is optional and is never read by the library.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};

use nasiko_tool_compact::{
    CompactTools, FunctionDef, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const REFERENCE_TIME: &str = "Today is 2026-10-02. Timezone: Asia/Kolkata.";

#[derive(Debug, Deserialize)]
struct EvalSet {
    tools: Value,
    #[serde(default)]
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<ExpectedCall>,
}

#[derive(Debug, Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("compact_tools_eval: {error}");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = std::env::var("EVAL_SET")?;
    let out_path = std::env::var("OUT")?;
    let set: EvalSet = serde_json::from_reader(File::open(eval_path)?)?;
    let all_tools = parse_tools(set.tools)?;
    let mut out = BufWriter::new(File::create(out_path)?);
    let live = LiveConfig::from_env();
    let client = live.as_ref().map(|_| reqwest::Client::new());

    for case in set.cases {
        let tools = select_tools(&all_tools, &case.tools)?;
        let expected = case
            .expected
            .iter()
            .map(|call| ToolCall {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect::<Vec<_>>();
        let rendered_calls = render_calls(&expected)?;
        let (compact_request, compacted, roundtrip_calls) = match encode_tools(&tools) {
            Ok(compact) => {
                let request = compact_request(&case.messages, &compact);
                let decoded = decode_calls(&rendered_calls, &tools)?;
                (request, true, decoded)
            }
            // An unsupported schema is deliberately sent in native form. The
            // resulting 0% saving is honest and keeps the normal request usable.
            Err(_) => (
                native_request(&case.messages, &tools),
                false,
                expected.clone(),
            ),
        };

        let mut line = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        });
        if let (Some(live), Some(client)) = (&live, &client) {
            let (raw_output, live_calls) =
                run_live(client, live, line["compact_request"].clone(), &tools).await;
            line["raw_output"] = raw_output;
            line["live_calls"] = live_calls;
        }
        write_jsonl(&mut out, &line)?;
    }

    for case in set.decoder_cases {
        let tools = select_tools(&all_tools, &case.tools)?;
        let mut decoder = StreamDecoder::new(&tools);
        for chunk in case.chunks {
            decoder.push(&chunk);
        }
        let decoded = match decoder.finish() {
            Ok(calls) => json!({"calls": calls}),
            Err(error) => json!({"error": error.code()}),
        };
        write_jsonl(&mut out, &json!({"id": case.id, "decoded": decoded}))?;
    }
    out.flush()?;
    Ok(())
}

fn parse_tools(value: Value) -> Result<BTreeMap<String, ToolDef>, Box<dyn std::error::Error>> {
    let entries = match value {
        Value::Array(entries) => entries
            .into_iter()
            .map(|entry| {
                let tool: ToolDef = serde_json::from_value(entry)?;
                Ok((tool.function.name.clone(), tool))
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()?,
        Value::Object(entries) => entries
            .into_iter()
            .map(|(name, entry)| parse_named_tool(name, entry))
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("eval set tools must be an array or object".into()),
    };
    Ok(entries.into_iter().collect())
}

fn parse_named_tool(name: String, entry: Value) -> Result<(String, ToolDef), serde_json::Error> {
    if entry.get("function").is_some() {
        let tool: ToolDef = serde_json::from_value(entry)?;
        return Ok((tool.function.name.clone(), tool));
    }
    let mut entry = entry;
    if let Value::Object(object) = &mut entry
        && !object.contains_key("name")
    {
        object.insert("name".into(), Value::String(name.clone()));
    }
    let function: FunctionDef = serde_json::from_value(entry)?;
    Ok((
        name,
        ToolDef {
            kind: "function".into(),
            function,
            extra: Map::new(),
        },
    ))
}

fn select_tools(
    all_tools: &BTreeMap<String, ToolDef>,
    names: &[String],
) -> Result<Vec<ToolDef>, Box<dyn std::error::Error>> {
    names
        .iter()
        .map(|name| {
            all_tools
                .get(name)
                .cloned()
                .ok_or_else(|| format!("case references missing tool '{name}'").into())
        })
        .collect()
}

fn compact_request(messages: &[Value], compact: &CompactTools) -> Value {
    let mut messages = messages.to_vec();
    messages.insert(
        0,
        json!({
            "role": "system",
            "content": format!("{REFERENCE_TIME}\n\n{}", compact.prompt),
        }),
    );
    json!({"messages": messages, "temperature": 0})
}

fn native_request(messages: &[Value], tools: &[ToolDef]) -> Value {
    json!({"messages": messages, "tools": tools, "temperature": 0})
}

fn render_calls(calls: &[ToolCall]) -> Result<String, serde_json::Error> {
    calls
        .iter()
        .map(|call| {
            Ok(format!(
                "<<call {} {}>>",
                call.name,
                serde_json::to_string(&call.arguments)?
            ))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|calls| calls.join("\n"))
}

fn write_jsonl(out: &mut BufWriter<File>, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}

struct LiveConfig {
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl LiveConfig {
    fn from_env() -> Option<Self> {
        let base_url = std::env::var("PROVIDER_BASE_URL").ok()?;
        let model = std::env::var("MODEL").ok()?;
        Some(Self {
            base_url,
            model,
            api_key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
        })
    }
}

async fn run_live(
    client: &reqwest::Client,
    config: &LiveConfig,
    mut request: Value,
    tools: &[ToolDef],
) -> (Value, Value) {
    request["model"] = Value::String(config.model.clone());
    let endpoint = if config.base_url.ends_with("/chat/completions") {
        config.base_url.clone()
    } else {
        format!("{}/chat/completions", config.base_url.trim_end_matches('/'))
    };
    let mut call = client.post(endpoint).json(&request);
    if let Some(api_key) = &config.api_key {
        call = call.bearer_auth(api_key);
    }
    match call.send().await {
        Ok(response) => match response.json::<Value>().await {
            Ok(body) => {
                let raw = body
                    .pointer("/choices/0/message/content")
                    .cloned()
                    .unwrap_or_else(|| body.clone());
                let decoded = raw.as_str().map(|text| match decode_calls(text, tools) {
                    Ok(calls) => json!({"calls": calls}),
                    Err(error) => json!({"error": error.code()}),
                });
                (
                    raw,
                    decoded.unwrap_or_else(|| json!({"error": "invalid_response"})),
                )
            }
            Err(error) => (
                json!({"error": error.to_string()}),
                json!({"error": "live_request_failed"}),
            ),
        },
        Err(error) => (
            json!({"error": error.to_string()}),
            json!({"error": "live_request_failed"}),
        ),
    }
}

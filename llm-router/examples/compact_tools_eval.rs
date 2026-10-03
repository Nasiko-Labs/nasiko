//! Offline-first evaluator for the compact tool schema build-a-thon track.
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Set both `PROVIDER_BASE_URL` and `MODEL` for live mode. Authentication is read from
//! `PROVIDER_API_KEY`, falling back to `OPENAI_API_KEY`. The base URL may be an API root (for
//! example, `https://api.openai.com/v1`) or a full `/chat/completions` endpoint.

use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, render_call,
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tiktoken_rs::o200k_base;

const REFERENCE_CONTEXT: &str =
    "Evaluation reference: today is 2026-10-02 and the timezone is Asia/Kolkata.";
const MAX_LIVE_ATTEMPTS: usize = 3;

#[derive(Deserialize)]
struct EvalSet {
    schema_version: String,
    tools: Vec<ToolDef>,
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
    expected: Vec<ToolCall>,
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
}

struct TokenTotals {
    native: usize,
    compact: usize,
    compacted_cases: usize,
    bypassed_cases: usize,
}

impl TokenTotals {
    fn new() -> Self {
        Self {
            native: 0,
            compact: 0,
            compacted_cases: 0,
            bypassed_cases: 0,
        }
    }

    fn reduction(&self) -> f64 {
        if self.native == 0 {
            0.0
        } else {
            1.0 - self.compact as f64 / self.native as f64
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let eval_path = env::var("EVAL_SET").context("EVAL_SET must point to the evaluation JSON")?;
    let output_path = env::var("OUT").context("OUT must name the output JSONL file")?;
    let eval_set: EvalSet = serde_json::from_reader(BufReader::new(
        File::open(&eval_path).with_context(|| format!("opening EVAL_SET at {eval_path}"))?,
    ))
    .with_context(|| format!("parsing EVAL_SET at {eval_path}"))?;
    let tool_index = index_tools(&eval_set.tools)?;
    let live = live_config()?;
    let client = if live.is_some() {
        Some(
            Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()
                .context("building live HTTP client")?,
        )
    } else {
        None
    };
    if let Some(parent) = Path::new(&output_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }
    let mut output = BufWriter::new(
        File::create(&output_path).with_context(|| format!("creating OUT at {output_path}"))?,
    );
    let tokenizer = o200k_base().context("initializing o200k_base")?;
    let mut totals = TokenTotals::new();

    for case in &eval_set.cases {
        let tools = resolve_tools(&tool_index, &case.tools)?;
        let native_request = native_request(&case.messages, &tools, live.as_ref());
        let encoded = encode_tools(&tools);
        let (compacted, compact_request_body, rendered_calls, roundtrip_calls, prompt_parts) =
            match encoded {
                Ok(compact) => {
                    let request = compact_request(&case.messages, &compact.prompt(), live.as_ref());
                    let rendered = render_calls(&case.expected)?;
                    let roundtrip = decode_calls(&rendered, &tools)?;
                    let header =
                        format!("Tools ({}):\n", nasiko_tool_compact::CompactTools::legend());
                    let definitions = compact.definitions().to_string();
                    let instructions =
                        format!("\n{}", nasiko_tool_compact::CompactTools::instructions());
                    (
                        true,
                        request,
                        Value::String(rendered),
                        roundtrip,
                        Some((header, definitions, instructions)),
                    )
                }
                Err(error) if is_bypass_error(&error) => (
                    false,
                    native_request.clone(),
                    Value::Null,
                    case.expected.clone(),
                    None,
                ),
                Err(error) => return Err(error).context("encoding tools"),
            };

        let native_json = serde_json::to_string(&native_request)?;
        let compact_json = serde_json::to_string(&compact_request_body)?;
        let native_tokens = tokenizer.encode_with_special_tokens(&native_json).len();
        let compact_tokens = if compacted {
            tokenizer.encode_with_special_tokens(&compact_json).len()
        } else {
            native_tokens
        };
        totals.native += native_tokens;
        totals.compact += compact_tokens;
        if compacted {
            totals.compacted_cases += 1;
        } else {
            totals.bypassed_cases += 1;
        }
        if let Some((header, definitions, instructions)) = prompt_parts {
            let common_tokens = request_tokens(
                &tokenizer,
                &compact_request(&case.messages, "", live.as_ref()),
            )?;
            let with_legend_tokens = request_tokens(
                &tokenizer,
                &compact_request(&case.messages, &header, live.as_ref()),
            )?;
            let with_definitions_tokens = request_tokens(
                &tokenizer,
                &compact_request(
                    &case.messages,
                    &format!("{header}{definitions}"),
                    live.as_ref(),
                ),
            )?;
            eprintln!(
                "case={} native={} compact={} reduction={:.2}% common={} legend={} definitions={} instructions={}",
                case.id,
                native_tokens,
                compact_tokens,
                (1.0 - compact_tokens as f64 / native_tokens as f64) * 100.0,
                common_tokens,
                with_legend_tokens.saturating_sub(common_tokens),
                with_definitions_tokens.saturating_sub(with_legend_tokens),
                compact_tokens.saturating_sub(with_definitions_tokens),
            );
            debug_assert_eq!(
                format!("{header}{definitions}{instructions}"),
                encode_tools(&tools)?.prompt()
            );
        } else {
            eprintln!(
                "case={} native={} compact={} reduction=0.00% bypassed=true",
                case.id, native_tokens, compact_tokens
            );
        }

        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(case.id.clone()));
        record.insert("compact_request".to_string(), compact_request_body.clone());
        record.insert("compacted".to_string(), Value::Bool(compacted));
        record.insert("rendered_calls".to_string(), rendered_calls);
        record.insert(
            "roundtrip_calls".to_string(),
            serde_json::to_value(roundtrip_calls)?,
        );

        if let (Some(config), Some(http)) = (&live, &client) {
            let response = send_live(http, config, compact_request_body).await?;
            add_live_result(&mut record, &response, compacted, &tools)?;
        }
        write_record(&mut output, Value::Object(record))?;
    }

    for case in &eval_set.decoder_cases {
        let tools = resolve_tools(&tool_index, &case.tools)?;
        let decoded = decode_stream_case(case, &tools);
        write_record(
            &mut output,
            json!({
                "id": case.id,
                "decoded": decoded,
            }),
        )?;
    }
    output.flush().context("flushing OUT")?;
    eprintln!(
        "dataset={} tokenizer=o200k_base native_tokens={} compact_tokens={} reduction={:.2}% compacted_cases={} bypassed_cases={}",
        eval_set.schema_version,
        totals.native,
        totals.compact,
        totals.reduction() * 100.0,
        totals.compacted_cases,
        totals.bypassed_cases,
    );
    Ok(())
}

fn index_tools(tools: &[ToolDef]) -> Result<BTreeMap<String, ToolDef>> {
    let mut index = BTreeMap::new();
    for tool in tools {
        if index
            .insert(tool.function.name.clone(), tool.clone())
            .is_some()
        {
            bail!("duplicate dataset tool '{}'", tool.function.name);
        }
    }
    Ok(index)
}

fn resolve_tools(index: &BTreeMap<String, ToolDef>, names: &[String]) -> Result<Vec<ToolDef>> {
    names
        .iter()
        .map(|name| {
            index
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("case references unknown dataset tool '{name}'"))
        })
        .collect()
}

fn messages_with_system(messages: &[Value], system: String) -> Vec<Value> {
    std::iter::once(json!({"role": "system", "content": system}))
        .chain(messages.iter().cloned())
        .collect()
}

fn native_request(messages: &[Value], tools: &[ToolDef], live: Option<&LiveConfig>) -> Value {
    let mut request = Map::new();
    request.insert(
        "messages".to_string(),
        Value::Array(messages_with_system(
            messages,
            REFERENCE_CONTEXT.to_string(),
        )),
    );
    request.insert(
        "tools".to_string(),
        serde_json::to_value(tools).expect("ToolDef serialization is infallible"),
    );
    add_common_request_fields(&mut request, live);
    Value::Object(request)
}

fn compact_request(messages: &[Value], prompt: &str, live: Option<&LiveConfig>) -> Value {
    let mut request = Map::new();
    request.insert(
        "messages".to_string(),
        Value::Array(messages_with_system(
            messages,
            format!("{REFERENCE_CONTEXT}\n\n{prompt}"),
        )),
    );
    add_common_request_fields(&mut request, live);
    Value::Object(request)
}

fn add_common_request_fields(request: &mut Map<String, Value>, live: Option<&LiveConfig>) {
    if let Some(config) = live {
        request.insert("model".to_string(), Value::String(config.model.clone()));
    }
    request.insert("stream".to_string(), Value::Bool(false));
    request.insert("temperature".to_string(), json!(0));
}

fn render_calls(calls: &[ToolCall]) -> Result<String> {
    calls
        .iter()
        .map(|call| render_call(&call.name, &call.arguments).map_err(Into::into))
        .collect::<Result<Vec<_>>>()
        .map(|calls| calls.join("\n"))
}

fn decode_stream_case(case: &DecoderCase, tools: &[ToolDef]) -> Value {
    let decoded = StreamDecoder::new(tools).and_then(|mut decoder| {
        for chunk in &case.chunks {
            decoder.feed(chunk)?;
        }
        decoder.finish()
    });
    match decoded {
        Ok(calls) => json!({"calls": calls}),
        Err(error) => json!({"error": error.code().as_str()}),
    }
}

fn is_bypass_error(error: &CompactError) -> bool {
    matches!(
        error,
        CompactError::UnsupportedSchema { .. }
            | CompactError::InvalidToolDefinition(_)
            | CompactError::DuplicateTool(_)
    )
}

fn live_config() -> Result<Option<LiveConfig>> {
    let base = env::var("PROVIDER_BASE_URL").ok();
    let model = env::var("MODEL").ok();
    match (base, model) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            bail!("live mode requires both PROVIDER_BASE_URL and MODEL")
        }
        (Some(base), Some(model)) => Ok(Some(LiveConfig {
            endpoint: completion_endpoint(&base),
            model,
            api_key: env::var("PROVIDER_API_KEY")
                .or_else(|_| env::var("OPENAI_API_KEY"))
                .ok(),
        })),
    }
}

fn completion_endpoint(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

async fn send_live(client: &Client, config: &LiveConfig, request: Value) -> Result<Value> {
    let mut last_error = None;
    for attempt in 0..MAX_LIVE_ATTEMPTS {
        let mut builder = client.post(&config.endpoint).json(&request);
        if let Some(api_key) = &config.api_key {
            builder = builder.bearer_auth(api_key);
        }
        match builder.send().await {
            Ok(response) if response.status().is_success() => {
                return response
                    .json()
                    .await
                    .context("parsing live provider JSON response");
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let error = anyhow!("live provider returned {status}: {body}");
                if !retryable_status(status) || attempt + 1 == MAX_LIVE_ATTEMPTS {
                    return Err(error);
                }
                last_error = Some(error);
            }
            Err(error) => {
                if (!error.is_connect() && !error.is_timeout()) || attempt + 1 == MAX_LIVE_ATTEMPTS
                {
                    return Err(error).context("calling live provider");
                }
                last_error = Some(error.into());
            }
        }
        tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
    }
    Err(last_error.unwrap_or_else(|| anyhow!("live provider call failed")))
}

fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn add_live_result(
    record: &mut Map<String, Value>,
    response: &Value,
    compacted: bool,
    tools: &[ToolDef],
) -> Result<()> {
    let message = response
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow!("live response lacks choices[0].message"))?;
    let raw_output = message_text(message);
    record.insert("raw_output".to_string(), Value::String(raw_output.clone()));
    let decoded = if compacted {
        decode_calls(&raw_output, tools)
    } else {
        decode_native_calls(message, tools)
    };
    match decoded {
        Ok(calls) => {
            record.insert("live_calls".to_string(), serde_json::to_value(calls)?);
        }
        Err(error) => {
            record.insert(
                "live_error".to_string(),
                Value::String(error.code().as_str().to_string()),
            );
        }
    }
    Ok(())
}

fn message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn decode_native_calls(message: &Value, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut rendered = String::new();
    for call in calls {
        let name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .ok_or_else(|| CompactError::MalformedSyntax("native call lacks a name".to_string()))?;
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CompactError::MalformedSyntax("native call lacks argument JSON".to_string())
            })?;
        rendered.push_str("<<call ");
        rendered.push_str(name);
        rendered.push(' ');
        rendered.push_str(arguments);
        rendered.push_str(">>");
    }
    decode_calls(&rendered, tools)
}

fn write_record(output: &mut BufWriter<File>, record: Value) -> Result<()> {
    serde_json::to_writer(&mut *output, &record).context("serializing JSONL record")?;
    output.write_all(b"\n").context("writing JSONL newline")?;
    Ok(())
}

fn request_tokens(tokenizer: &tiktoken_rs::CoreBPE, request: &Value) -> Result<usize> {
    let serialized = serde_json::to_string(request)?;
    Ok(tokenizer.encode_with_special_tokens(&serialized).len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_root_or_complete_url() {
        assert_eq!(
            completion_endpoint("https://example.test/v1/"),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            completion_endpoint("https://example.test/v1/chat/completions"),
            "https://example.test/v1/chat/completions"
        );
    }

    #[test]
    fn native_decoder_validates_raw_argument_json() {
        let tools = vec![
            serde_json::from_value(json!({
                "type": "function",
                "function": {
                    "name": "echo",
                    "parameters": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"]
                    }
                }
            }))
            .expect("test tool is valid"),
        ];
        let message = json!({
            "tool_calls": [{
                "function": {"name": "echo", "arguments": "{\"text\":\"ok\"}"}
            }]
        });
        assert_eq!(
            decode_native_calls(&message, &tools).expect("native call is valid")[0].arguments["text"],
            "ok"
        );
    }
}

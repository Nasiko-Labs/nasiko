//! Offline by default. EVAL_SET=<dataset.json> OUT=<outputs.jsonl>
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! Live mode: set both PROVIDER_BASE_URL and MODEL, optionally OPENAI_API_KEY.
use nasiko_llm_router::{
    compact_tools::{prepare_request, wire_supported},
    ir::ChatRequest,
};
use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, render_calls,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    error::Error,
    io::Write,
    time::{Duration, Instant},
};

const REFERENCE_TIME: &str =
    "Today is Friday 2026-10-02. Timezone: Asia/Kolkata. Resolve relative dates from this date.";

#[derive(Deserialize)]
struct Dataset {
    tools: Vec<Value>,
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

fn select(
    names: &[String],
    catalog: &HashMap<String, Value>,
) -> Result<Vec<Value>, Box<dyn Error>> {
    let mut seen = std::collections::HashSet::new();
    names
        .iter()
        .map(|name| {
            if !seen.insert(name) {
                return Err(format!("duplicate tool reference {name}").into());
            }
            catalog
                .get(name)
                .cloned()
                .ok_or_else(|| format!("unknown dataset tool {name}").into())
        })
        .collect()
}

fn definitions(native: &[Value]) -> Result<Vec<ToolDef>, Box<dyn Error>> {
    native
        .iter()
        .map(|value| {
            let f = &value["function"];
            Ok(ToolDef {
                name: f["name"].as_str().ok_or("tool name required")?.into(),
                description: f
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                parameters: f.get("parameters").ok_or("parameters required")?.clone(),
            })
        })
        .collect()
}

fn decoded(result: Result<Vec<ToolCall>, nasiko_tool_compact::CompactError>) -> Value {
    match result {
        Ok(calls) => json!({"calls":calls}),
        Err(error) => json!({"error":error.code()}),
    }
}

fn native_calls(response: &Value) -> Result<Vec<ToolCall>, CompactError> {
    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .ok_or_else(|| CompactError::MalformedOutput("missing response choice".into()))?;
    let mut out = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            out.push(ToolCall {
                name: call["function"]["name"]
                    .as_str()
                    .ok_or_else(|| CompactError::MalformedOutput("call name required".into()))?
                    .into(),
                arguments: serde_json::from_str(
                    call["function"]["arguments"].as_str().ok_or_else(|| {
                        CompactError::InvalidArguments("call arguments required".into())
                    })?,
                )
                .map_err(|_| CompactError::InvalidArguments("invalid native call JSON".into()))?,
            });
        }
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let path = std::env::var("EVAL_SET").map_err(|_| "set EVAL_SET to the dataset JSON")?;
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let base = std::env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let model = std::env::var("MODEL").ok().filter(|s| !s.is_empty());
    if base.is_some() != model.is_some() {
        return Err("live mode requires PROVIDER_BASE_URL and MODEL together".into());
    }
    let live = base.is_some();
    let compact_mode = match std::env::var("COMPACT_TOOLS_EVAL_MODE").as_deref() {
        Ok("native") => false,
        Ok("compact") | Err(_) => true,
        _ => return Err("COMPACT_TOOLS_EVAL_MODE must be compact or native".into()),
    };
    // No HTTP client and no credential read in offline mode.
    let http = if live {
        Some(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(40))
                .build()?,
        )
    } else {
        None
    };
    let key = if live {
        std::env::var("OPENAI_API_KEY").ok()
    } else {
        None
    };
    let dataset: Dataset = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let mut catalog = HashMap::new();
    for tool in dataset.tools {
        let name = tool["function"]["name"]
            .as_str()
            .ok_or("tool name required")?
            .to_string();
        if catalog.insert(name, tool).is_some() {
            return Err("duplicate catalog tool".into());
        }
    }
    let tokenizer = tiktoken_rs::o200k_base()?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(out_path)?);
    let (mut baseline_tokens, mut compact_tokens, mut bypasses) = (0usize, 0usize, 0usize);
    let mut ids = std::collections::HashSet::new();
    for case in dataset.cases {
        if !ids.insert(case.id.clone()) {
            return Err("duplicate case ID".into());
        }
        if started.elapsed() > Duration::from_secs(840) {
            return Err("14 minute evaluation budget exceeded".into());
        }
        let native = select(&case.tools, &catalog)?;
        let defs = definitions(&native)?;
        let mut messages = if live {
            vec![json!({"role":"system","content":REFERENCE_TIME})]
        } else {
            vec![]
        };
        messages.extend(case.messages);
        let mut baseline = json!({"messages":messages,"tools":native});
        if let Some(model) = &model {
            baseline["model"] = json!(model);
            baseline["temperature"] = json!(0);
        }
        let mut req: ChatRequest = serde_json::from_value(baseline.clone())?;
        let session = if wire_supported(&baseline) {
            prepare_request(&mut req, compact_mode).ok()
        } else {
            None
        };
        let compacted = session.is_some();
        let request = if compacted {
            serde_json::to_value(&req)?
        } else {
            baseline.clone()
        };
        let (rendered, roundtrip) = if compacted {
            let rendered = render_calls(&case.expected, &defs)?;
            let roundtrip = decode_calls(&rendered, &defs)?;
            (rendered, roundtrip)
        } else {
            bypasses += 1;
            // Native bypass: only a native JSON serialization round trip, no compact decoder claim.
            let native_text = serde_json::to_string(&case.expected)?;
            let calls = serde_json::from_str(&native_text)?;
            (native_text, calls)
        };
        let before = tokenizer
            .encode_with_special_tokens(&serde_json::to_string(&baseline)?)
            .len();
        let after = tokenizer
            .encode_with_special_tokens(&serde_json::to_string(&request)?)
            .len();
        baseline_tokens += before;
        compact_tokens += after;
        let mut line = json!({
            "id":case.id, "compact_request":request, "compacted":compacted,
            "rendered_calls":rendered, "roundtrip_calls":roundtrip
        });
        if let (Some(http), Some(base)) = (&http, &base) {
            let url = format!("{}/chat/completions", base.trim_end_matches('/'));
            let mut call = http.post(url).json(&request);
            if let Some(key) = &key {
                call = call.bearer_auth(key);
            }
            let response = call.send().await?;
            if !response.status().is_success() {
                // Never log authorization headers or provider error bodies.
                return Err(format!("provider returned HTTP {}", response.status()).into());
            }
            let response: Value = response.json().await?;
            let choice = response["choices"]
                .as_array()
                .and_then(|c| c.first())
                .ok_or("response choice missing")?;
            let raw = choice["message"]["content"].as_str().unwrap_or("");
            line["raw_output"] = json!(raw);
            line["provider_finish_reason"] = choice["finish_reason"].clone();
            line["live_calls"] = if let Some(session) = &session {
                // Exercise the router's response validation too: native calls,
                // missing text, truncation, and all choices must be checked.
                match serde_json::from_value::<nasiko_llm_router::ir::ChatResponse>(
                    response.clone(),
                ) {
                    Ok(mut restored) => match session.restore(&mut restored) {
                        Ok(()) => decoded(native_calls(&serde_json::to_value(restored)?)),
                        Err(error) => json!({"error":error.code()}),
                    },
                    Err(_) => json!({"error":"malformed_output"}),
                }
            } else {
                decoded(native_calls(&response))
            };
            line["model"] = response.get("model").cloned().unwrap_or(Value::Null);
            line["usage"] = response.get("usage").cloned().unwrap_or(Value::Null);
        }
        writeln!(out, "{line}")?;
    }
    for case in dataset.decoder_cases {
        if !ids.insert(case.id.clone()) {
            return Err("duplicate case ID".into());
        }
        let defs = definitions(&select(&case.tools, &catalog)?)?;
        let result = (|| {
            let mut stream = StreamDecoder::new(&defs)?;
            for chunk in &case.chunks {
                stream.push(chunk)?;
            }
            stream.finish()
        })();
        writeln!(out, "{}", json!({"id":case.id,"decoded":decoded(result)}))?;
    }
    out.flush()?;
    eprintln!(
        "cases={} baseline_tokens={} compact_tokens={} bypasses={} live={}",
        ids.len(),
        baseline_tokens,
        compact_tokens,
        bypasses,
        live
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_native_arguments_are_a_case_error_not_a_runner_abort() {
        let response = json!({"choices":[{"message":{"tool_calls":[{"function":{
            "name":"event", "arguments":"{\"title\":\"Review\""
        }}]}}]});
        assert_eq!(
            decoded(native_calls(&response)),
            json!({"error":"invalid_arguments"})
        );
    }
}

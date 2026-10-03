//! Official P1 evaluator: deterministic offline JSONL, optional OpenAI-compatible live mode.
//!
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl cargo run --release \
//!   -p nasiko-llm-router --example compact_tools_eval
//! PROVIDER_BASE_URL and MODEL together enable live calls; PROVIDER_API_KEY is optional.

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, render_call,
};
use serde_json::{Value, json};

fn to_def(raw: &Value) -> Result<ToolDef, Box<dyn std::error::Error>> {
    let function = raw.get("function").ok_or("missing function")?;
    Ok(ToolDef {
        name: function
            .get("name")
            .and_then(Value::as_str)
            .ok_or("missing tool name")?
            .to_string(),
        description: function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        parameters: function.get("parameters").cloned(),
    })
}

fn selected<'a>(
    names: &Value,
    catalog: &'a HashMap<String, Value>,
) -> Result<Vec<&'a Value>, Box<dyn std::error::Error>> {
    let names = names.as_array().ok_or("case tools must be an array")?;
    names
        .iter()
        .map(|name| {
            let name = name.as_str().ok_or("tool name must be a string")?;
            catalog
                .get(name)
                .ok_or_else(|| format!("unknown dataset tool: {name}").into())
        })
        .collect()
}

fn calls_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|call| {
                json!({
                    "name":call.name, "arguments":call.arguments
                })
            })
            .collect(),
    )
}

fn decode_result(result: nasiko_tool_compact::Result<Vec<ToolCall>>) -> Value {
    match result {
        Ok(calls) => json!({"calls":calls_json(&calls)}),
        Err(CompactError::UnknownTool(_)) => json!({"error":"unknown_tool"}),
        Err(_) => json!({"error":"invalid_arguments"}),
    }
}

fn request_for(
    case: &Value,
    native_tools: &[&Value],
    definitions: &[ToolDef],
    model: &str,
) -> (Value, bool) {
    let messages = case.get("messages").cloned().unwrap_or_else(|| json!([]));
    match encode_tools(definitions) {
        Ok(compact)
            if !native_tools.is_empty()
                && native_tools.iter().all(|t| {
                    t.get("type").and_then(Value::as_str) == Some("function")
                        && t.as_object().is_some_and(|o| o.len() == 2)
                        && t.get("function")
                            .and_then(Value::as_object)
                            .is_some_and(|f| {
                                f.keys().all(|key| {
                                    matches!(key.as_str(), "name" | "description" | "parameters")
                                })
                            })
                }) =>
        {
            let instruction = format!(
                "Today: 2026-10-02 Asia/Kolkata. Tools:\n{}\nUse <<call name {{JSON arguments}}>> for each call; else answer normally.",
                compact.text
            );
            let mut out_messages = vec![json!({"role":"system","content":instruction})];
            out_messages.extend(messages.as_array().cloned().unwrap_or_default());
            (
                json!({"model":model,"messages":out_messages,"temperature":0,"stream":false}),
                true,
            )
        }
        _ => (
            json!({"model":model,"messages":messages,"tools":native_tools,"temperature":0,"stream":false}),
            false,
        ),
    }
}

async fn live_output(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    request: &Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let url = if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{}/chat/completions", base.trim_end_matches('/'))
    };
    let mut builder = client.post(url).json(request);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    let response = builder
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(response)
}

fn native_live_calls(response: &Value) -> Value {
    let Some(calls) = response
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
    else {
        return json!({"calls":[]});
    };
    let mut parsed = Vec::with_capacity(calls.len());
    for call in calls {
        let Some(name) = call.pointer("/function/name").and_then(Value::as_str) else {
            return json!({"error":"invalid_arguments"});
        };
        let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) else {
            return json!({"error":"invalid_arguments"});
        };
        let Ok(arguments) = serde_json::from_str::<Value>(arguments) else {
            return json!({"error":"invalid_arguments"});
        };
        parsed.push(ToolCall {
            name: name.to_string(),
            arguments,
        });
    }
    json!({"calls":calls_json(&parsed)})
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".into());
    let output = env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".into());
    let dataset: Value = serde_json::from_reader(File::open(eval_set)?)?;
    let mut writer = BufWriter::new(File::create(output)?);
    let catalog = dataset["tools"]
        .as_array()
        .ok_or("dataset tools missing")?
        .iter()
        .map(|raw| {
            let name = raw
                .pointer("/function/name")
                .and_then(Value::as_str)
                .ok_or("tool name missing")?;
            Ok((name.to_string(), raw.clone()))
        })
        .collect::<Result<HashMap<_, _>, Box<dyn std::error::Error>>>()?;
    let base = env::var("PROVIDER_BASE_URL").ok();
    let model = env::var("MODEL").ok();
    let live = base.is_some() && model.is_some();
    if base.is_some() != model.is_some() {
        return Err("set PROVIDER_BASE_URL and MODEL together".into());
    }
    let model_id = model.as_deref().unwrap_or("gpt-4o-mini");
    let api_key = env::var("PROVIDER_API_KEY").ok();
    let timeout = env::var("REQUEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout))
        .build()?;
    let tokenizer = tiktoken_rs::o200k_base()?;
    let mut native_tokens = 0usize;
    let mut compact_tokens = 0usize;

    for case in dataset["cases"].as_array().ok_or("dataset cases missing")? {
        let native_tools = selected(&case["tools"], &catalog)?;
        let definitions = native_tools
            .iter()
            .map(|raw| to_def(raw))
            .collect::<Result<Vec<_>, _>>()?;
        let (request, compacted) = request_for(case, &native_tools, &definitions, model_id);
        let expected = case["expected"].as_array().ok_or("case expected missing")?;
        let calls = expected
            .iter()
            .map(|item| -> Result<ToolCall, Box<dyn std::error::Error>> {
                Ok(ToolCall {
                    name: item["name"]
                        .as_str()
                        .ok_or("expected name missing")?
                        .to_string(),
                    arguments: item["arguments"].clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rendered = if compacted {
            calls.iter().map(render_call).collect::<Vec<_>>().join("\n")
        } else {
            String::new()
        };
        let roundtrip = if compacted {
            decode_calls(&rendered, &definitions)?
        } else {
            // Native fallback keeps the original call contract; no compact decoder runs.
            calls.clone()
        };
        let mut line = json!({
            "id":case["id"], "compact_request":request,
            "compacted":compacted, "rendered_calls":rendered,
            "roundtrip_calls":calls_json(&roundtrip)
        });
        let native = json!({
            "model":model_id,"messages":case["messages"],"tools":native_tools,
            "temperature":0,"stream":false
        });
        native_tokens += tokenizer.encode_ordinary(&native.to_string()).len();
        compact_tokens += tokenizer.encode_ordinary(&request.to_string()).len();
        if live {
            let response = live_output(
                &client,
                base.as_deref().unwrap(),
                api_key.as_deref(),
                &request,
            )
            .await?;
            let raw = response
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            line["live_calls"] = if compacted {
                decode_result(decode_calls(&raw, &definitions))
            } else {
                native_live_calls(&response)
            };
            line["raw_output"] = Value::String(raw);
        }
        writeln!(writer, "{line}")?;
    }

    for case in dataset["decoder_cases"]
        .as_array()
        .ok_or("dataset decoder_cases missing")?
    {
        let native_tools = selected(&case["tools"], &catalog)?;
        let definitions = native_tools
            .iter()
            .map(|raw| to_def(raw))
            .collect::<Result<Vec<_>, _>>()?;
        let mut decoder = StreamDecoder::new(&definitions)?;
        let mut calls = Vec::new();
        let mut failure = None;
        for chunk in case["chunks"].as_array().ok_or("decoder chunks missing")? {
            let chunk = chunk.as_str().ok_or("decoder chunk is not a string")?;
            match decoder.push(chunk) {
                Ok(found) => calls.extend(found),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let decoded = if let Some(error) = failure {
            decode_result(Err(error))
        } else {
            match decoder.finish() {
                Ok(found) => {
                    calls.extend(found);
                    decode_result(Ok(calls))
                }
                Err(error) => decode_result(Err(error)),
            }
        };
        writeln!(writer, "{}", json!({"id":case["id"],"decoded":decoded}))?;
    }
    writer.flush()?;
    eprintln!("o200k_base request tokens: native={native_tokens}, submitted={compact_tokens}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_schema_keeps_native_tool_request() {
        let native = json!({"type":"function","function":{
            "name":"lookup",
            "parameters":{"type":"object","properties":{"x":{"oneOf":[{"type":"string"}]}}}
        }});
        let definitions = vec![to_def(&native).unwrap()];
        let case = json!({"messages":[{"role":"user","content":"lookup x"}]});
        let (request, compacted) = request_for(&case, &[&native], &definitions, "test-model");
        assert!(!compacted);
        assert_eq!(request["tools"][0], native);
        assert_eq!(request["messages"], case["messages"]);
    }

    #[test]
    fn provider_specific_function_options_bypass_compaction() {
        let native = json!({"type":"function","function":{
            "name":"lookup", "strict":true,
            "parameters":{"type":"object","properties":{}}
        }});
        let definitions = vec![to_def(&native).unwrap()];
        let case = json!({"messages":[{"role":"user","content":"lookup"}]});
        let (request, compacted) = request_for(&case, &[&native], &definitions, "test-model");
        assert!(!compacted);
        assert_eq!(request["tools"][0], native);
    }
}

//! Official P1 evaluator: deterministic offline JSONL, optional OpenAI-compatible live mode.
//!
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl cargo run --release \
//!   -p nasiko-llm-router --example compact_tools_eval
//! PROVIDER_BASE_URL and MODEL together enable live calls; PROVIDER_API_KEY is optional.

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

use nasiko_llm_router::tool_selection::{SelectionMode, select_request};
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
    // Explicit evaluator opt-in: the official invocation remains offline Phase 1
    // even if a deployment has TOOL_SELECTION_MODE or credentials configured.
    let eval_selection = env::var("EVAL_SELECTION_MODE")
        .unwrap_or_else(|_| "off".into())
        .parse::<SelectionMode>()?;
    let mut selection_cfg = nasiko_llm_router::GatewayConfig::from_env().tool_selection;
    selection_cfg.mode = eval_selection;

    for case in dataset["cases"].as_array().ok_or("dataset cases missing")? {
        let all_native_tools = selected(&case["tools"], &catalog)?;
        let all_definitions = all_native_tools
            .iter()
            .map(|raw| to_def(raw))
            .collect::<Result<Vec<_>, _>>()?;
        let baseline_request = request_for(case, &all_native_tools, &all_definitions, model_id).0;
        let selection_request = serde_json::from_value(json!({
            "model":model_id,"messages":case["messages"],"tools":all_native_tools
        }))?;
        let outcome = select_request(&selection_request, &selection_cfg, &client, None).await;
        let native_tools = if let Some(outcome) = &outcome {
            outcome
                .indices
                .iter()
                .map(|&i| all_native_tools[i])
                .collect::<Vec<_>>()
        } else {
            all_native_tools.clone()
        };
        let definitions = native_tools
            .iter()
            .map(|raw| to_def(raw))
            .collect::<Result<Vec<_>, _>>()?;
        let (mut request, mut compacted) = request_for(case, &native_tools, &definitions, model_id);
        if outcome.as_ref().is_some_and(|o| o.native_fallback) {
            request = json!({"model":model_id,"messages":case["messages"],"tools":all_native_tools,"temperature":0,"stream":false});
            compacted = false;
        }
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
        let roundtrip_result = if compacted {
            decode_calls(&rendered, &definitions)
        } else {
            // Native fallback keeps the original call contract; no compact decoder runs.
            Ok(calls.clone())
        };
        if outcome.is_none() && roundtrip_result.is_err() {
            return Err(roundtrip_result.err().unwrap().into());
        }
        let roundtrip_ok = roundtrip_result.is_ok();
        let roundtrip = roundtrip_result.unwrap_or_default();
        let mut line = json!({
            "id":case["id"], "compact_request":request,
            "compacted":compacted, "rendered_calls":rendered,
            "roundtrip_calls":calls_json(&roundtrip)
        });
        let native = json!({
            "model":model_id,"messages":case["messages"],"tools":all_native_tools,
            "temperature":0,"stream":false
        });
        let native_count = tokenizer.encode_ordinary(&native.to_string()).len();
        let submitted_count = tokenizer.encode_ordinary(&request.to_string()).len();
        native_tokens += native_count;
        compact_tokens += submitted_count;
        if let Some(outcome) = &outcome {
            let required = required_names(case)?;
            let selected_names = definitions
                .iter()
                .map(|t| t.name.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            let retained = required
                .iter()
                .filter(|n| selected_names.contains(n.as_str()))
                .count();
            let overhead = if matches!(eval_selection, SelectionMode::Deterministic) {
                Some(0u64)
            } else {
                outcome
                    .telemetry
                    .selection_usage
                    .as_ref()
                    .and_then(|u| u.input_tokens.checked_add(u.output_tokens))
            };
            let baseline_count = tokenizer
                .encode_ordinary(&baseline_request.to_string())
                .len();
            let mut telemetry = serde_json::to_value(&outcome.telemetry)?;
            // Offline rows must be byte-deterministic; timing is only reported live.
            if eval_selection == SelectionMode::Deterministic {
                telemetry["selection_latency_ms"] = Value::Null;
            }
            line["selection"] = json!({
                "telemetry":telemetry,
                "probabilities":outcome.probabilities,
                "selected_names":selected_names,
                "ground_truth_source":if case.get("required_tools").is_some() {"required_tools"} else {"expected"},
                "required_tools":required,
                "false_exclusions":required.len()-retained,
                "false_inclusions":selected_names.len()-retained,
                "recall":ratio(retained,required.len()),"precision":ratio(retained,selected_names.len()),
                "tool_count_reduction":1.0 - selected_names.len() as f64 / all_native_tools.len() as f64,
                "tokenizer":"o200k_base","native_request_tokens":native_count,
                "phase1_request_tokens":baseline_count,"submitted_request_tokens":submitted_count,
                "gross_request_reduction":1.0 - submitted_count as f64 / native_count as f64,
                "request_tokens_saved_over_phase1":baseline_count as i64-submitted_count as i64,
                "selector_tokens":overhead,
                "net_request_tokens_saved":overhead.map(|n| native_count as i128-submitted_count as i128-i128::from(n)),
                "roundtrip_ok":roundtrip_ok,
                "model_adherence":Value::Null
            });
            eprintln!(
                "phase2 {}: tools {} -> {}, required retained {}/{}, fallback={:?}, request tokens {} -> {}, selector tokens={:?}",
                case["id"].as_str().unwrap_or("unknown"),
                all_native_tools.len(),
                selected_names.len(),
                retained,
                required.len(),
                outcome.telemetry.fallback,
                native_count,
                submitted_count,
                overhead
            );
        }
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
            if outcome.is_some() {
                line["selection"]["model_adherence"] =
                    json!(matches_expected(case, &line["live_calls"]["calls"]));
            }
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

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

fn matches_expected(case: &Value, decoded: &Value) -> bool {
    let Some(actual) = decoded.as_array() else {
        return false;
    };
    let Some(expected) = case["expected"].as_array() else {
        return false;
    };
    let ignored = case
        .pointer("/match/free_text_fields")
        .and_then(Value::as_array);
    let canonical = |calls: &[Value]| {
        let mut calls = calls.to_vec();
        for call in &mut calls {
            if let Some(arguments) = call["arguments"].as_object_mut() {
                for field in ignored.into_iter().flatten().filter_map(Value::as_str) {
                    arguments.remove(field);
                }
            }
        }
        let mut serialized = calls.iter().map(Value::to_string).collect::<Vec<_>>();
        serialized.sort();
        serialized
    };
    canonical(actual) == canonical(expected)
}

/// Ground truth is used only after selection, never in request construction.
fn required_names(
    case: &Value,
) -> Result<std::collections::BTreeSet<String>, Box<dyn std::error::Error>> {
    if let Some(names) = case.get("required_tools") {
        return names
            .as_array()
            .ok_or("required_tools must be an array")?
            .iter()
            .map(|name| {
                name.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "required tool must be a name".into())
            })
            .collect();
    }
    case["expected"]
        .as_array()
        .ok_or("case expected missing")?
        .iter()
        .map(|call| {
            call["name"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "expected name missing".into())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adherence_uses_official_free_text_matching_rules() {
        let case = json!({"expected":[{"name":"calendar","arguments":{"title":"Review","duration":30}}],"match":{"free_text_fields":["title"]}});
        assert!(matches_expected(
            &case,
            &json!([{"name":"calendar","arguments":{"title":"Design review","duration":30}}])
        ));
        assert!(!matches_expected(
            &case,
            &json!([{"name":"calendar","arguments":{"title":"Review","duration":10}}])
        ));
        assert!(!matches_expected(&case, &Value::Null));
    }

    #[test]
    fn ground_truth_is_dataset_derived_and_empty_truth_is_not_perfect_recall() {
        let case = json!({"expected":[{"name":"calendar","arguments":{}}]});
        assert_eq!(
            required_names(&case).unwrap(),
            std::collections::BTreeSet::from(["calendar".into()])
        );
        assert_eq!(ratio(0, 0), None);
        assert!(required_names(&json!({"required_tools":[1],"expected":[]})).is_err());
    }

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

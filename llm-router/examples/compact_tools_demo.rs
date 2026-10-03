//! Real provider calls and real local CPU tools. No synthetic model response.
//! PROVIDER_BASE_URL=<.../v1> MODEL=<id> [OPENAI_API_KEY=<secret>]
//! cargo run --release -p nasiko-llm-router --example compact_tools_demo
use nasiko_llm_router::{
    compact_tools::prepare_request,
    ir::{ChatRequest, ChatResponse},
};
use nasiko_tool_compact::{ToolCall, ToolDef, render_calls};
use serde_json::{Value, json};
use std::{error::Error, time::Duration};

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "sum_numbers".into(),
            description: Some("Compute the sum of the supplied numbers. Use this tool for a requested sum; provide each input number, not the calculated result.".into()),
            parameters: json!({"type":"object","properties":{
                "numbers":{"type":"array","minItems":1,"maxItems":64,"items":{"type":"number","minimum":-1000000000,"maximum":1000000000},"description":"Numbers to add, preserving the user's supplied values"}
            },"required":["numbers"],"additionalProperties":false}),
        },
        ToolDef {
            name: "count_text".into(),
            description: Some("Count Unicode scalar values and UTF-8 bytes in the exact supplied text. Execute this tool for a requested character count; do not calculate the count yourself.".into()),
            parameters: json!({"type":"object","properties":{
                "text":{"type":"string","maxLength":4096,"description":"Exact input text, including spaces, quotes, and Unicode"}
            },"required":["text"],"additionalProperties":false}),
        },
    ]
}

fn execute(calls: &[ToolCall], tools: &[ToolDef]) -> Result<Vec<Value>, Box<dyn Error>> {
    // Validate the complete batch before any execution, including native mode.
    render_calls(calls, tools)?;
    calls
        .iter()
        .map(|call| {
            let result = match call.name.as_str() {
                "sum_numbers" => json!({"sum":call.arguments["numbers"].as_array().unwrap()
                .iter().map(|v| v.as_f64().unwrap()).sum::<f64>()}),
                "count_text" => {
                    let text = call.arguments["text"].as_str().unwrap();
                    json!({"unicode_scalars":text.chars().count(),"utf8_bytes":text.len()})
                }
                _ => return Err("tool is not executable in this demo".into()),
            };
            Ok(json!({"name":call.name,"arguments":call.arguments,"result":result}))
        })
        .collect()
}

fn calls(response: &ChatResponse) -> Result<Vec<ToolCall>, Box<dyn Error>> {
    let choice = response.choices.first().ok_or("missing response choice")?;
    choice
        .message
        .tool_calls
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|call| {
            if call.kind != "function" {
                return Err("unexpected tool kind".into());
            }
            Ok(ToolCall {
                name: call.function.name.clone(),
                arguments: serde_json::from_str(&call.function.arguments)?,
            })
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let base = std::env::var("PROVIDER_BASE_URL").map_err(|_| "set PROVIDER_BASE_URL")?;
    let model = std::env::var("MODEL").map_err(|_| "set MODEL")?;
    let compact = match std::env::var("COMPACT_TOOLS_EVAL_MODE").as_deref() {
        Ok("native") => false,
        Ok("compact") | Err(_) => true,
        _ => return Err("COMPACT_TOOLS_EVAL_MODE must be compact or native".into()),
    };
    let prompt = std::env::var("DEMO_PROMPT").unwrap_or_else(|_| {
        "Sum numbers [2,3,5]. Then count the Unicode characters in exactly \"Hi 中文\".".into()
    });
    let defs = tools();
    let native: Vec<Value> = defs
        .iter()
        .map(|tool| {
            json!({"type":"function","function":{
                "name":tool.name,"description":tool.description,"parameters":tool.parameters
            }})
        })
        .collect();
    let mut request: ChatRequest = serde_json::from_value(json!({"model":model,"temperature":0,
        "messages":[{"role":"user","content":prompt}],"tools":native}))?;
    let session = prepare_request(&mut request, compact).ok();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .build()?;
    let mut upstream = http
        .post(format!("{}/chat/completions", base.trim_end_matches('/')))
        .json(&request);
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        upstream = upstream.bearer_auth(key);
    }
    let upstream = upstream.send().await?;
    if !upstream.status().is_success() {
        return Err(format!("provider returned HTTP {}", upstream.status()).into());
    }
    let mut response: ChatResponse = upstream.json().await?;
    let raw_text = response
        .choices
        .first()
        .and_then(|c| c.message.content.clone());
    if let Some(session) = &session {
        session.restore(&mut response)?;
    }
    let calls = calls(&response)?;
    let results = execute(&calls, &defs)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "model":response.model,"compacted":session.is_some(),"raw_output":raw_text,
            "usage":response.usage,"status":if results.is_empty(){"no_calls"}else{"executed"},
            "executions":results,
            "answer":response.choices.first().and_then(|c| c.message.content.clone())
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn executes_real_arithmetic_and_unicode_counts_after_validation() {
        let calls = vec![
            ToolCall {
                name: "sum_numbers".into(),
                arguments: json!({"numbers":[2,3,5]}),
            },
            ToolCall {
                name: "count_text".into(),
                arguments: json!({"text":"Hi 中文"}),
            },
        ];
        let results = execute(&calls, &tools()).unwrap();
        assert_eq!(results[0]["result"], json!({"sum":10.0}));
        assert_eq!(
            results[1]["result"],
            json!({"unicode_scalars":5,"utf8_bytes":9})
        );
    }
    #[test]
    fn invalid_batch_cannot_produce_partial_results() {
        let calls = vec![
            ToolCall {
                name: "sum_numbers".into(),
                arguments: json!({"numbers":[2,3,5]}),
            },
            ToolCall {
                name: "count_text".into(),
                arguments: json!({"text":17}),
            },
        ];
        assert!(execute(&calls, &tools()).is_err());
    }
}

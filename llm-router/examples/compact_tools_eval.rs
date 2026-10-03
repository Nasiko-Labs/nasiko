use nasiko_tool_compact::{
    StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{env, fs, path::PathBuf};

#[derive(Debug, Deserialize)]
struct EvalSet {
    #[serde(default)]
    tools: Vec<Value>,
    #[serde(default)]
    cases: Vec<Case>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ExpectedCall>,
    #[serde(default)]
    r#match: Option<MatchSpec>,
}

#[derive(Debug, Deserialize)]
struct MatchSpec {
    #[serde(default)]
    free_text_fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[serde(default)]
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: Value,
}

#[derive(Debug, Serialize)]
struct OutputLine {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    compact_request: Option<Value>,
    compacted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rendered_calls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    roundtrip_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoded: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compact_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_calls: Option<Value>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = env::var("EVAL_SET").unwrap_or_else(|_| "compact-tools-eval.json".into());
    let output = env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());

    let root: EvalSet = serde_json::from_str(&fs::read_to_string(&input)?)?;
    let tools = parse_tools(&root.tools)?;

    let tokenizer = tiktoken_rs::o200k_base()?;
    let mut lines = Vec::new();

    for case in root.cases {
        let selected = select_tools(&tools, &case.tools);
        let encoded = encode_tools(&selected);

        let Ok(compact) = encoded else {
            lines.push(OutputLine {
                id: case.id,
                compact_request: None,
                compacted: false,
                rendered_calls: None,
                roundtrip_calls: None,
                decoded: None,
                baseline_tokens: None,
                compact_tokens: None,
                raw_output: None,
                live_calls: None,
            });
            continue;
        };

        let rendered = render_expected(&case.expected);
        let roundtrip = decode_calls(&rendered, &selected)?;

        let mut compact_messages = Vec::with_capacity(case.messages.len() + 1);
        compact_messages.push(json!({
            "role":"system",
            "content": format!(
                "Today is 2026-10-02. Timezone: Asia/Kolkata.\n\n{}",
                compact.prompt()
            )
        }));
        compact_messages.extend(case.messages.clone());

        let compact_request = json!({
            "model": env::var("MODEL").unwrap_or_else(|_| "compact-tools-eval".into()),
            "messages": compact_messages,
            "temperature": 0
        });

        let baseline_request = json!({
            "model": env::var("MODEL").unwrap_or_else(|_| "compact-tools-eval".into()),
            "messages": case.messages,
            "tools": selected.iter().map(native_tool).collect::<Vec<_>>(),
            "temperature": 0
        });

        let baseline_text = serde_json::to_string(&baseline_request)?;
        let compact_text = serde_json::to_string(&compact_request);

        let mut raw_output = None;
        let mut live_calls = None;

        if let (Ok(base), Ok(model)) = (env::var("PROVIDER_BASE_URL"), env::var("MODEL")) {
            let client = reqwest::Client::new();
            let mut request = client
                .post(base.trim_end_matches('/').to_string())
                .json(&compact_request);
            if let Ok(key) = env::var("PROVIDER_API_KEY") {
                request = request.bearer_auth(key);
            }
            let response: Value = request.send().await?.json().await?;
            let text = response["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            raw_output = Some(text.clone());
            live_calls = Some(match decode_calls(&text, &selected) {
                Ok(calls) => json!({"calls": calls}),
                Err(e) => json!({"error": e.to_string()}),
            });
            let _ = model;
        }

        lines.push(OutputLine {
            id: case.id,
            compact_request: Some(compact_request),
            compacted: true,
            rendered_calls: Some(rendered),
            roundtrip_calls: Some(json!({
                "calls": roundtrip
            })),
            decoded: None,
            baseline_tokens: Some(tokenizer.encode_with_special_tokens(&baseline_text).len()),
            compact_tokens: Some(tokenizer.encode_with_special_tokens(&compact_text?).len()),
            raw_output,
            live_calls,
        });
    }

    for case in root.decoder_cases {
        let selected = select_tools(&tools, &case.tools);
        let mut decoder = StreamDecoder::new(selected);
        let mut calls = Vec::<ToolCall>::new();
        let mut error = None;

        for chunk in case.chunks {
            match decoder.push(&chunk) {
                Ok(mut new_calls) => calls.append(&mut new_calls),
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        if error.is_none() {
            match decoder.finish() {
                Ok(mut new_calls) => calls.append(&mut new_calls),
                Err(e) => error = Some(e.to_string()),
            }
        }

        let decoded = match error {
            Some(e) => json!({"error": error_code(&e)}),
            None => json!({"calls": calls}),
        };

        lines.push(OutputLine {
            id: case.id,
            compact_request: None,
            compacted: true,
            rendered_calls: None,
            roundtrip_calls: None,
            decoded: Some(decoded),
            baseline_tokens: None,
            compact_tokens: None,
            raw_output: None,
            live_calls: None,
        });

        // Keep `expected` consumed/validated by serde without hard-coding cases.
        let _ = case.expected;
    }

    let mut out = String::new();
    for line in lines {
        out.push_str(&serde_json::to_string(&line)?);
        out.push('\n');
    }
    fs::write(PathBuf::from(output), out)?;
    Ok(())
}

fn parse_tools(values: &[Value]) -> Result<Vec<ToolDef>, Box<dyn std::error::Error>> {
    values
        .iter()
        .map(|v| {
            let function = v.get("function").unwrap_or(v);
            Ok(ToolDef {
                name: function["name"]
                    .as_str()
                    .ok_or("tool missing name")?
                    .to_string(),
                description: function["description"].as_str().map(str::to_string),
                parameters: function.get("parameters").cloned(),
            })
        })
        .collect()
}

fn select_tools(all: &[ToolDef], names: &[String]) -> Vec<ToolDef> {
    names
        .iter()
        .filter_map(|name| all.iter().find(|t| &t.name == name).cloned())
        .collect()
}

fn native_tool(tool: &ToolDef) -> Value {
    json!({
        "type":"function",
        "function":{
            "name":tool.name,
            "description":tool.description,
            "parameters":tool.parameters
        }
    })
}

fn render_expected(calls: &[ExpectedCall]) -> String {
    calls
        .iter()
        .map(|c| {
            format!(
                "<<call {} {}>>",
                c.name,
                serde_json::to_string(&c.arguments).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn error_code(error: &str) -> &'static str {
    if error.starts_with("unknown_tool:") {
        "unknown_tool"
    } else {
        "invalid_arguments"
    }
}

#[allow(dead_code)]
fn _schema_roundtrip_is_available(
    tools: &[ToolDef],
) -> Result<Vec<ToolDef>, Box<dyn std::error::Error>> {
    let compact = encode_tools(tools)?;
    Ok(decode_tools(&compact)?)
}

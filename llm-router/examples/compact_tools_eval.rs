//! Offline token cost evaluation for compact tool schemas.
//!
//! `EVAL_SET` is JSON with an `examples` array. Each row contains `id`, `tools`
//! (the independent `nasiko-tool-compact::ToolDef` shape), and optionally a
//! `prompt`. Results are JSONL written to `OUT`. Set `LIVE=1`, `LIVE_MODEL`,
//! and `OPENAI_API_KEY` to additionally send each prompt/schema to a compatible
//! chat completions endpoint (`OPENAI_BASE_URL` defaults to api.openai.com).
//!
//!   EVAL_SET=/tmp/tools.json OUT=/tmp/compact.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
use std::io::Write;

use nasiko_tool_compact::{ToolDef, decode_calls, encode_tools};
use serde_json::{Value, json};
use tiktoken_rs::{CoreBPE, o200k_base};

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let live = std::env::var("LIVE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let raw = std::fs::read_to_string(path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");
    let tokenizer = o200k_base().expect("load o200k_base tokenizer");
    let client = reqwest::Client::new();
    let mut out = std::io::BufWriter::new(std::fs::File::create(out_path).expect("create OUT"));

    for example in examples {
        let id = example["id"].as_str().expect("example id");
        let prompt = example.get("prompt").and_then(Value::as_str).unwrap_or("");
        let tools: Vec<ToolDef> = serde_json::from_value(example["tools"].clone())
            .expect("tools must match ToolDef shape");
        let encoded = match encode_tools(&tools) {
            Ok(v) => v.definitions,
            Err(e) => {
                write_line(&mut out, json!({"id":id,"error":e.to_string()}));
                continue;
            }
        };
        let full_schema = serde_json::to_string(&tools).expect("serialize tools");
        let original_tokens = tokens(&tokenizer, &format!("{prompt}\n{full_schema}"));
        let compact_tokens = tokens(&tokenizer, &format!("{prompt}\n{encoded}"));
        let saved = original_tokens.saturating_sub(compact_tokens);
        let mut row = json!({
            "id": id, "original_tokens": original_tokens, "compact_tokens": compact_tokens,
            "tokens_saved": original_tokens as i64 - compact_tokens as i64,
            "reduction_ratio": if original_tokens == 0 { 0.0 } else { saved as f64 / original_tokens as f64 },
        });
        if live {
            match live_call(&client, prompt, &encoded).await {
                Ok(answer) => {
                    let calls = decode_calls(&answer, &tools);
                    row["model_output"] = json!(answer);
                    row["decoded_calls"] = match calls {
                        Ok(c) => json!(c),
                        Err(e) => json!({"error":e.to_string()}),
                    };
                    row["output_tokens"] = json!(tokens(&tokenizer, &answer));
                }
                Err(e) => row["live_error"] = json!(e),
            }
        }
        write_line(&mut out, row);
    }
    out.flush().expect("flush OUT");
}

fn tokens(bpe: &CoreBPE, text: &str) -> usize {
    bpe.encode_with_special_tokens(text).len()
}

fn write_line(out: &mut impl Write, value: Value) {
    writeln!(out, "{value}").expect("write OUT");
}

async fn live_call(
    client: &reqwest::Client,
    prompt: &str,
    compact_schema: &str,
) -> Result<String, String> {
    let key =
        std::env::var("OPENAI_API_KEY").map_err(|_| "LIVE=1 requires OPENAI_API_KEY".to_owned())?;
    let model = std::env::var("LIVE_MODEL").map_err(|_| "LIVE=1 requires LIVE_MODEL".to_owned())?;
    let base =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
    let response = client.post(format!("{}/chat/completions", base.trim_end_matches('/')))
        .bearer_auth(key)
        .json(&json!({"model":model,"messages":[{"role":"system","content":format!("Available tools:\n{compact_schema}")},{"role":"user","content":prompt}]}))
        .send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(body.to_string());
    }
    body["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "response did not contain message content".into())
}

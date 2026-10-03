//! Compact tool schemas eval (`[compact-tools]`).
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Offline by default (no network, no keys): deterministic. Writes one JSONL line per case and
//! per decoder case to `OUT`, and a metrics summary to stderr (outputs are the record; the
//! summary is a convenience).
//!
//! Live mode, when `PROVIDER_BASE_URL` and `MODEL` are set: each `compact_request` is sent to
//! `{PROVIDER_BASE_URL}/chat/completions` at temperature 0 (`PROVIDER_API_KEY` is sent as a bearer
//! token only if set) and the line gains `raw_output` and `live_calls`.
//!
//! Optional: `MODEL` also names the model in the request bodies (default `gpt-4o`).
//! `COMPACT_TOOLS=off` bypasses compaction for every case, so live mode measures native tool
//! calling on the same cases (the baseline to compare format adherence against).

use std::{env, fs, process::ExitCode, time::Duration};

use futures::StreamExt;
use nasiko_tool_compact::{Error, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools};
use serde_json::{Value, json};
use tiktoken_rs::o200k_base;

/// Live requests in flight at once.
const CONCURRENCY: usize = 6;

const REFERENCE_TIME: &str = "Today is Friday, 2026-10-02. The user's timezone is Asia/Kolkata (IST, +05:30). \
Resolve relative dates and times in that timezone.";

struct Live {
    client: reqwest::Client,
    base: String,
    key: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("compact_tools_eval: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let path = env::var("EVAL_SET").map_err(|_| "set EVAL_SET to the eval JSON file")?;
    let set: Value =
        serde_json::from_str(&fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?)
            .map_err(|e| format!("{path}: {e}"))?;
    let model = env::var("MODEL").ok().filter(|m| !m.is_empty());
    let live = match (
        env::var("PROVIDER_BASE_URL").ok().filter(|b| !b.is_empty()),
        &model,
    ) {
        (Some(base), Some(_)) => Some(Live {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|e| e.to_string())?,
            base: base.trim_end_matches('/').to_string(),
            key: env::var("PROVIDER_API_KEY").ok().filter(|k| !k.is_empty()),
        }),
        _ => None,
    };
    let model_name = model.clone().unwrap_or_else(|| "gpt-4o".into());
    let native_only = env::var("COMPACT_TOOLS").is_ok_and(|v| v == "off");
    let bpe = o200k_base().map_err(|e| e.to_string())?;
    let count = |v: &Value| bpe.encode_ordinary(&v.to_string()).len();

    let all_tools = set["tools"].as_array().ok_or("eval set has no `tools`")?;
    let pick = |names: &Value| -> Result<(Vec<ToolDef>, Vec<Value>), String> {
        let mut defs = vec![];
        let mut native = vec![];
        for name in names
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let t = all_tools
                .iter()
                .find(|t| t["function"]["name"] == name)
                .ok_or_else(|| format!("unknown tool `{name}` in eval set"))?;
            defs.push(tool_def(t).ok_or("malformed tool in eval set")?);
            native.push(t.clone());
        }
        Ok((defs, native))
    };

    let mut lines: Vec<Value> = vec![];
    let mut pending = vec![];
    let (mut native_tokens, mut compact_tokens) = (0usize, 0usize);
    let (mut cases, mut compacted_cases, mut roundtrip_ok) = (0, 0, 0);
    let (mut decoder_total, mut decoder_ok, mut bad_rejected, mut bad_total) = (0, 0, 0, 0);
    let (mut live_total, mut live_valid, mut live_match) = (0, 0, 0);

    for case in set["cases"].as_array().into_iter().flatten() {
        let (defs, native_tools) = pick(&case["tools"])?;
        let expected = case["expected"].as_array().cloned().unwrap_or_default();
        let messages = case["messages"].as_array().cloned().unwrap_or_default();
        let with_messages = |system: String| {
            let mut m = vec![json!({"role": "system", "content": system})];
            m.extend(messages.clone());
            m
        };
        let native_request = json!({
            "model": model_name, "temperature": 0,
            "messages": with_messages(REFERENCE_TIME.into()), "tools": native_tools,
        });

        let (request, compacted, rendered, roundtrip) = match if native_only {
            Err(Error::Bypass("COMPACT_TOOLS=off".into()))
        } else {
            encode_tools(&defs)
        } {
            Ok(compact) => {
                let request = json!({
                    "model": model_name, "temperature": 0,
                    "messages": with_messages(format!("{REFERENCE_TIME}\n\n{}", compact.prompt)),
                });
                let rendered = render(&expected);
                let roundtrip = outcome(decode_calls(&rendered, &defs));
                (request, true, rendered, roundtrip)
            }
            // Bypass: the native request goes out and native tool calls come back untouched.
            Err(_) => (
                native_request.clone(),
                false,
                String::new(),
                Value::Array(expected.clone()),
            ),
        };
        native_tokens += count(&native_request);
        compact_tokens += count(&request);
        cases += 1;
        compacted_cases += compacted as usize;
        roundtrip_ok += (roundtrip == Value::Array(expected.clone())) as usize;

        let line = json!({
            "id": case["id"], "compact_request": request, "compacted": compacted,
            "rendered_calls": rendered, "roundtrip_calls": roundtrip,
        });
        if live.is_some() {
            let free = case["match"]["free_text_fields"].clone();
            pending.push((
                lines.len(),
                request.clone(),
                compacted,
                defs,
                expected,
                free,
            ));
        }
        lines.push(line);
    }

    for case in set["decoder_cases"].as_array().into_iter().flatten() {
        let (defs, _) = pick(&case["tools"])?;
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let decoded = stream(&defs, &chunks);
        let expected = &case["expected"];
        let ok = match (expected.get("calls"), expected.get("error")) {
            (Some(calls), _) => decoded.get("calls") == Some(calls),
            (_, Some(code)) => decoded.get("error") == Some(code),
            _ => false,
        };
        if expected.get("error").is_some() {
            bad_total += 1;
            bad_rejected += decoded.get("error").is_some() as usize;
        }
        decoder_total += 1;
        decoder_ok += ok as usize;
        lines.push(json!({"id": case["id"], "decoded": decoded}));
    }

    if let Some(live) = &live {
        let done = futures::stream::iter(pending)
            .map(
                |(idx, request, compacted, defs, expected, free)| async move {
                    let (raw, calls) = live_call(live, &request, compacted, &defs).await;
                    (idx, raw, calls, expected, free)
                },
            )
            .buffered(CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        for (idx, raw, calls, expected, free) in done {
            live_total += 1;
            live_valid += names_match(&expected, &calls) as usize;
            live_match += args_match(&expected, &calls, &free) as usize;
            lines[idx]["raw_output"] = raw;
            lines[idx]["live_calls"] = calls;
        }
    }

    let body = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    match env::var("OUT") {
        Ok(out) if !out.is_empty() => fs::write(&out, body).map_err(|e| format!("{out}: {e}"))?,
        _ => print!("{body}"),
    }

    let pct = |a: usize, b: usize| {
        if b == 0 {
            0.0
        } else {
            100.0 * (1.0 - a as f64 / b as f64)
        }
    };
    eprintln!(
        "Token reduction : native {native_tokens} -> compact {compact_tokens} tokens ({:+.1}%)  [o200k_base]",
        -pct(compact_tokens, native_tokens)
    );
    eprintln!(
        "Compacted       : {compacted_cases}/{cases} cases ({} bypassed)",
        cases - compacted_cases
    );
    eprintln!("Round trip      : {roundtrip_ok}/{cases} cases exact");
    eprintln!(
        "Decoder cases   : {decoder_ok}/{decoder_total} correct; {bad_rejected}/{bad_total} bad calls rejected"
    );
    if live.is_some() {
        eprintln!("Live ({model_name})");
        eprintln!(
            "  format valid  : {live_valid}/{live_total} replies decoded to the expected tool names"
        );
        eprintln!(
            "  args match    : {live_match}/{live_total} replies also match the expected arguments (free-text fields: presence/type only)"
        );
    }
    Ok(())
}

fn tool_def(t: &Value) -> Option<ToolDef> {
    let f = t.get("function")?;
    Some(ToolDef {
        name: f.get("name")?.as_str()?.to_string(),
        description: f
            .get("description")
            .and_then(Value::as_str)
            .map(String::from),
        parameters: f.get("parameters").cloned(),
    })
}

/// Expected calls written in the compact grammar.
fn render(expected: &[Value]) -> String {
    expected
        .iter()
        .map(|c| {
            format!(
                "<<call {} {}>>",
                c["name"].as_str().unwrap_or_default(),
                c["arguments"]
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn calls_json(calls: &[ToolCall]) -> Value {
    calls
        .iter()
        .map(|c| json!({"name": c.name, "arguments": serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null)}))
        .collect()
}

/// `[calls]` on success, `{"error": code}` on failure (never a guessed call).
fn outcome(r: Result<Vec<ToolCall>, Error>) -> Value {
    match r {
        Ok(calls) => calls_json(&calls),
        Err(e) => json!({"error": e.code()}),
    }
}

/// Runs a decoder case through `StreamDecoder`, chunk by chunk.
fn stream(defs: &[ToolDef], chunks: &[&str]) -> Value {
    let run = || -> Result<Vec<ToolCall>, Error> {
        let mut decoder = StreamDecoder::new(defs)?;
        let mut calls = vec![];
        for chunk in chunks {
            calls.extend(decoder.push(chunk)?);
        }
        calls.extend(decoder.finish()?);
        Ok(calls)
    };
    match run() {
        Ok(calls) => json!({"calls": calls_json(&calls)}),
        Err(e) => json!({"error": e.code()}),
    }
}

/// The reply decoded without error to the expected sequence of tool names.
fn names_match(expected: &[Value], actual: &Value) -> bool {
    let names = |v: &[Value]| v.iter().map(|c| c["name"].clone()).collect::<Vec<_>>();
    actual
        .as_array()
        .is_some_and(|a| names(a) == names(expected))
}

/// Same calls and arguments (key order ignored); `free_text_fields` need only be present with the same type.
fn args_match(expected: &[Value], actual: &Value, free_text: &Value) -> bool {
    let free: Vec<&str> = free_text
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let same_type = |a: &Value, b: &Value| std::mem::discriminant(a) == std::mem::discriminant(b);
    let Some(actual) = actual.as_array().filter(|a| a.len() == expected.len()) else {
        return false;
    };
    expected.iter().zip(actual).all(|(e, a)| {
        let (ea, aa) = (e["arguments"].as_object(), a["arguments"].as_object());
        e["name"] == a["name"]
            && matches!((ea, aa), (Some(ea), Some(aa)) if ea.len() == aa.len()
                && ea.iter().all(|(k, v)| aa.get(k).is_some_and(|x| if free.contains(&k.as_str()) { same_type(v, x) } else { v == x })))
    })
}

/// Sends one request; returns the model's text and the calls it decodes to.
async fn live_call(
    live: &Live,
    request: &Value,
    compacted: bool,
    defs: &[ToolDef],
) -> (Value, Value) {
    let reply = match send(live, request).await {
        Ok(v) => v,
        Err(e) => {
            return (
                Value::Null,
                json!({"error": format!("request_failed: {e}")}),
            );
        }
    };
    let message = &reply["choices"][0]["message"];
    let text = message["content"].as_str().unwrap_or_default();
    let calls = if compacted {
        outcome(decode_calls(text, defs))
    } else {
        // Native path: the provider already returned standard tool calls.
        message["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| json!({"name": c["function"]["name"], "arguments": c["function"]["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok()).unwrap_or(Value::Null)}))
            .collect()
    };
    (json!(text), calls)
}

async fn send(live: &Live, request: &Value) -> Result<Value, String> {
    let mut req = live
        .client
        .post(format!("{}/chat/completions", live.base))
        .json(request);
    if let Some(key) = &live.key {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("{status}: {}", body["error"]["message"]))
    }
}

//! Eval harness for compact tool schemas (`nasiko-tool-compact`), per the `compact-tools` brief.
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! No arguments. Environment:
//!
//! * `EVAL_SET`: eval file (default `/tmp/compact-tools-eval.json`).
//! * `OUT`: JSONL output path, one line per case (default: stdout).
//! * `COMPACT_INSTRUCTIONS`: call-format guidance after the tool lines.
//!   * `balanced` (default here): one instruction line with a brace-free placeholder and
//!     "Always call tools directly, never ask", with parameter descriptions written as `(text)`.
//!     It had the best live format adherence of the variants tried (tied with `terse`, and
//!     cheaper). That wording nudges the model to call, so the library's own default stays
//!     `minimal`; `balanced` is chosen only here and via this variable.
//!   * `minimal`: the library default; the call instruction plus a notes line.
//!   * `example`: `minimal` plus one sample call line.
//!   * `terse`: the call instruction plus the shortest notes line.
//! * `PROVIDER_BASE_URL` + `MODEL`: enable **live mode**. Each `compact_request` is POSTed to
//!   `{PROVIDER_BASE_URL}/chat/completions` (an OpenAI-compatible base, e.g.
//!   `https://api.openai.com/v1`) at temperature 0, and `raw_output` and `live_calls` are added to
//!   the case's line. `PROVIDER_API_KEY` is optional and sent as a bearer token.
//! * `LIVE_BASELINE=native` (live mode only, off by default): also send the **native** request
//!   for each case (original tools, same reference-time system message, temperature 0) and add
//!   `native_raw` (the assistant message as returned) and `native_calls` (its `tool_calls`, with
//!   arguments parsed). This compares compact and native accuracy on the same cases.
//!
//! Without the live-mode variables the run is offline and deterministic: no network, no keys, and
//! byte-identical `OUT` across runs. Token counts (`o200k_base`, via `tiktoken-rs`) go to stderr
//! only; they are estimates, and the scorer's own counts are what matter.
//!
//! Output lines:
//!
//! * `ct-*`: `{"id","compact_request","compacted","rendered_calls","roundtrip_calls"}`.
//!   * Compacted cases: `rendered_calls` is in the `<<call ..>>` grammar.
//!   * Bypassed cases: `compact_request` is the native request (including the case's
//!     `tool_choice`), and `rendered_calls` is OpenAI `tool_calls` JSON parsed back into
//!     `roundtrip_calls`. These lines also carry `"bypass"` (the reason).
//!   * A failed round trip carries `"roundtrip_error"` with an error code.
//! * `dc-*`: `{"id","decoded":{"calls":[..]}}` or `{"id","decoded":{"error":"<code>"}}`. The
//!   chunks are fed through `StreamDecoder` one at a time. The grammar is the brief's
//!   `<<call NAME {json}>>`, so the chunks are used as given.

use std::io::Write;

use nasiko_tool_compact::{
    CompactTools, EncodeOptions, Instructions, StreamDecoder, ToolCall, ToolDef, decode_calls,
    encode_tools_with, render_call,
};
use serde_json::{Map, Value, json};

/// The fixed reference time, so relative dates resolve the same way for every run. The brief
/// says 2026-10-02; the organizers clarified on event day that it is 2026-10-03. The weekday is
/// derived from the date (2026-10-03 is a Saturday) so the model need not compute it.
const REFERENCE_TIME: &str = "Today is Saturday, 2026-10-03 (Asia/Kolkata, +05:30).";

/// Per-request timeout in live mode.
const LIVE_TIMEOUT_SECS: u64 = 120;

struct Live {
    url: String,
    model: String,
    api_key: Option<String>,
    /// `LIVE_BASELINE=native`: also send each case's native request.
    native_baseline: bool,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("compact_tools_eval: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let eval_path =
        std::env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".into());
    let raw = std::fs::read_to_string(&eval_path).map_err(|e| format!("read {eval_path}: {e}"))?;
    let eval: Value = serde_json::from_str(&raw).map_err(|e| format!("parse {eval_path}: {e}"))?;

    let instructions = match std::env::var("COMPACT_INSTRUCTIONS") {
        Err(_) => Instructions::Balanced,
        Ok(s) => Instructions::parse(&s).ok_or_else(|| {
            format!("COMPACT_INSTRUCTIONS must be minimal|example|terse|balanced, got '{s}'")
        })?,
    };
    let live = live_from_env()?;

    // Tool schemas by name, kept as raw JSON: cases reference them, and a tool the crate cannot
    // carry losslessly must still be sendable natively.
    let mut catalog: Vec<(String, Value)> = Vec::new();
    for t in array(&eval, "tools") {
        if let Some(name) = t.pointer("/function/name").and_then(Value::as_str) {
            catalog.push((name.to_string(), t.clone()));
        }
    }

    let mut out: Box<dyn Write> = match std::env::var("OUT") {
        Ok(path) if !path.is_empty() && path != "-" => Box::new(std::io::BufWriter::new(
            std::fs::File::create(&path).map_err(|e| format!("create {path}: {e}"))?,
        )),
        _ => Box::new(std::io::stdout().lock()),
    };

    let bpe = tiktoken_rs::o200k_base().map_err(|e| format!("load o200k_base: {e}"))?;
    let tokens = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();
    let (mut native_total, mut compact_total, mut bypassed) = (0usize, 0usize, 0usize);

    let cases = array(&eval, "cases");
    for case in cases {
        let id = str_field(case, "id");
        let (native_tools, tool_defs) = resolve_tools(case, &catalog);
        let messages = with_reference_time(array(case, "messages"), None);
        let mut native_body = json!({ "messages": messages, "tools": native_tools });
        if let (Some(choice), Some(obj)) = (case.get("tool_choice"), native_body.as_object_mut()) {
            obj.insert("tool_choice".into(), choice.clone());
        }

        let encoded = match &tool_defs {
            Ok(defs) => encode_tools_with(
                defs,
                &EncodeOptions {
                    tool_choice: case.get("tool_choice"),
                    instructions,
                },
            )
            .map_err(|e| e.to_string()),
            Err((_, detail)) => Err(detail.clone()),
        };
        let (compact_request, bypass) = match &encoded {
            Ok(CompactTools::Compact { prompt }) => {
                let body = json!({ "messages": with_reference_time(array(case, "messages"), Some(prompt)) });
                // Never worse than baseline, measured on the whole request body.
                if body.to_string().len() < native_body.to_string().len() {
                    (body, None)
                } else {
                    (native_body.clone(), Some("not_smaller_request".to_string()))
                }
            }
            Ok(CompactTools::Bypass { reason, .. }) => {
                (native_body.clone(), Some(reason.to_string()))
            }
            Err(e) => (native_body.clone(), Some(e.clone())),
        };
        let compacted = bypass.is_none();

        let (n, c) = (tokens(&native_body), tokens(&compact_request));
        native_total += n;
        compact_total += c;
        if !compacted {
            bypassed += 1;
        }
        eprintln!(
            "{id}: native {n} tok, compact {c} tok{}",
            bypass
                .as_ref()
                .map_or(String::new(), |b| format!(" (bypass: {b})"))
        );

        let expected: Vec<ToolCall> = array(case, "expected")
            .iter()
            .map(|e| ToolCall {
                name: str_field(e, "name"),
                arguments: e
                    .get("arguments")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect();
        // The expected calls in the format this request actually uses, then decoded back: our
        // grammar when compacted, OpenAI `tool_calls` JSON when bypassed.
        let (rendered_calls, roundtrip) = match (&tool_defs, compacted) {
            (Ok(defs), true) => {
                let text = expected
                    .iter()
                    .map(render_call)
                    .collect::<Vec<_>>()
                    .join("\n");
                let back = decode_calls(&text, defs)
                    .map(|d| calls_json(&d.calls))
                    .map_err(|e| eval_code(&e));
                (text, back)
            }
            _ => native_round_trip(&expected),
        };

        let mut line = Map::new();
        line.insert("id".into(), json!(id));
        line.insert("compact_request".into(), compact_request.clone());
        line.insert("compacted".into(), json!(compacted));
        line.insert("rendered_calls".into(), json!(rendered_calls));
        match roundtrip {
            Ok(calls) => {
                line.insert("roundtrip_calls".into(), calls);
            }
            Err(code) => {
                line.insert("roundtrip_calls".into(), json!([]));
                line.insert("roundtrip_error".into(), json!(code));
            }
        }
        if let Some(reason) = &bypass {
            line.insert("bypass".into(), json!(reason));
        }
        if let Some(live) = &live {
            let (raw_output, live_calls) =
                live_call(live, &compact_request, compacted, tool_defs.as_deref().ok());
            line.insert("raw_output".into(), raw_output);
            line.insert("live_calls".into(), live_calls);
            if live.native_baseline {
                let (native_raw, native_calls) = match send_chat(live, &native_body) {
                    Ok(message) => {
                        let calls = native_calls_json(&message);
                        (message, calls)
                    }
                    Err(e) => (
                        Value::Null,
                        json!({ "error": format!("provider_error: {e}") }),
                    ),
                };
                line.insert("native_raw".into(), native_raw);
                line.insert("native_calls".into(), native_calls);
            }
        }
        write_line(&mut out, line)?;
    }

    for case in array(&eval, "decoder_cases") {
        let (_, tool_defs) = resolve_tools(case, &catalog);
        let decoded = match tool_defs {
            Ok(defs) => {
                let mut decoder = StreamDecoder::new(&defs);
                let pushed = array(case, "chunks")
                    .iter()
                    .try_for_each(|chunk| decoder.push(chunk.as_str().unwrap_or_default()));
                let result = pushed.and_then(|()| decoder.finish()).map(|d| d.calls);
                result_json(result.map_err(|e| eval_code(&e).to_string()))
            }
            // Only the code goes to OUT; the detail is for the operator.
            Err((code, detail)) => {
                eprintln!("{}: {detail}", str_field(case, "id"));
                json!({ "error": code })
            }
        };
        let mut line = Map::new();
        line.insert("id".into(), json!(str_field(case, "id")));
        line.insert("decoded".into(), decoded);
        write_line(&mut out, line)?;
    }
    out.flush().map_err(|e| format!("flush OUT: {e}"))?;

    if native_total > 0 {
        eprintln!(
            "total: native {native_total} tok, compact {compact_total} tok, savings {:.1}% \
             ({bypassed}/{} cases bypassed, o200k_base, instructions {instructions:?})",
            100.0 * (1.0 - compact_total as f64 / native_total as f64),
            cases.len(),
        );
    }
    Ok(())
}

/// The eval contract has exactly two error codes. `unknown_tool` passes through; every other
/// failure (including `invalid_tools`, a schema that cannot validate the call) is
/// `invalid_arguments`, since no call can be trusted.
fn eval_code(e: &nasiko_tool_compact::Error) -> &'static str {
    match e.code() {
        "unknown_tool" => "unknown_tool",
        _ => "invalid_arguments",
    }
}

/// Why a case's tools could not become crate `ToolDef`s: an error code for `OUT`
/// (`unknown_tool` / `invalid_arguments`), and a detail for stderr.
type ResolveError = (&'static str, String);

/// The case's tools as native JSON (always) and as crate `ToolDef`s (when representable).
fn resolve_tools(
    case: &Value,
    catalog: &[(String, Value)],
) -> (Vec<Value>, Result<Vec<ToolDef>, ResolveError>) {
    let mut native = Vec::new();
    let mut missing = None;
    for name in array(case, "tools").iter().filter_map(Value::as_str) {
        match catalog.iter().find(|(n, _)| n == name) {
            Some((_, t)) => native.push(t.clone()),
            None => missing = Some(format!("case references unknown tool '{name}'")),
        }
    }
    let defs = match missing {
        Some(detail) => Err(("unknown_tool", detail)),
        // A tool this crate cannot carry cannot validate calls either: fail closed.
        None => native
            .iter()
            .map(|t| ToolDef::from_openai(t).map_err(|e| ("invalid_arguments", e.to_string())))
            .collect(),
    };
    (native, defs)
}

/// Expected calls written as OpenAI `tool_calls` JSON (ids `call_1`, ...; arguments as JSON
/// strings), and that text parsed back to `[{name, arguments}]`. Used for bypassed cases.
fn native_round_trip(expected: &[ToolCall]) -> (String, Result<Value, &'static str>) {
    let tool_calls: Vec<Value> = expected
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "id": format!("call_{}", i + 1),
                "type": "function",
                "function": { "name": c.name, "arguments": c.arguments_json() },
            })
        })
        .collect();
    let text = Value::Array(tool_calls).to_string();
    let back = serde_json::from_str::<Value>(&text)
        .map_err(|_| "invalid_arguments")
        .and_then(|parsed| {
            let decoded = native_calls_json(&json!({ "tool_calls": parsed }));
            decoded.get("calls").cloned().ok_or("invalid_arguments")
        });
    (text, back)
}

/// Prepend the reference-time system message, with the compact tool prompt appended to it when
/// given (one system message, not two: fewer tokens, same effect).
fn with_reference_time(messages: &[Value], compact_prompt: Option<&str>) -> Vec<Value> {
    let content = match compact_prompt {
        Some(p) => format!("{REFERENCE_TIME}\n\n{p}"),
        None => REFERENCE_TIME.to_string(),
    };
    let mut out = vec![json!({ "role": "system", "content": content })];
    out.extend(messages.iter().cloned());
    out
}

fn calls_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| json!({ "name": c.name, "arguments": c.arguments }))
            .collect(),
    )
}

fn result_json(r: Result<Vec<ToolCall>, String>) -> Value {
    match r {
        Ok(calls) => json!({ "calls": calls_json(&calls) }),
        Err(code) => json!({ "error": code }),
    }
}

fn live_from_env() -> Result<Option<Live>, String> {
    let (Ok(base), Ok(model)) = (std::env::var("PROVIDER_BASE_URL"), std::env::var("MODEL")) else {
        return Ok(None);
    };
    if base.is_empty() || model.is_empty() {
        return Ok(None);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(LIVE_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let native_baseline = match std::env::var("LIVE_BASELINE") {
        Err(_) => false,
        Ok(v) if v.is_empty() => false,
        Ok(v) if v == "native" => true,
        Ok(v) => {
            return Err(format!(
                "LIVE_BASELINE must be 'native' or unset, got '{v}'"
            ));
        }
    };
    Ok(Some(Live {
        url: format!("{}/chat/completions", base.trim_end_matches('/')),
        model,
        api_key: std::env::var("PROVIDER_API_KEY")
            .ok()
            .filter(|k| !k.is_empty()),
        native_baseline,
        client,
        runtime,
    }))
}

/// Send one request; return `(raw_output, live_calls)`. Never fails the run: provider errors are
/// reported in `live_calls`.
fn live_call(
    live: &Live,
    request: &Value,
    compacted: bool,
    tools: Option<&[ToolDef]>,
) -> (Value, Value) {
    let message = match send_chat(live, request) {
        Ok(message) => message,
        Err(e) => {
            return (
                Value::Null,
                json!({ "error": format!("provider_error: {e}") }),
            );
        }
    };
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let live_calls = if compacted {
        match tools {
            Some(defs) => result_json(
                decode_calls(&text, defs)
                    .map(|d| d.calls)
                    .map_err(|e| eval_code(&e).to_string()),
            ),
            None => json!({ "error": "invalid_tools" }),
        }
    } else {
        // Bypassed: the model answered with native tool calls.
        native_calls_json(&message)
    };
    (json!(text), live_calls)
}

/// POST one chat request at temperature 0; return the first choice's assistant message.
fn send_chat(live: &Live, request: &Value) -> Result<Value, String> {
    let mut body = request.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".into(), json!(live.model));
        obj.insert("temperature".into(), json!(0));
    }
    let mut req = live.client.post(&live.url).json(&body);
    if let Some(key) = &live.api_key {
        req = req.bearer_auth(key);
    }
    let response: Value = live.runtime.block_on(async {
        let resp = req.send().await.map_err(|e| format!("request: {e}"))?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| format!("body: {e}"))?;
        if status.is_success() {
            Ok(v)
        } else {
            Err(format!("HTTP {status}: {v}"))
        }
    })?;
    Ok(response
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null))
}

/// A native assistant message's `tool_calls` as `{"calls":[{name, arguments}]}`. Arguments that
/// are not valid JSON make the whole result `{"error":"invalid_arguments"}`, as in compact mode.
fn native_calls_json(message: &Value) -> Value {
    let mut calls = Vec::new();
    for tc in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let args = tc
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let Some(args) = args else {
            return json!({ "error": "invalid_arguments" });
        };
        calls.push(json!({ "name": tc.pointer("/function/name"), "arguments": args }));
    }
    json!({ "calls": calls })
}

fn write_line(out: &mut dyn Write, line: Map<String, Value>) -> Result<(), String> {
    writeln!(out, "{}", Value::Object(line)).map_err(|e| format!("write OUT: {e}"))
}

fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

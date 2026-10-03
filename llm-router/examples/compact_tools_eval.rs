//! `[compact-tools]` evaluation harness.
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Writes one JSONL line per case to `OUT` — outputs only; the scorer computes every metric:
//!
//! * `cases`: `compact_request` (the OpenAI chat body that would be sent), `compacted`,
//!   `rendered_calls` (the expected calls in the call grammar), `roundtrip_calls` (that text
//!   decoded back), and `decoded_tools` (the compact definitions parsed back into JSON Schema, so
//!   schema preservation can be checked mechanically). A bypassed case says why in
//!   `bypass_reason` and sends the native request.
//! * `decoder_cases`: `decoded`, the [`StreamDecoder`] result fed chunk by chunk.
//!
//! The grammar is the reference `<<call NAME {json}>>`, so decoder cases are used as given.
//!
//! **Offline by default**: no network, no keys, deterministic (run it twice and diff). Token
//! diagnostics (`o200k_base`, both with and without the reference-date line in the baseline) go
//! to stderr, never to `OUT`.
//!
//! **Live mode** when `PROVIDER_BASE_URL` and `MODEL` are set: each request is sent to that
//! OpenAI-compatible endpoint at temperature 0, adding `raw_output` and `live_calls`.
//! `PROVIDER_API_KEY` is sent as a bearer token when set. `LIVE_NATIVE=1` also sends the native
//! request and records `native_calls`, for an adherence comparison on the same cases.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate};
use nasiko_tool_compact::{
    CompactError, CompactTools, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_reply,
    decode_tools, encode_tools, render_calls,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// The fixed reference time every live run uses, so relative dates resolve the same way.
const REFERENCE_DATE: &str = "2026-10-02";
const TIMEZONE: &str = "Asia/Kolkata";
const UTC_OFFSET: &str = "+05:30";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Live mode stops sending once this is spent, keeping the run inside the 15-minute limit.
const LIVE_BUDGET: Duration = Duration::from_secs(12 * 60);
const RATE_LIMIT_RETRIES: [u64; 3] = [2, 5, 10];

/// The eval file. Cases are read one by one, so one malformed case becomes an error line
/// instead of failing the whole run.
#[derive(Deserialize)]
struct EvalSet {
    #[serde(default)]
    tools: Option<Vec<Value>>,
    #[serde(default)]
    cases: Option<Vec<Value>>,
    #[serde(default)]
    decoder_cases: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct Case {
    #[serde(deserialize_with = "id_text")]
    id: String,
    #[serde(default, deserialize_with = "or_default")]
    tools: Vec<String>,
    #[serde(default, deserialize_with = "or_default")]
    messages: Vec<Value>,
    #[serde(default, deserialize_with = "or_default")]
    expected: Vec<ExpectedCall>,
    #[serde(default, rename = "match", deserialize_with = "or_default")]
    match_rules: MatchRules,
}

#[derive(Deserialize)]
struct ExpectedCall {
    name: String,
    /// An object, or the OpenAI-style JSON string of one.
    #[serde(default)]
    arguments: Value,
}

impl ExpectedCall {
    fn to_call(&self) -> Option<ToolCall> {
        let arguments = match &self.arguments {
            Value::Object(map) => map.clone(),
            Value::String(text) => serde_json::from_str(text).ok()?,
            Value::Null => Map::new(),
            _ => return None,
        };
        Some(ToolCall {
            name: self.name.clone(),
            arguments,
        })
    }
}

#[derive(Deserialize, Default)]
struct MatchRules {
    #[serde(default, deserialize_with = "or_default")]
    free_text_fields: Vec<String>,
}

#[derive(Deserialize)]
struct DecoderCase {
    #[serde(deserialize_with = "id_text")]
    id: String,
    #[serde(default, deserialize_with = "or_default")]
    tools: Vec<String>,
    #[serde(default, deserialize_with = "or_default")]
    chunks: Vec<String>,
    #[serde(default)]
    expected: Option<Value>,
}

/// `null` reads as the field's default.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// Ids are echoed as given; a numeric id is accepted and written as text.
fn id_text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(s) => s,
        other => other.to_string(),
    })
}

fn unreadable(item: &Value, error: serde_json::Error) -> Value {
    let id = match item.get("id") {
        Some(Value::String(s)) => json!(s),
        Some(other) => json!(other.to_string()),
        None => Value::Null,
    };
    json!({"id": id, "error": "unreadable_case", "error_detail": error.to_string()})
}

/// One tool from the file: as the crate reads it, and exactly as written (for native requests).
struct Tool {
    def: Option<ToolDef>,
    raw: Value,
}

#[tokio::main]
async fn main() {
    let (Ok(eval_path), Ok(out_path)) = (std::env::var("EVAL_SET"), std::env::var("OUT")) else {
        eprintln!("usage: EVAL_SET=<eval json> OUT=<output jsonl> compact_tools_eval");
        std::process::exit(2);
    };
    let set: EvalSet = match std::fs::read_to_string(&eval_path)
        .map_err(|e| e.to_string())
        .and_then(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))
    {
        Ok(set) => set,
        Err(e) => {
            eprintln!("compact_tools_eval: cannot read {eval_path}: {e}");
            std::process::exit(1);
        }
    };

    let tools = index_tools(&set.tools.unwrap_or_default());
    let date_line = reference_date_line();
    let mut live = Live::from_env();
    let mut stats = Stats::default();
    let mut lines = Vec::new();

    for item in set.cases.unwrap_or_default() {
        let case: Case = match serde_json::from_value(item.clone()) {
            Ok(case) => case,
            Err(e) => {
                lines.push(unreadable(&item, e));
                continue;
            }
        };
        let mut line = run_case(&case, &tools, &date_line, &mut stats);
        if let Some(live) = live.as_mut() {
            live.run_case(&case, &tools, &date_line, &mut line).await;
        }
        lines.push(line);
    }
    for item in set.decoder_cases.unwrap_or_default() {
        match serde_json::from_value::<DecoderCase>(item.clone()) {
            Ok(case) => lines.push(run_decoder_case(&case, &tools, &mut stats)),
            Err(e) => lines.push(unreadable(&item, e)),
        }
    }

    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    if let Err(e) = std::fs::write(&out_path, body) {
        eprintln!("compact_tools_eval: cannot write {out_path}: {e}");
        std::process::exit(1);
    }
    stats.report();
}

fn index_tools(raw: &[Value]) -> BTreeMap<String, Tool> {
    let mut out = BTreeMap::new();
    for value in raw {
        let Some(name) = value.pointer("/function/name").and_then(Value::as_str) else {
            continue;
        };
        out.insert(
            name.to_string(),
            Tool {
                def: serde_json::from_value(value.clone()).ok(),
                raw: value.clone(),
            },
        );
    }
    out
}

/// "Today is Fri 2026-10-02, Asia/Kolkata (+05:30)." The weekday and offset are derived, not
/// typed: models resolve "next Tuesday" against the weekday, and copy the offset into
/// `date-time` arguments.
fn reference_date_line() -> String {
    let weekday = NaiveDate::parse_from_str(REFERENCE_DATE, "%Y-%m-%d")
        .map(|d| d.weekday().to_string())
        .unwrap_or_default();
    format!("Today is {weekday} {REFERENCE_DATE}, {TIMEZONE} ({UTC_OFFSET}).")
}

/// The case's tools in the order it lists them. Missing or non-function tools make the case
/// ineligible for compaction (the native request still carries what the file declares).
fn case_tools<'a>(names: &[String], tools: &'a BTreeMap<String, Tool>) -> Vec<&'a Tool> {
    names.iter().filter_map(|n| tools.get(n)).collect()
}

fn run_case(
    case: &Case,
    tools: &BTreeMap<String, Tool>,
    date_line: &str,
    stats: &mut Stats,
) -> Value {
    let selected = case_tools(&case.tools, tools);
    let defs: Vec<ToolDef> = selected.iter().filter_map(|t| t.def.clone()).collect();
    let native = native_request(&case.messages, &selected, Some(date_line));
    let baseline_plain = native_request(&case.messages, &selected, None);

    let compacted = match compaction_blocker(case, &selected, &defs) {
        Some(reason) => Err(reason),
        None => encode_tools(&defs).map_err(|e| bypass_label(&e)),
    };
    let mut line = Map::new();
    line.insert("id".into(), json!(case.id));
    let request = match &compacted {
        Ok(compact) => {
            line.insert("compacted".into(), json!(true));
            line.insert("decoded_tools".into(), decoded_tools(compact));
            compact_request(&case.messages, compact, date_line)
        }
        Err(reason) => {
            line.insert("compacted".into(), json!(false));
            line.insert("bypass_reason".into(), json!(reason));
            stats.bypasses.push((case.id.clone(), reason.clone()));
            native.clone()
        }
    };
    stats.add_tokens(&request, &native, &baseline_plain);
    line.insert("compact_request".into(), request);

    let expected: Vec<ToolCall> = case
        .expected
        .iter()
        .filter_map(ExpectedCall::to_call)
        .collect();
    let rendered = render_calls(&expected);
    let roundtrip = decode_calls(&rendered, &defs);
    stats.roundtrip(&case.id, &roundtrip, &expected, &case.match_rules);
    line.insert("rendered_calls".into(), json!(rendered));
    // Always a list, so a consumer can read it without checking its shape; a decode failure is
    // reported beside it.
    match &roundtrip {
        Ok(calls) => {
            line.insert("roundtrip_calls".into(), calls_json(calls));
        }
        Err(e) => {
            line.insert("roundtrip_calls".into(), json!([]));
            line.insert("roundtrip_error".into(), error_json(e));
        }
    }
    Value::Object(line)
}

/// Why a case must go out natively before the schemas are even looked at.
fn compaction_blocker(case: &Case, selected: &[&Tool], defs: &[ToolDef]) -> Option<String> {
    if case.tools.is_empty() {
        return Some("no_tools".into());
    }
    if selected.len() != case.tools.len() || defs.len() != selected.len() {
        return Some("unknown_or_non_function_tool".into());
    }
    let has_tool_history = case.messages.iter().any(|m| {
        m.get("role").and_then(Value::as_str) == Some("tool") || m.get("tool_calls").is_some()
    });
    has_tool_history.then(|| "tool_history".into())
}

fn bypass_label(error: &CompactError) -> String {
    match error {
        CompactError::Unsupported { feature, path, .. } => {
            format!("{}:{path}", feature.as_label())
        }
        other => other.code().to_string(),
    }
}

/// The native request the scorer's baseline corresponds to: the messages and the `tools` array
/// exactly as the file declares them, optionally led by the reference-date system message.
fn native_request(messages: &[Value], tools: &[&Tool], date_line: Option<&str>) -> Value {
    let mut all = Vec::with_capacity(messages.len() + 1);
    if let Some(date_line) = date_line {
        all.push(json!({"role": "system", "content": date_line}));
    }
    all.extend(messages.iter().cloned());
    let raw: Vec<Value> = tools.iter().map(|t| t.raw.clone()).collect();
    if raw.is_empty() {
        // Providers reject an empty `tools` array; a request without tools simply has none.
        return json!({ "messages": all });
    }
    json!({"messages": all, "tools": raw})
}

/// The compact request: one system message carrying the reference date, the call-format
/// instruction and the definitions, ahead of the case's own messages. A case that already opens
/// with a system message gets the block merged in front of it rather than a second one.
fn compact_request(messages: &[Value], compact: &CompactTools, date_line: &str) -> Value {
    let block = format!("{date_line}\n{}", compact.system_prompt());
    let mut out: Vec<Value> = messages.to_vec();
    let leading_system_text = out.first().and_then(|m| {
        (m.get("role").and_then(Value::as_str) == Some("system"))
            .then(|| m.get("content").and_then(Value::as_str))
            .flatten()
            .map(str::to_string)
    });
    match leading_system_text {
        Some(text) => out[0]["content"] = json!(format!("{block}\n\n{text}")),
        None => out.insert(0, json!({"role": "system", "content": block})),
    }
    json!({ "messages": out })
}

fn decoded_tools(compact: &CompactTools) -> Value {
    match decode_tools(compact) {
        Ok(tools) => serde_json::to_value(tools).unwrap_or(Value::Null),
        Err(e) => error_json(&e),
    }
}

/// The one error shape this harness writes: the stable code plus a human-readable detail.
fn error_json(error: &CompactError) -> Value {
    json!({"error": error.code(), "error_detail": error.to_string()})
}

fn calls_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| json!({"name": c.name, "arguments": c.arguments}))
            .collect(),
    )
}

fn run_decoder_case(
    case: &DecoderCase,
    tools: &BTreeMap<String, Tool>,
    stats: &mut Stats,
) -> Value {
    let defs: Vec<ToolDef> = case_tools(&case.tools, tools)
        .iter()
        .filter_map(|t| t.def.clone())
        .collect();
    let result = stream_chunks(&case.chunks, &defs);
    let mut line = Map::new();
    line.insert("id".into(), json!(case.id));
    match &result {
        Ok(calls) => {
            line.insert("decoded".into(), json!({ "calls": calls_json(calls) }));
        }
        Err(e) => {
            line.insert("decoded".into(), json!({ "error": e.code() }));
            line.insert("error_detail".into(), json!(e.to_string()));
        }
    }
    if let Some(expected) = &case.expected {
        stats.decoder(&case.id, &line["decoded"], expected);
    }
    Value::Object(line)
}

fn stream_chunks(chunks: &[String], defs: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut decoder = StreamDecoder::new(defs)?;
    for chunk in chunks {
        decoder.push(chunk)?;
    }
    decoder.finish().map(|done| done.calls)
}

// ── stderr diagnostics (never written to OUT) ──────────────────────────────────

#[derive(Default)]
struct Stats {
    compact_tokens: usize,
    baseline_with_date: usize,
    baseline_plain: usize,
    cases: usize,
    bypasses: Vec<(String, String)>,
    roundtrip_ok: usize,
    roundtrip_notes: Vec<String>,
    decoder_ok: usize,
    decoder_total: usize,
    decoder_notes: Vec<String>,
}

impl Stats {
    fn add_tokens(&mut self, request: &Value, native: &Value, plain: &Value) {
        self.cases += 1;
        self.compact_tokens += tokens(request);
        self.baseline_with_date += tokens(native);
        self.baseline_plain += tokens(plain);
    }

    fn roundtrip(
        &mut self,
        id: &str,
        result: &Result<Vec<ToolCall>, CompactError>,
        expected: &[ToolCall],
        rules: &MatchRules,
    ) {
        let ok = result.as_ref().is_ok_and(|calls| {
            calls.len() == expected.len()
                && calls
                    .iter()
                    .zip(expected)
                    .all(|(got, want)| same_call(got, want, rules))
        });
        if ok {
            self.roundtrip_ok += 1;
        } else {
            self.roundtrip_notes.push(format!("{id}: {result:?}"));
        }
    }

    fn decoder(&mut self, id: &str, got: &Value, expected: &Value) {
        self.decoder_total += 1;
        if got == expected {
            self.decoder_ok += 1;
        } else {
            self.decoder_notes
                .push(format!("{id}: got {got}, expected {expected}"));
        }
    }

    fn report(&self) {
        let pct = |part: usize, whole: usize| {
            if whole == 0 {
                0.0
            } else {
                100.0 * (1.0 - part as f64 / whole as f64)
            }
        };
        eprintln!("compact_tools_eval — local diagnostics (the scorer recomputes everything)");
        eprintln!(
            "  token reduction (o200k_base, compact JSON bodies, {} cases): {:.1}% vs a baseline with the date line, {:.1}% vs messages+tools only",
            self.cases,
            pct(self.compact_tokens, self.baseline_with_date),
            pct(self.compact_tokens, self.baseline_plain),
        );
        eprintln!(
            "  tokens: compact {} / baseline {} (with date) / {} (plain)",
            self.compact_tokens, self.baseline_with_date, self.baseline_plain
        );
        eprintln!("  bypassed: {} case(s)", self.bypasses.len());
        for (id, reason) in &self.bypasses {
            eprintln!("    {id}: {reason}");
        }
        eprintln!(
            "  round trip: {}/{} match expected",
            self.roundtrip_ok, self.cases
        );
        for note in &self.roundtrip_notes {
            eprintln!("    MISMATCH {note}");
        }
        eprintln!(
            "  decoder cases: {}/{} match expected",
            self.decoder_ok, self.decoder_total
        );
        for note in &self.decoder_notes {
            eprintln!("    MISMATCH {note}");
        }
        eprintln!(
            "  caveats: token counts depend on how the scorer serialises the body (compact JSON here); public samples are tiny, so treat percentages as indicative"
        );
    }
}

fn tokens(body: &Value) -> usize {
    tiktoken_rs::o200k_base_singleton()
        .encode_ordinary(&body.to_string())
        .len()
}

/// Name and arguments equal; free-text fields only need to be present with the same JSON type.
fn same_call(got: &ToolCall, want: &ToolCall, rules: &MatchRules) -> bool {
    if got.name != want.name || got.arguments.len() != want.arguments.len() {
        return false;
    }
    want.arguments.iter().all(|(key, want_value)| {
        got.arguments.get(key).is_some_and(|got_value| {
            if rules.free_text_fields.contains(key) {
                std::mem::discriminant(got_value) == std::mem::discriminant(want_value)
            } else {
                got_value == want_value
            }
        })
    })
}

// ── live mode ──────────────────────────────────────────────────────────────────

struct Live {
    http: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    with_native: bool,
    started: Instant,
}

impl Live {
    fn from_env() -> Option<Self> {
        let base = std::env::var("PROVIDER_BASE_URL").ok()?;
        let model = std::env::var("MODEL").ok()?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        let base = base.trim_end_matches('/');
        let endpoint = if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        };
        Some(Self {
            http,
            endpoint,
            model,
            api_key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty()),
            with_native: std::env::var("LIVE_NATIVE").is_ok_and(|v| v == "1"),
            started: Instant::now(),
        })
    }

    async fn run_case(
        &mut self,
        case: &Case,
        tools: &BTreeMap<String, Tool>,
        date_line: &str,
        line: &mut Value,
    ) {
        let selected = case_tools(&case.tools, tools);
        let defs: Vec<ToolDef> = selected.iter().filter_map(|t| t.def.clone()).collect();
        let compacted = line["compacted"].as_bool().unwrap_or(false);
        let request = line["compact_request"].clone();
        match self.send(request).await {
            Ok(reply) => {
                line["raw_output"] = json!(reply.text);
                line["live_calls"] = if compacted {
                    match decode_reply(&reply.text, &defs) {
                        Ok(decoded) => json!({ "calls": calls_json(&decoded.calls) }),
                        Err(e) => {
                            line["live_error_detail"] = json!(e.to_string());
                            json!({ "error": e.code() })
                        }
                    }
                } else {
                    native_calls(&reply.tool_calls)
                };
                if reply.temperature_dropped {
                    line["temperature_dropped"] = json!(true);
                }
            }
            Err(e) => line["live_error"] = json!(e),
        }
        if self.with_native {
            let native = native_request(&case.messages, &selected, Some(date_line));
            match self.send(native).await {
                Ok(reply) => line["native_calls"] = native_calls(&reply.tool_calls),
                Err(e) => line["native_error"] = json!(e),
            }
        }
    }

    async fn send(&self, mut body: Value) -> Result<Reply, String> {
        if self.started.elapsed() > LIVE_BUDGET {
            return Err("live budget exhausted".into());
        }
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        let mut temperature_dropped = false;
        let mut retries = RATE_LIMIT_RETRIES.iter();
        loop {
            let mut request = self.http.post(&self.endpoint).json(&body);
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            let response = request.send().await.map_err(|e| e.to_string())?;
            let status = response.status();
            let text = response.text().await.map_err(|e| e.to_string())?;
            if status.is_success() {
                return parse_reply(&text, temperature_dropped);
            }
            // Some reasoning models accept only their default temperature. Retry without it,
            // and say so on the line: the result is then not a temperature-0 sample.
            if status.as_u16() == 400 && text.contains("temperature") && !temperature_dropped {
                if let Some(map) = body.as_object_mut() {
                    map.remove("temperature");
                }
                temperature_dropped = true;
                continue;
            }
            if status.as_u16() == 429
                && let Some(wait) = retries.next()
                && self.started.elapsed() < LIVE_BUDGET
            {
                tokio::time::sleep(Duration::from_secs(*wait)).await;
                continue;
            }
            return Err(format!(
                "HTTP {status}: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
    }
}

struct Reply {
    text: String,
    tool_calls: Vec<Value>,
    temperature_dropped: bool,
}

fn parse_reply(body: &str, temperature_dropped: bool) -> Result<Reply, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let message = value.pointer("/choices/0/message").ok_or_else(|| {
        format!(
            "no choices in reply: {}",
            body.chars().take(200).collect::<String>()
        )
    })?;
    // `content` may be a string, an array of parts, or null (a reply made only of tool calls).
    let text = match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    };
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(Reply {
        text,
        tool_calls,
        temperature_dropped,
    })
}

/// Native `tool_calls` in the same `{name, arguments}` shape as decoded compact calls.
fn native_calls(tool_calls: &[Value]) -> Value {
    let calls: Result<Vec<Value>, String> = tool_calls
        .iter()
        .map(|c| {
            let name = c
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let raw = c
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            serde_json::from_str::<Value>(raw)
                .map(|arguments| json!({"name": name, "arguments": arguments}))
                .map_err(|e| e.to_string())
        })
        .collect();
    match calls {
        Ok(calls) => json!({ "calls": calls }),
        Err(e) => json!({"error": "invalid_arguments", "error_detail": e}),
    }
}

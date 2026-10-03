//! Compact Tools (Track P1) evaluation harness.
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! # Environment
//!
//! | variable            | required | meaning                                                       |
//! |---------------------|----------|---------------------------------------------------------------|
//! | `EVAL_SET`          | yes      | dataset: `{schema_version, purpose, tools, cases, decoder_cases}` |
//! | `OUT`               | yes      | JSONL output, one line per case (cases, then decoder cases)   |
//! | `MEASURE_OUT`       | no       | per-case `o200k_base` token report (JSONL), kept out of `OUT` |
//! | `PROVIDER_BASE_URL` | no       | OpenAI-compatible base URL (e.g. `https://api.openai.com/v1`); with `MODEL`, enables live mode |
//! | `MODEL`             | no       | model id sent in live mode                                    |
//! | `PROVIDER_API_KEY`  | no       | bearer token for the live endpoint, if it needs one           |
//! | `LIVE_TIMEOUT_SECS` | no       | per-request timeout in live mode (default 60)                 |
//!
//! Offline (the default) the run uses no network, no key and no model, and is
//! deterministic: two runs produce byte-identical `OUT`. A token summary is printed
//! to stderr. In live mode each `compact_request` is sent at temperature 0 and the
//! line gains `raw_output` and `live_calls`.
//!
//! Live mode also prepends the fixed reference time the track prescribes for live
//! runs. It is a fixture for resolving relative dates, not part of compaction, so the
//! offline `compact_request` (whose tokens are scored against a baseline built from
//! `tools` and `messages`) omits it.
//!
//! The harness only calls the public `nasiko_tool_compact` API; it has no
//! case-specific logic.
//!
//! # Extended cases
//!
//! `examples/data/compact-tools-extended.json` is a team-authored set in the same
//! format (more tools, multi-call, no-call, nested and numeric arguments, and stream
//! edge cases). Run it by pointing `EVAL_SET` at it; report its results separately
//! from the official set.

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
    time::Duration,
};

use nasiko_tool_compact::{
    CompactError, CompactTools, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools,
    render_call,
};
use serde_json::{Map, Value, json};
use tiktoken_rs::{CoreBPE, o200k_base};

type AppResult<T> = Result<T, String>;

/// Fixed reference time for live runs, so relative dates resolve identically.
const REFERENCE_CONTEXT: &str = "Today is Friday, 2026-10-02. Timezone: Asia/Kolkata.";

fn main() {
    if let Err(error) = run() {
        eprintln!("compact_tools_eval: {error}");
        std::process::exit(1);
    }
}

fn run() -> AppResult<()> {
    let eval_set = env::var("EVAL_SET").map_err(|_| "EVAL_SET must name an evaluation dataset")?;
    let out = env::var("OUT").map_err(|_| "OUT must name the JSONL output file")?;
    let dataset = Dataset::load(Path::new(&eval_set))?;
    let live = LiveClient::from_env()?;
    let reference = live.as_ref().map(|_| REFERENCE_CONTEXT);
    let tokenizer = o200k_base().map_err(|error| format!("could not load o200k_base: {error}"))?;

    let mut lines = Vec::with_capacity(dataset.cases.len() + dataset.decoder_cases.len());
    let mut measurements = Vec::new();
    let mut live_summary = LiveSummary::default();

    for (index, case) in dataset.cases.iter().enumerate() {
        let normal = normal_record(&dataset, case, index, reference)?;
        measurements.push(TokenMeasurement::new(
            &normal.id,
            &normal.native_request,
            &normal.record["compact_request"],
            normal.record["compacted"] == json!(true),
            &tokenizer,
        )?);
        let mut record = normal.record;
        if let Some(live) = &live {
            let outcome = live.run(&record["compact_request"], &normal.tools);
            live_summary.add(&outcome, &normal.expected, case);
            outcome.write_into(&mut record);
        }
        lines.push(record);
    }
    for (index, case) in dataset.decoder_cases.iter().enumerate() {
        lines.push(decoder_record(&dataset, case, index)?);
    }

    write_jsonl(Path::new(&out), &lines)?;
    if let Ok(path) = env::var("MEASURE_OUT") {
        let values = measurements
            .iter()
            .map(|m| serde_json::to_value(m).map_err(|e| e.to_string()))
            .collect::<AppResult<Vec<_>>>()?;
        write_jsonl(Path::new(&path), &values)?;
    }
    print_token_summary(&measurements);
    if live.is_some() {
        live_summary.print();
    }
    Ok(())
}

// ── Dataset ─────────────────────────────────────────────────────────────────────

struct Dataset {
    /// Native OpenAI tool definitions by function name.
    tools: BTreeMap<String, Value>,
    cases: Vec<Value>,
    decoder_cases: Vec<Value>,
}

impl Dataset {
    fn load(path: &Path) -> AppResult<Self> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
        let root: Value = serde_json::from_str(&text)
            .map_err(|error| format!("'{}' is not valid JSON: {error}", path.display()))?;
        let root = root
            .as_object()
            .ok_or("evaluation dataset must be a JSON object")?;

        let mut tools = BTreeMap::new();
        for tool in array_field(root, "tools") {
            let name = function_of(tool)
                .get("name")
                .and_then(Value::as_str)
                .ok_or("every dataset tool needs a function name")?;
            tools.insert(name.to_string(), tool.clone());
        }
        Ok(Self {
            tools,
            cases: array_field(root, "cases").to_vec(),
            decoder_cases: array_field(root, "decoder_cases").to_vec(),
        })
    }

    /// A case lists tools by name (or, defensively, inline as full definitions).
    fn native_tools(&self, case: &Value, id: &Value) -> AppResult<Vec<Value>> {
        case.get("tools")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|entry| match entry {
                Value::String(name) => self
                    .tools
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("case {id}: unknown dataset tool '{name}'")),
                Value::Object(_) => Ok(entry.clone()),
                _ => Err(format!("case {id}: tool entries must be names or objects")),
            })
            .collect()
    }
}

fn array_field<'a>(object: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn function_of(tool: &Value) -> &Value {
    tool.get("function").unwrap_or(tool)
}

fn case_id(case: &Value, index: usize, kind: &str) -> AppResult<Value> {
    case.get("id")
        .cloned()
        .ok_or_else(|| format!("{kind} {index} is missing an id"))
}

// ── Normal cases ────────────────────────────────────────────────────────────────

struct NormalRecord {
    id: Value,
    record: Value,
    native_request: Value,
    tools: Vec<ToolDef>,
    expected: Vec<ToolCall>,
}

fn normal_record(
    dataset: &Dataset,
    case: &Value,
    index: usize,
    reference: Option<&str>,
) -> AppResult<NormalRecord> {
    let id = case_id(case, index, "case")?;
    let native_tools = dataset.native_tools(case, &id)?;
    let tools = native_tools
        .iter()
        .map(tool_def)
        .collect::<AppResult<Vec<_>>>()
        .map_err(|error| format!("case {id}: {error}"))?;
    let messages = case
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tool_choice = case.get("tool_choice");

    let native_request = native_request(&messages, &native_tools, tool_choice, reference);
    let compact = match encode_tools(&tools) {
        Ok(compact) if !tools.is_empty() && tool_choice_allows_compaction(tool_choice) => {
            Some(compact)
        }
        _ => None,
    };
    let compact_request = match &compact {
        Some(compact) => compact_request(&messages, compact, reference),
        None => native_request.clone(),
    };

    let expected = expected_calls(case).map_err(|error| format!("case {id}: {error}"))?;
    let rendered_calls = expected
        .iter()
        .map(render_call)
        .collect::<Vec<_>>()
        .join("\n");
    let mut record = json!({
        "id": id,
        "compact_request": compact_request,
        "compacted": compact.is_some(),
        "rendered_calls": rendered_calls,
    });
    match decode_calls(&rendered_calls, &tools) {
        Ok(calls) => record["roundtrip_calls"] = json!(calls),
        Err(error) => {
            record["roundtrip_calls"] = json!([]);
            record["roundtrip_error"] = json!(error_detail(&error));
        }
    }
    Ok(NormalRecord {
        id,
        record,
        native_request,
        tools,
        expected,
    })
}

/// The native baseline: the case's messages and full tool schemas (plus the live
/// reference time, when set, so both sides carry it).
fn native_request(
    messages: &[Value],
    tools: &[Value],
    tool_choice: Option<&Value>,
    reference: Option<&str>,
) -> Value {
    let mut all: Vec<Value> = reference
        .map(|text| json!({"role": "system", "content": text}))
        .into_iter()
        .collect();
    all.extend_from_slice(messages);
    let mut request = json!({ "messages": all });
    if !tools.is_empty() {
        request["tools"] = json!(tools);
    }
    if let Some(choice) = tool_choice {
        request["tool_choice"] = choice.clone();
    }
    request
}

/// The compact request: no native `tools`; one system message carrying the compact
/// definitions and call-format instructions (after the live reference time, if set).
fn compact_request(messages: &[Value], compact: &CompactTools, reference: Option<&str>) -> Value {
    let system = match reference {
        Some(text) => format!(
            "{text}

{}",
            compact.render()
        ),
        None => compact.render(),
    };
    let mut all = vec![json!({"role": "system", "content": system})];
    all.extend_from_slice(messages);
    json!({ "messages": all })
}

/// Compaction keeps OpenAI's default `auto` behaviour; a forced or disabled tool
/// choice cannot be guaranteed by a prompt, so those requests are sent natively.
fn tool_choice_allows_compaction(tool_choice: Option<&Value>) -> bool {
    matches!(tool_choice, None | Some(Value::Null)) || tool_choice == Some(&json!("auto"))
}

fn tool_def(tool: &Value) -> AppResult<ToolDef> {
    if tool
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "function")
    {
        return Err("only function tools are supported".into());
    }
    let function = function_of(tool);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or("tool is missing a function name")?;
    Ok(ToolDef {
        name: name.into(),
        description: function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        parameters: function.get("parameters").cloned(),
    })
}

/// `expected` is a list of `{name, arguments}` (OpenAI `function` wrappers and
/// string-encoded arguments are also accepted).
fn expected_calls(case: &Value) -> AppResult<Vec<ToolCall>> {
    let calls = match case.get("expected") {
        Some(Value::Array(calls)) => calls.as_slice(),
        Some(Value::Object(object)) => array_field(object, "calls"),
        _ => &[],
    };
    calls.iter().map(expected_call).collect()
}

fn expected_call(call: &Value) -> AppResult<ToolCall> {
    let function = function_of(call);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or("expected call is missing a name")?;
    let arguments = match function.get("arguments") {
        Some(Value::String(text)) => serde_json::from_str(text)
            .map_err(|error| format!("expected call '{name}' has invalid arguments: {error}"))?,
        Some(value) => value.clone(),
        None => json!({}),
    };
    Ok(ToolCall {
        name: name.into(),
        arguments,
    })
}

// ── Decoder cases ───────────────────────────────────────────────────────────────

fn decoder_record(dataset: &Dataset, case: &Value, index: usize) -> AppResult<Value> {
    let id = case_id(case, index, "decoder case")?;
    let tools = dataset
        .native_tools(case, &id)?
        .iter()
        .map(tool_def)
        .collect::<AppResult<Vec<_>>>()
        .map_err(|error| format!("decoder case {id}: {error}"))?;
    let chunks = case
        .get("chunks")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("decoder case {id} is missing chunks"))?
        .iter()
        .map(|chunk| {
            chunk
                .as_str()
                .ok_or_else(|| format!("decoder case {id}: chunks must be strings"))
        })
        .collect::<AppResult<Vec<_>>>()?;

    let mut decoder = StreamDecoder::new(&tools);
    let result = chunks
        .iter()
        .try_for_each(|chunk| decoder.push(chunk).map(drop))
        .and_then(|()| decoder.finish());
    Ok(json!({ "id": id, "decoded": calls_or_error(result) }))
}

/// The scorer's vocabulary is `{"calls": [...]}` or `{"error": "unknown_tool" |
/// "invalid_arguments"}`; `detail` keeps the precise library error.
fn calls_or_error(result: Result<Vec<ToolCall>, CompactError>) -> Value {
    match result {
        Ok(calls) => json!({ "calls": calls }),
        Err(error) => json!({
            "error": scorer_error_code(&error),
            "detail": error_detail(&error),
        }),
    }
}

fn scorer_error_code(error: &CompactError) -> &'static str {
    match error {
        CompactError::UnknownTool(_) => "unknown_tool",
        _ => "invalid_arguments",
    }
}

fn error_detail(error: &CompactError) -> String {
    format!("{}: {error}", error.code())
}

// ── Live mode ───────────────────────────────────────────────────────────────────

struct LiveClient {
    endpoint: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

struct LiveOutcome {
    raw_output: Option<String>,
    result: Option<Result<Vec<ToolCall>, CompactError>>,
    error: Option<String>,
}

impl LiveClient {
    fn from_env() -> AppResult<Option<Self>> {
        let non_empty = |key| env::var(key).ok().filter(|value| !value.is_empty());
        let (Some(base_url), Some(model)) = (non_empty("PROVIDER_BASE_URL"), non_empty("MODEL"))
        else {
            return Ok(None);
        };
        let timeout = non_empty("LIVE_TIMEOUT_SECS")
            .map(|secs| {
                secs.parse::<u64>()
                    .map_err(|_| "LIVE_TIMEOUT_SECS must be an integer")
            })
            .transpose()?
            .unwrap_or(60);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .map_err(|error| format!("could not build HTTP client: {error}"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("could not start runtime: {error}"))?;
        Ok(Some(Self {
            endpoint: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            model,
            api_key: non_empty("PROVIDER_API_KEY"),
            client,
            runtime,
        }))
    }

    fn run(&self, compact_request: &Value, tools: &[ToolDef]) -> LiveOutcome {
        match self.runtime.block_on(self.complete(compact_request)) {
            Ok(text) => LiveOutcome {
                result: Some(decode_calls(&text, tools)),
                raw_output: Some(text),
                error: None,
            },
            Err(error) => LiveOutcome {
                raw_output: None,
                result: None,
                error: Some(error),
            },
        }
    }

    async fn complete(&self, compact_request: &Value) -> AppResult<String> {
        let mut body = compact_request.clone();
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        let mut request = self.client.post(&self.endpoint).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = response.status();
        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("invalid response body ({status}): {e}"))?;
        if !status.is_success() {
            return Err(format!("provider returned {status}: {payload}"));
        }
        Ok(payload["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }
}

impl LiveOutcome {
    fn write_into(self, record: &mut Value) {
        record["raw_output"] = json!(self.raw_output);
        match self.result {
            Some(result) => record["live_calls"] = calls_or_error(result),
            None => record["live_calls"] = Value::Null,
        }
        if let Some(error) = self.error {
            record["live_error"] = json!(error);
        }
    }
}

/// Local, indicative live metrics for the PR description. The organizers' scorer is
/// authoritative; this compares tool names in order, exact non-free-text arguments,
/// and presence plus type of `match.free_text_fields`.
#[derive(Default)]
struct LiveSummary {
    cases: usize,
    transport_errors: usize,
    decode_errors: usize,
    correct_tools: usize,
    exact: usize,
}

impl LiveSummary {
    fn add(&mut self, outcome: &LiveOutcome, expected: &[ToolCall], case: &Value) {
        self.cases += 1;
        let Some(result) = &outcome.result else {
            self.transport_errors += 1;
            return;
        };
        let Ok(calls) = result else {
            self.decode_errors += 1;
            return;
        };
        let free_text = case["match"]["free_text_fields"]
            .as_array()
            .map(|fields| fields.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let names_match = calls.len() == expected.len()
            && calls.iter().zip(expected).all(|(a, b)| a.name == b.name);
        if names_match {
            self.correct_tools += 1;
            if calls
                .iter()
                .zip(expected)
                .all(|(a, b)| arguments_match(&a.arguments, &b.arguments, &free_text))
            {
                self.exact += 1;
            }
        }
    }

    fn print(&self) {
        let pct = |n: usize| 100.0 * n as f64 / self.cases.max(1) as f64;
        eprintln!(
            "live: {} cases | decoded {:.1}% | right tools {:.1}% | right tools+args {:.1}% | decode errors {} | transport errors {}",
            self.cases,
            pct(self.cases - self.transport_errors - self.decode_errors),
            pct(self.correct_tools),
            pct(self.exact),
            self.decode_errors,
            self.transport_errors,
        );
    }
}

fn arguments_match(actual: &Value, expected: &Value, free_text: &[&str]) -> bool {
    let (Some(actual), Some(expected)) = (actual.as_object(), expected.as_object()) else {
        return actual == expected;
    };
    actual.len() == expected.len()
        && expected.iter().all(|(key, want)| match actual.get(key) {
            Some(got) if free_text.contains(&key.as_str()) => {
                std::mem::discriminant(got) == std::mem::discriminant(want)
            }
            Some(got) => got == want,
            None => false,
        })
}

// ── Token measurement ───────────────────────────────────────────────────────────

/// Local-only token report. `OUT` carries outputs only; the organizers recount.
#[derive(serde::Serialize)]
struct TokenMeasurement {
    id: Value,
    compacted: bool,
    native_tokens: usize,
    compact_tokens: usize,
    token_reduction: f64,
}

impl TokenMeasurement {
    fn new(
        id: &Value,
        native: &Value,
        compact: &Value,
        compacted: bool,
        tokenizer: &CoreBPE,
    ) -> AppResult<Self> {
        let native_tokens = token_count(native, tokenizer)?;
        let compact_tokens = token_count(compact, tokenizer)?;
        Ok(Self {
            id: id.clone(),
            compacted,
            native_tokens,
            compact_tokens,
            token_reduction: reduction(native_tokens, compact_tokens),
        })
    }
}

/// Tokens in the complete request body, serialized as it would be sent.
fn token_count(request: &Value, tokenizer: &CoreBPE) -> AppResult<usize> {
    let body = serde_json::to_string(request)
        .map_err(|error| format!("could not serialize request: {error}"))?;
    Ok(tokenizer.encode_with_special_tokens(&body).len())
}

fn reduction(native: usize, compact: usize) -> f64 {
    if native == 0 {
        0.0
    } else {
        1.0 - compact as f64 / native as f64
    }
}

fn print_token_summary(measurements: &[TokenMeasurement]) {
    let native: usize = measurements.iter().map(|m| m.native_tokens).sum();
    let compact: usize = measurements.iter().map(|m| m.compact_tokens).sum();
    let compacted = measurements.iter().filter(|m| m.compacted).count();
    eprintln!(
        "tokens (o200k_base): native {native} | compact {compact} | reduction {:.1}% | compacted {compacted}/{} cases",
        100.0 * reduction(native, compact),
        measurements.len(),
    );
}

// ── Output ──────────────────────────────────────────────────────────────────────

fn write_jsonl(path: &Path, values: &[Value]) -> AppResult<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create '{}': {error}", parent.display()))?;
    }
    let file = File::create(path)
        .map_err(|error| format!("could not create '{}': {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    for value in values {
        serde_json::to_writer(&mut writer, &canonicalize(value))
            .and_then(|()| writer.write_all(b"\n").map_err(serde_json::Error::io))
            .map_err(|error| format!("could not write '{}': {error}", path.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("could not flush '{}': {error}", path.display()))
}

/// Sort object keys recursively so output bytes never depend on map ordering.
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let sorted: BTreeMap<_, _> = values
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dataset() -> Dataset {
        let tool = json!({"type": "function", "function": {
            "name": "lookup",
            "description": "Look something up.",
            "parameters": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}
        }});
        Dataset {
            tools: BTreeMap::from([("lookup".to_string(), tool)]),
            cases: vec![],
            decoder_cases: vec![],
        }
    }

    /// The team's extended set is held to its own expectations offline: every case
    /// compacts and round-trips, and every decoder case yields its calls or error code.
    #[test]
    fn extended_dataset_meets_its_expectations() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/data/compact-tools-extended.json");
        let dataset = Dataset::load(&path).unwrap();
        assert!(!dataset.cases.is_empty() && !dataset.decoder_cases.is_empty());
        for (index, case) in dataset.cases.iter().enumerate() {
            let normal = normal_record(&dataset, case, index, None).unwrap();
            assert_eq!(normal.record["compacted"], json!(true), "{}", normal.id);
            assert_eq!(
                normal.record["roundtrip_calls"],
                json!(normal.expected),
                "{}",
                normal.id
            );
        }
        for (index, case) in dataset.decoder_cases.iter().enumerate() {
            let record = decoder_record(&dataset, case, index).unwrap();
            let expected = &case["expected"];
            match expected.get("error") {
                Some(code) => assert_eq!(&record["decoded"]["error"], code, "{}", record["id"]),
                None => assert_eq!(&record["decoded"], expected, "{}", record["id"]),
            }
        }
    }

    #[test]
    fn normal_case_resolves_named_tools_and_round_trips_expected_calls() {
        let case = json!({
            "id": "t-1",
            "tools": ["lookup"],
            "messages": [{"role": "user", "content": "find x"}],
            "expected": [{"name": "lookup", "arguments": {"q": "x"}}]
        });
        let normal = normal_record(&dataset(), &case, 0, Some(REFERENCE_CONTEXT)).unwrap();
        let record = normal.record;
        assert_eq!(record["compacted"], json!(true));
        assert_eq!(
            record["rendered_calls"],
            json!(r#"<<call lookup {"q":"x"}>>"#)
        );
        assert_eq!(
            record["roundtrip_calls"],
            json!([{"name": "lookup", "arguments": {"q": "x"}}])
        );
        let request = &record["compact_request"];
        assert!(request.get("tools").is_none());
        let system = request["messages"][0]["content"].as_str().unwrap();
        assert!(system.starts_with(REFERENCE_CONTEXT) && system.contains("lookup("));
        assert_eq!(
            request["messages"][1],
            json!({"role": "user", "content": "find x"})
        );
    }

    #[test]
    fn offline_requests_carry_no_reference_time() {
        let case = json!({"id": "t-5", "tools": ["lookup"], "messages": [{"role": "user", "content": "hi"}]});
        let normal = normal_record(&dataset(), &case, 0, None).unwrap();
        let system = normal.record["compact_request"]["messages"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            system.starts_with("Tools; ")
                && system.contains("\nlookup(")
                && !system.contains("2026-10-02")
        );
        assert_eq!(
            normal.native_request["messages"],
            json!([{"role": "user", "content": "hi"}])
        );
    }

    #[test]
    fn forced_tool_choice_bypasses_compaction() {
        let case = json!({
            "id": "t-2", "tools": ["lookup"], "messages": [],
            "tool_choice": {"type": "function", "function": {"name": "lookup"}}
        });
        let record = normal_record(&dataset(), &case, 0, None).unwrap().record;
        assert_eq!(record["compacted"], json!(false));
        assert!(record["compact_request"].get("tools").is_some());
    }

    #[test]
    fn invalid_expected_call_is_reported_not_fatal() {
        let case = json!({
            "id": "t-3", "tools": ["lookup"], "messages": [],
            "expected": [{"name": "lookup", "arguments": {}}]
        });
        let record = normal_record(&dataset(), &case, 0, None).unwrap().record;
        assert_eq!(record["roundtrip_calls"], json!([]));
        assert!(
            record["roundtrip_error"]
                .as_str()
                .unwrap()
                .starts_with("invalid_arguments")
        );
    }

    #[test]
    fn decoder_case_feeds_chunks_and_maps_errors() {
        let ok = json!({"id": "d-1", "tools": ["lookup"], "chunks": ["<<call look", "up {\"q\":\"x\"}>>"]});
        assert_eq!(
            decoder_record(&dataset(), &ok, 0).unwrap()["decoded"],
            json!({"calls": [{"name": "lookup", "arguments": {"q": "x"}}]})
        );
        let unknown = json!({"id": "d-2", "tools": ["lookup"], "chunks": ["<<call nope {}>>"]});
        assert_eq!(
            decoder_record(&dataset(), &unknown, 0).unwrap()["decoded"]["error"],
            json!("unknown_tool")
        );
        let bad = json!({"id": "d-3", "tools": ["lookup"], "chunks": ["<<call lookup {}>>"]});
        assert_eq!(
            decoder_record(&dataset(), &bad, 0).unwrap()["decoded"]["error"],
            json!("invalid_arguments")
        );
    }

    #[test]
    fn unknown_dataset_tool_is_a_dataset_error() {
        let case = json!({"id": "t-4", "tools": ["missing"], "messages": []});
        assert!(normal_record(&dataset(), &case, 0, None).is_err());
    }

    #[test]
    fn free_text_fields_match_on_presence_and_type() {
        let expected = json!({"to": ["a"], "subject": "Build status"});
        assert!(arguments_match(
            &json!({"to": ["a"], "subject": "Green"}),
            &expected,
            &["subject"]
        ));
        assert!(!arguments_match(
            &json!({"to": ["a"], "subject": 1}),
            &expected,
            &["subject"]
        ));
        assert!(!arguments_match(
            &json!({"to": ["b"], "subject": "x"}),
            &expected,
            &["subject"]
        ));
    }

    #[test]
    fn canonical_output_sorts_keys() {
        let value = json!({"b": 1, "a": {"d": 2, "c": 3}});
        assert_eq!(
            serde_json::to_string(&canonicalize(&value)).unwrap(),
            r#"{"a":{"c":3,"d":2},"b":1}"#
        );
    }
}

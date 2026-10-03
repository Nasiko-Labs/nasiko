//! Compact tool schemas eval.
//!
//! Run (offline, deterministic — no network):
//!   curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Live mode (also sends every compact request — and its native-tools twin — to a model):
//!   PROVIDER_BASE_URL=https://…/v1 MODEL=… [PROVIDER_API_KEY=…] [LIVE_DATE_LINE="Today is …"]
//!   [LIVE_MAX_TOKENS=1024] [COMPACT_INSTRUCTIONS="…" — ablation: replaces the instruction line]
//!
//! Writes one JSONL line per case to `OUT`:
//!   cases:          {id, compact_request, compacted, rendered_calls, roundtrip_calls[, raw_output, live_calls, native_calls]}
//!   decoder cases:  {id, decoded: {calls:[{name, arguments}]} | {error: "unknown_tool"|"invalid_arguments"}}
//! Token counts (tiktoken-rs, o200k_base) and pass counts go to stderr only — never into `OUT`.
//!
//! Decoder cases are fed to the streaming decoder chunk by chunk, unchanged: the call grammar is
//! exactly the brief's `<<call NAME {JSON}>>`.
use std::collections::BTreeMap;
use std::io::Write;

use nasiko_tool_compact::{self as tc, Event, ToolCall, ToolDef};
use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Date line for live runs: the public cases resolve "tomorrow"/"Monday" against 2026-10-03.
/// Added to BOTH the compact and native arms, and never to the scored `compact_request`.
const DEFAULT_DATE_LINE: &str = "Today is 2026-10-03 (Saturday), timezone Asia/Kolkata.";

struct Live {
    base: String,
    model: String,
    key: Option<String>,
    date_line: String,
    max_tokens: u64,
    http: reqwest::Client,
    rt: tokio::runtime::Runtime,
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");
    // The same file, key order preserved: request bodies are written and token-counted exactly
    // as authored (serde_json's `Value` would re-sort keys and skew the baseline).
    let ordered: Ordered = serde_json::from_str(&raw).expect("valid eval JSON");
    let ordered_tools: BTreeMap<String, Ordered> = ordered
        .get("tools")
        .map(Ordered::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|t| {
            let name = t.get("function")?.get("name")?.as_str()?.to_string();
            Some((name, t.clone()))
        })
        .collect();
    let ordered_cases: BTreeMap<String, &Ordered> = ordered
        .get("cases")
        .map(Ordered::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| Some((c.get("id")?.as_str()?.to_string(), c)))
        .collect();

    // name → (native OpenAI tool JSON, crate ToolDef)
    let mut tools: BTreeMap<String, (Value, ToolDef)> = BTreeMap::new();
    for t in data["tools"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let f = &t["function"];
        let Some(name) = f["name"].as_str() else {
            continue;
        };
        let def = ToolDef {
            name: name.to_string(),
            description: f["description"].as_str().map(str::to_string),
            parameters: f.get("parameters").cloned(),
        };
        tools.insert(name.to_string(), (t.clone(), def));
    }
    let pick = |names: &Value| -> (Vec<Value>, Vec<ToolDef>) {
        let mut native = Vec::new();
        let mut defs = Vec::new();
        for n in names.as_array().map(Vec::as_slice).unwrap_or_default() {
            match n.as_str().and_then(|n| tools.get(n)) {
                Some((t, d)) => {
                    native.push(t.clone());
                    defs.push(d.clone());
                }
                None => eprintln!("warning: case references unknown tool {n}"),
            }
        }
        (native, defs)
    };

    let instructions_override = std::env::var("COMPACT_INSTRUCTIONS")
        .ok()
        .filter(|s| !s.trim().is_empty());
    if let Some(ins) = &instructions_override {
        eprintln!("ablation: COMPACT_INSTRUCTIONS = {ins:?}");
    }
    let live = live_from_env();
    let bpe = tiktoken_rs::o200k_base().ok();
    let count = |v: &Ordered, pretty: bool| -> Option<usize> {
        let s = if pretty {
            serde_json::to_string_pretty(v).ok()?
        } else {
            serde_json::to_string(v).ok()?
        };
        Some(bpe.as_ref()?.encode_with_special_tokens(&s).len())
    };

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut summary = Summary::default();

    for case in data["cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or_default();
        let (_, defs) = pick(&case["tools"]);
        // Authored-order copies of this case's messages and native tools.
        let messages: Vec<Ordered> = ordered_cases
            .get(id)
            .and_then(|c| c.get("messages"))
            .map(Ordered::items)
            .unwrap_or_default()
            .to_vec();
        let native_tools: Vec<Ordered> = case["tools"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|n| ordered_tools.get(n.as_str()?).cloned())
            .collect();
        let native_request = Ordered::object([
            ("messages", Ordered::Arr(messages.clone())),
            ("tools", Ordered::Arr(native_tools)),
        ]);

        let (compact_request, compacted) = match tc::encode_tools(&defs) {
            Ok(c) => {
                // Ablation knob for live runs: swap the instruction line, keep the definitions.
                let system = match &instructions_override {
                    Some(ins) => format!("{ins}\n{}", c.definitions),
                    None => c.system_text(),
                };
                let mut msgs = vec![Ordered::object([
                    ("role", Ordered::Str("system".into())),
                    ("content", Ordered::Str(system)),
                ])];
                msgs.extend(messages.iter().cloned());
                (Ordered::object([("messages", Ordered::Arr(msgs))]), true)
            }
            Err(e) => {
                eprintln!("{id}: bypass ({e}) — compact_request carries the native tools");
                summary.bypassed += 1;
                // Byte-identical to the native request: a bypass saves exactly 0%.
                (native_request.clone(), false)
            }
        };

        let expected: Vec<ToolCall> = case["expected"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|c| ToolCall {
                name: c["name"].as_str().unwrap_or_default().to_string(),
                arguments: c["arguments"].to_string(),
            })
            .collect();
        let rendered = tc::render_calls(&expected);
        let roundtrip = if compacted {
            match tc::decode_calls(&rendered, &defs) {
                Ok(calls) => calls_json(&calls),
                Err(e) => json!({ "error": e.code() }),
            }
        } else {
            // Bypassed: the request keeps native tools, so calls come back as native
            // `tool_calls` and are never transformed — they pass through unchanged.
            calls_json(&expected)
        };
        summary.cases += 1;
        if roundtrip == case["expected"] {
            summary.roundtrip_ok += 1;
        } else {
            eprintln!("{id}: round trip mismatch");
        }

        let mut line = CaseLine {
            id,
            compact_request: &compact_request,
            compacted,
            rendered_calls: &rendered,
            roundtrip_calls: &roundtrip,
            raw_output: None,
            live_calls: None,
            native_calls: None,
        };

        if let Some(live) = &live {
            let compact_value = serde_json::to_value(&compact_request).expect("request JSON");
            let native_value = serde_json::to_value(&native_request).expect("request JSON");
            // A bypassed case sends native tools, so its "compact" arm answers natively too.
            let (raw_output, live_calls) = if compacted {
                live.compact_arm(&compact_value, &defs)
            } else {
                (Value::Null, live.native_arm(&compact_value))
            };
            let native_calls = live.native_arm(&native_value);
            let free: Vec<&str> = case["match"]["free_text_fields"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            summary
                .live_compact
                .add(score(&live_calls, &case["expected"], &free));
            summary
                .live_native
                .add(score(&native_calls, &case["expected"], &free));
            line.raw_output = Some(raw_output);
            line.live_calls = Some(live_calls);
            line.native_calls = Some(native_calls);
        }

        if let (Some(n), Some(np), Some(c)) = (
            count(&native_request, false),
            count(&native_request, true),
            count(&compact_request, false),
        ) {
            summary.tokens.push((id.to_string(), n, np, c));
        }
        writeln!(out, "{}", serde_json::to_string(&line).expect("OUT line")).expect("write OUT");
    }

    for case in data["decoder_cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or_default();
        let (_, defs) = pick(&case["tools"]);
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let decoded = match decode_chunks(&defs, &chunks) {
            Ok(calls) => json!({ "calls": calls_json(&calls) }),
            Err(code) => json!({ "error": code }),
        };
        summary.decoder += 1;
        if case.get("expected").is_some_and(|e| *e == decoded) {
            summary.decoder_ok += 1;
        } else if case.get("expected").is_some() {
            eprintln!("{id}: decoder output differs from expected");
        }
        writeln!(out, "{}", json!({ "id": id, "decoded": decoded })).expect("write OUT");
    }
    out.flush().expect("flush OUT");
    summary.print(bpe.is_some(), live.as_ref().map(|l| l.model.as_str()));
}

/// One `OUT` line for a case. Field order is fixed, so output is deterministic.
#[derive(Serialize)]
struct CaseLine<'a> {
    id: &'a str,
    compact_request: &'a Ordered,
    compacted: bool,
    rendered_calls: &'a str,
    roundtrip_calls: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_calls: Option<Value>,
}

/// A JSON value that keeps object keys in input order. The workspace deliberately does not
/// enable serde_json's `preserve_order`, so `Value` sorts keys; request bodies built from
/// `Value` would not be the bodies the eval file authored, and their token counts would drift.
#[derive(Debug, Clone, PartialEq)]
enum Ordered {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<Ordered>),
    Obj(Vec<(String, Ordered)>),
}

impl Ordered {
    fn object<const N: usize>(fields: [(&str, Ordered); N]) -> Self {
        Ordered::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }
    fn get(&self, key: &str) -> Option<&Ordered> {
        match self {
            Ordered::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn items(&self) -> &[Ordered] {
        match self {
            Ordered::Arr(items) => items,
            _ => &[],
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Ordered::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Ordered::Null => s.serialize_unit(),
            Ordered::Bool(b) => s.serialize_bool(*b),
            Ordered::Num(n) => n.serialize(s),
            Ordered::Str(v) => s.serialize_str(v),
            Ordered::Arr(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Ordered::Obj(fields) => {
                let mut map = s.serialize_map(Some(fields.len()))?;
                for (k, v) in fields {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_unit<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Ordered, E> {
                Ok(Ordered::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Ordered, E> {
                Ok(Ordered::Num(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Ordered, E> {
                Ok(Ordered::Num(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Ordered, E> {
                serde_json::Number::from_f64(v)
                    .map(Ordered::Num)
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Ordered, E> {
                Ok(Ordered::Str(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<Ordered, E> {
                Ok(Ordered::Str(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Ordered::Arr(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut fields = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, Ordered>()? {
                    fields.push((k, v));
                }
                Ok(Ordered::Obj(fields))
            }
        }
        d.deserialize_any(V)
    }
}

/// Feed `chunks` to the streaming decoder in order; calls, or the external error code.
fn decode_chunks(defs: &[ToolDef], chunks: &[&str]) -> Result<Vec<ToolCall>, &'static str> {
    let mut decoder = tc::StreamDecoder::new(defs).map_err(|_| "invalid_arguments")?;
    let mut calls = Vec::new();
    let mut take = |events: Vec<Event>| {
        calls.extend(events.into_iter().filter_map(|e| match e {
            Event::Call(c) => Some(c),
            Event::Text(_) => None,
        }))
    };
    for chunk in chunks {
        take(decoder.feed(chunk).map_err(|e| e.code())?);
    }
    take(decoder.finish().map_err(|e| e.code())?);
    Ok(calls)
}

/// Calls as `[{name, arguments: {…object…}}]` — the eval's `expected` shape.
fn calls_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                let args: Value = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                json!({ "name": c.name, "arguments": args })
            })
            .collect(),
    )
}

fn live_from_env() -> Option<Live> {
    let base = std::env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let model = std::env::var("MODEL").ok().filter(|s| !s.is_empty())?;
    Some(Live {
        base: base.trim_end_matches('/').to_string(),
        model,
        key: std::env::var("PROVIDER_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        date_line: std::env::var("LIVE_DATE_LINE").unwrap_or_else(|_| DEFAULT_DATE_LINE.into()),
        // Explicit cap: some providers (OpenRouter + Gemini 2.5) otherwise reserve their
        // 64k default and reject the call (HTTP 402) on low-credit accounts.
        max_tokens: std::env::var("LIVE_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1024),
        http: reqwest::Client::new(),
        rt: tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime"),
    })
}

impl Live {
    /// POST `request` (+ model, temperature 0, date line) to `/chat/completions`.
    fn post(&self, request: &Value) -> Result<Value, String> {
        let mut body = request.clone();
        let mut msgs = vec![json!({"role": "system", "content": self.date_line})];
        msgs.extend(body["messages"].as_array().cloned().unwrap_or_default());
        body["messages"] = Value::Array(msgs);
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        body["max_tokens"] = json!(self.max_tokens);
        self.rt.block_on(async {
            let mut req = self
                .http
                .post(format!("{}/chat/completions", self.base))
                .json(&body);
            if let Some(k) = &self.key {
                req = req.bearer_auth(k);
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            let v: Value = resp.json().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {v}"));
            }
            Ok(v)
        })
    }

    fn compact_arm(&self, request: &Value, defs: &[ToolDef]) -> (Value, Value) {
        match self.post(request) {
            Ok(v) => {
                let text = v["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                // Live answers are decoded like the router does (bare-name alias on); the
                // offline decoder cases above use the strict brief grammar.
                let opts = tc::DecodeOptions {
                    bare_tool_markers: true,
                };
                let calls = match tc::decode_with(&text, defs, opts) {
                    Ok(d) => calls_json(&d.calls),
                    Err(e) => json!({ "error": e.code() }),
                };
                (json!(text), calls)
            }
            Err(e) => (
                Value::Null,
                json!({ "error": "request_failed", "detail": e }),
            ),
        }
    }

    fn native_arm(&self, request: &Value) -> Value {
        match self.post(request) {
            Ok(v) => Value::Array(
                v["choices"][0]["message"]["tool_calls"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|c| {
                        let args = c["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .unwrap_or(Value::Null);
                        json!({ "name": c["function"]["name"], "arguments": args })
                    })
                    .collect(),
            ),
            Err(e) => json!({ "error": "request_failed", "detail": e }),
        }
    }
}

#[derive(Default)]
struct Summary {
    cases: usize,
    roundtrip_ok: usize,
    bypassed: usize,
    decoder: usize,
    decoder_ok: usize,
    /// (id, native compact-JSON tokens, native pretty-JSON tokens, compact tokens)
    tokens: Vec<(String, usize, usize, usize)>,
    live_compact: LiveStats,
    live_native: LiveStats,
}

/// How one live answer compares to the expected calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Score {
    /// Same calls, same order, same argument keys; values equal except free-text fields
    /// (which only need to be present strings).
    Exact,
    /// Decoded fine but the calls differ (wrong tool, wrong/missing/extra arguments).
    WrongCalls,
    /// Expected calls, got none.
    Missed,
    /// Expected no call, got one.
    FalseCall,
    /// The compact text did not decode (`invalid_arguments` / `unknown_tool`).
    FormatError,
    /// The HTTP request itself failed — excluded from the rates.
    RequestFailed,
}

#[derive(Default)]
struct LiveStats {
    counts: BTreeMap<&'static str, usize>,
}

impl LiveStats {
    fn add(&mut self, s: Score) {
        let key = match s {
            Score::Exact => "exact",
            Score::WrongCalls => "wrong_calls",
            Score::Missed => "missed",
            Score::FalseCall => "false_call",
            Score::FormatError => "format_error",
            Score::RequestFailed => "request_failed",
        };
        *self.counts.entry(key).or_default() += 1;
    }
    fn get(&self, k: &str) -> usize {
        self.counts.get(k).copied().unwrap_or(0)
    }
    fn line(&self) -> String {
        let answered: usize = self
            .counts
            .iter()
            .filter(|(k, _)| **k != "request_failed")
            .map(|(_, v)| v)
            .sum();
        let valid = answered - self.get("format_error");
        format!(
            "exact {}/{answered}  valid-format {valid}/{answered}  wrong {}  missed {}  false-call {}  format-err {}  req-failed {}",
            self.get("exact"),
            self.get("wrong_calls"),
            self.get("missed"),
            self.get("false_call"),
            self.get("format_error"),
            self.get("request_failed"),
        )
    }
}

fn score(got: &Value, expected: &Value, free: &[&str]) -> Score {
    let Some(got) = got.as_array() else {
        return if got.get("error").and_then(Value::as_str) == Some("request_failed") {
            Score::RequestFailed
        } else {
            Score::FormatError
        };
    };
    let expected = expected.as_array().map(Vec::as_slice).unwrap_or_default();
    match (expected.is_empty(), got.is_empty()) {
        (true, true) => return Score::Exact,
        (true, false) => return Score::FalseCall,
        (false, true) => return Score::Missed,
        _ => {}
    }
    let same = got.len() == expected.len()
        && got.iter().zip(expected).all(|(g, e)| {
            let (Some(ga), Some(ea)) = (g["arguments"].as_object(), e["arguments"].as_object())
            else {
                return false;
            };
            g["name"] == e["name"]
                && ga.len() == ea.len()
                && ea.iter().all(|(k, ev)| match ga.get(k) {
                    Some(gv) if free.contains(&k.as_str()) => gv.is_string(),
                    Some(gv) => gv == ev,
                    None => false,
                })
        });
    if same {
        Score::Exact
    } else {
        Score::WrongCalls
    }
}

impl Summary {
    fn print(&self, have_tokens: bool, live_model: Option<&str>) {
        eprintln!("── compact_tools_eval ──────────────────────────────────────");
        eprintln!("round trips    {}/{}", self.roundtrip_ok, self.cases);
        eprintln!("decoder cases  {}/{}", self.decoder_ok, self.decoder);
        eprintln!("bypassed       {}", self.bypassed);
        if let Some(m) = live_model {
            eprintln!("live model     {m} (raw_output / live_calls / native_calls in OUT)");
            eprintln!("  compact arm  {}", self.live_compact.line());
            eprintln!("  native arm   {}", self.live_native.line());
        }
        if !have_tokens {
            eprintln!("tokens         unavailable (tiktoken o200k_base failed to load)");
            return;
        }
        eprintln!("tokens (o200k_base, full request body; baseline = messages + native tools)");
        eprintln!(
            "  {:<12} {:>8} {:>8} {:>8} {:>9}",
            "case", "native", "pretty", "compact", "reduction"
        );
        let (mut n, mut np, mut c) = (0, 0, 0);
        for (id, a, b, d) in &self.tokens {
            eprintln!("  {id:<12} {a:>8} {b:>8} {d:>8} {:>8.1}%", pct(*a, *d));
            n += a;
            np += b;
            c += d;
        }
        eprintln!(
            "  {:<12} {n:>8} {np:>8} {c:>8} {:>8.1}%",
            "TOTAL",
            pct(n, c)
        );
        eprintln!("  vs pretty-printed native baseline: {:.1}%", pct(np, c));
    }
}

fn pct(native: usize, compact: usize) -> f64 {
    if native == 0 {
        0.0
    } else {
        100.0 * (1.0 - compact as f64 / native as f64)
    }
}

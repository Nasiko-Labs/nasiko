//! Evaluation-set model and the offline output contract.
//!
//! One JSONL line per case, outputs only (no scores):
//!
//! ```json
//! {"id":"ct-001","compact_request":{…},"compacted":true,"rendered_calls":"<<call …>>","roundtrip_calls":[…]}
//! {"id":"dc-002","decoded":{"calls":[…]}}
//! ```
//!
//! `compact_request` is the complete body a live run POSTs, serialized once; nothing is added
//! afterwards. Expected answers are reachable only from `render_calls`, never from request
//! building: `CaseInputs` has no `expected` field by construction.

use std::io::{self, Write};

use nasiko_llm_router::compact_tools::{self, Compiled, Plan};
use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::inbound::{InboundFormat, InboundParser, OpenAiInbound};
use nasiko_llm_router::ir::ChatRequest;
use nasiko_tool_compact::{StreamDecoder, ToolDef, canonical_json, decode_calls, encode_call};
use serde::Deserialize;
use serde_json::{Value, json};

/// Fixed reference time for relative dates, as a system message in every request.
pub const REFERENCE_TIME_MESSAGE: &str = "Today is 2026-10-02. Timezone: Asia/Kolkata.";
/// `model` used offline when `MODEL` is unset. A constant so two runs are byte-identical.
pub const OFFLINE_MODEL_PLACEHOLDER: &str = "offline-placeholder";
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 1024;
pub const TEMPERATURE: f64 = 0.0;

#[derive(Debug, Clone, Deserialize)]
pub struct EvalSet {
    #[serde(default)]
    pub schema_version: Value,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub cases: Vec<Case>,
    #[serde(default)]
    pub decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub expected: Vec<ExpectedCall>,
    #[serde(rename = "match", default)]
    pub match_rules: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ExpectedCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecoderCase {
    pub id: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub chunks: Vec<String>,
    #[serde(default)]
    pub expected: Value,
}

/// The only inputs request building may read.
pub struct CaseInputs<'a> {
    pub tools: &'a [String],
    pub messages: &'a [Value],
}

pub struct BuiltRequest {
    /// The complete body. In live mode this exact value is POSTed.
    pub body: Value,
    pub compacted: bool,
    pub bypass: Option<&'static str>,
    pub compiled: Option<Compiled>,
    /// The original tool definitions, for validating calls.
    pub tool_defs: Vec<ToolDef>,
}

/// Resolve a case's tool names against the file's catalog, in the case's order.
pub fn resolve_tools(names: &[String], catalog: &[Value]) -> Result<Vec<Value>, String> {
    names
        .iter()
        .map(|name| {
            catalog
                .iter()
                .find(|t| t["function"]["name"].as_str() == Some(name))
                .cloned()
                .ok_or_else(|| format!("unknown tool name: {name}"))
        })
        .collect()
}

pub fn tool_defs(tools: &[Value]) -> Vec<ToolDef> {
    tools
        .iter()
        .map(|t| ToolDef {
            name: t["function"]["name"].as_str().unwrap_or("").to_owned(),
            description: t["function"]["description"].as_str().map(str::to_owned),
            parameters: t["function"].get("parameters").cloned(),
        })
        .collect()
}

fn eval_config() -> GatewayConfig {
    GatewayConfig {
        compact_tools_enabled: true,
        ..GatewayConfig::default()
    }
}

/// The native request for a case: reference-time system message first, then the case's
/// messages, the resolved tools, `temperature` and the output cap.
pub fn native_body(
    inputs: CaseInputs<'_>,
    catalog: &[Value],
    model: &str,
    max_output_tokens: u32,
) -> Result<(Value, Vec<Value>), String> {
    let tools = resolve_tools(inputs.tools, catalog)?;
    let mut messages = vec![json!({"role": "system", "content": REFERENCE_TIME_MESSAGE})];
    messages.extend(inputs.messages.iter().cloned());
    let body = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "temperature": TEMPERATURE,
        "max_tokens": max_output_tokens,
    });
    Ok((body, tools))
}

/// Parse a body into the IR the way the router does.
pub fn parse_request(body: &Value) -> Result<ChatRequest, String> {
    OpenAiInbound
        .parse_chat(body.clone())
        .map_err(|e| format!("request does not parse: {e}"))
}

/// Build the request the router would send: the same `plan`/`apply` the handler runs.
pub fn build_request(
    inputs: CaseInputs<'_>,
    catalog: &[Value],
    model: &str,
    max_output_tokens: u32,
) -> Result<BuiltRequest, String> {
    let (body, tools) = native_body(inputs, catalog, model, max_output_tokens)?;
    let defs = tool_defs(&tools);
    let mut req = parse_request(&body)?;
    let plan = compact_tools::plan_from_body(&req, &body, InboundFormat::OpenAi, &eval_config());
    let (compacted, bypass, compiled) = match plan {
        Plan::Apply(c) => {
            compact_tools::apply(&mut req, &c);
            (true, None, Some(c))
        }
        Plan::Bypass(b) => (false, Some(b.as_label()), None),
    };
    let body = serde_json::to_value(&req).map_err(|e| e.to_string())?;
    Ok(BuiltRequest {
        body,
        compacted,
        bypass,
        compiled,
        tool_defs: defs,
    })
}

/// Expected calls rendered in the call grammar, one per line.
pub fn render_calls(expected: &[ExpectedCall]) -> String {
    expected
        .iter()
        .map(|c| encode_call(&c.name, &c.arguments))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Decode rendered calls against the ORIGINAL schemas: `[{name, arguments}]` or `{"error"}`.
pub fn roundtrip(rendered: &str, tools: &[ToolDef]) -> Value {
    match decode_calls(rendered, tools) {
        Ok(d) => Value::Array(
            d.calls
                .into_iter()
                .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                .collect(),
        ),
        Err(e) => json!({"error": e.kind()}),
    }
}

/// Feed a decoder case chunk by chunk through `StreamDecoder`.
pub fn run_decoder_case(case: &DecoderCase, catalog: &[Value]) -> Value {
    let tools = match resolve_tools(&case.tools, catalog) {
        Ok(t) => tool_defs(&t),
        Err(e) => return json!({"error": "unknown_tool_name", "message": e}),
    };
    let mut decoder = match StreamDecoder::new(&tools) {
        Ok(d) => d,
        Err(e) => return json!({"error": e.kind()}),
    };
    for chunk in &case.chunks {
        if let Err(e) = decoder.push(chunk) {
            return json!({"error": e.kind()});
        }
    }
    match decoder.finish() {
        Ok(d) => {
            json!({"calls": d.calls.into_iter().map(|c| json!({"name": c.name, "arguments": c.arguments})).collect::<Vec<_>>()})
        }
        Err(e) => json!({"error": e.kind()}),
    }
}

/// One ordinary-case line (offline fields only).
pub fn case_line(case: &Case, catalog: &[Value], model: &str, max_output_tokens: u32) -> Value {
    let inputs = CaseInputs {
        tools: &case.tools,
        messages: &case.messages,
    };
    match build_request(inputs, catalog, model, max_output_tokens) {
        Ok(built) => {
            let rendered = render_calls(&case.expected);
            let mut line = json!({
                "id": case.id,
                "compact_request": built.body,
                "compacted": built.compacted,
                "rendered_calls": rendered,
                "roundtrip_calls": roundtrip(&rendered, &built.tool_defs),
            });
            if let Some(b) = built.bypass {
                line["bypass"] = json!(b);
            }
            line
        }
        Err(message) => json!({"id": case.id, "error": message}),
    }
}

pub fn decoder_line(case: &DecoderCase, catalog: &[Value]) -> Value {
    json!({"id": case.id, "decoded": run_decoder_case(case, catalog)})
}

/// Offline run: every case and decoder case, in file order, one line each.
pub fn run_offline(
    set: &EvalSet,
    model: &str,
    max_output_tokens: u32,
    out: &mut dyn Write,
) -> io::Result<()> {
    for case in &set.cases {
        write_line(out, &case_line(case, &set.tools, model, max_output_tokens))?;
    }
    for case in &set.decoder_cases {
        write_line(out, &decoder_line(case, &set.tools))?;
    }
    out.flush()
}

/// One JSONL line with recursively sorted keys, so the bytes do not depend on which
/// `serde_json` features the build happened to unify (the TOON comparator enables
/// `preserve_order` for example builds).
pub fn write_line(out: &mut dyn Write, line: &Value) -> io::Result<()> {
    writeln!(out, "{}", canonical_json(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EvalSet {
        serde_json::from_value(json!({
            "schema_version": "compact-tools-eval-v1",
            "tools": [
                {"type": "function", "function": {"name": "create_calendar_event", "description": "Create an event in the user's calendar.",
                    "parameters": {"type": "object", "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "duration_min": {"type": "integer", "description": "Duration in minutes"},
                        "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    }, "required": ["title", "start"]}}},
                {"type": "function", "function": {"name": "send_email", "description": "Send an email from the user's account.",
                    "parameters": {"type": "object", "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"}, "body": {"type": "string"}
                    }, "required": ["to", "subject", "body"]}}},
                {"type": "function", "function": {"name": "strict_tool", "strict": true,
                    "parameters": {"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"], "additionalProperties": false}}},
                {"type": "function", "function": {"name": "set_code",
                    "parameters": {"type": "object", "properties": {"code": {"type": "string", "pattern": "^[A-Z]{3}$"}}, "required": ["code"]}}}
            ],
            "cases": [
                {"id": "ct-001", "tools": ["create_calendar_event", "send_email"],
                 "messages": [{"role": "user", "content": "Book a design review Monday 3pm IST with riya@example.com"}],
                 "expected": [{"name": "create_calendar_event", "arguments": {"title": "Design review", "start": "2026-10-05T15:00:00+05:30", "attendees": ["riya@example.com"]}}],
                 "match": {"free_text_fields": ["title"]}},
                {"id": "ct-003", "tools": ["create_calendar_event"], "messages": [{"role": "user", "content": "What's the weather?"}], "expected": []},
                {"id": "ct-strict", "tools": ["strict_tool"], "messages": [{"role": "user", "content": "x"}], "expected": [{"name": "strict_tool", "arguments": {"x": "1"}}]},
                {"id": "ct-pattern", "tools": ["set_code"], "messages": [{"role": "user", "content": "set"}], "expected": [{"name": "set_code", "arguments": {"code": "not-three-caps"}}]},
                {"id": "ct-missing", "tools": ["nope"], "messages": [], "expected": []}
            ],
            "decoder_cases": [
                {"id": "dc-002", "tools": ["create_calendar_event"],
                 "chunks": ["<<ca", "ll create_calendar_event {\"title\":\"Ret", "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>", ">"],
                 "expected": {"calls": [{"name": "create_calendar_event", "arguments": {"title": "Retro", "start": "2026-10-04T10:00:00+05:30"}}]}},
                {"id": "dc-004", "tools": ["create_calendar_event"], "chunks": ["<<call delete_everything {}>>"], "expected": {"error": "unknown_tool"}},
                {"id": "dc-005", "tools": ["create_calendar_event"], "chunks": ["<<call create_calendar_event {\"start\":\"s\",\"visibility\":\"secret\"}>>"], "expected": {"error": "invalid_arguments"}},
                {"id": "dc-pattern", "tools": ["set_code"], "chunks": ["<<call set_code {\"code\":\"bad\"}>>"], "expected": {"error": "unsupported_schema"}}
            ]
        }))
        .unwrap()
    }

    fn lines(set: &EvalSet) -> Vec<Value> {
        let mut buf = Vec::new();
        run_offline(
            set,
            OFFLINE_MODEL_PLACEHOLDER,
            DEFAULT_MAX_OUTPUT_TOKENS,
            &mut buf,
        )
        .unwrap();
        String::from_utf8(buf)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn the_output_contract_is_exact_for_a_compacted_case() {
        let set = sample();
        let out = lines(&set);
        assert_eq!(out.len(), set.cases.len() + set.decoder_cases.len());
        let line = &out[0];
        assert_eq!(line["id"], "ct-001");
        assert_eq!(line["compacted"], json!(true));
        assert!(line.get("bypass").is_none());
        let req = &line["compact_request"];
        assert_eq!(req["model"], OFFLINE_MODEL_PLACEHOLDER);
        assert_eq!(req["temperature"], json!(0.0));
        assert_eq!(req["max_tokens"], json!(1024));
        assert!(req.get("tools").is_none());
        assert!(req.get("tool_choice").is_none());
        let messages = req["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[0],
            json!({"role": "system", "content": REFERENCE_TIME_MESSAGE})
        );
        assert_eq!(messages[1]["role"], "system");
        let catalog = messages[1]["content"].as_str().unwrap();
        assert!(catalog.starts_with(nasiko_tool_compact::HEADER));
        assert!(catalog.ends_with(nasiko_tool_compact::INSTRUCTIONS));
        assert!(catalog.contains("create_calendar_event(title:str 'Event title', start:datetime 'Start time, ISO 8601', attendees?:[str] 'Attendee emails', duration_min?:int 'Duration in minutes', visibility?:public|private) - Create an event in the user's calendar."));
        assert!(catalog.contains(
            "\nsend_email(to:[str], subject:str, body:str) - Send an email from the user's account."
        ));
        assert_eq!(
            messages[2]["content"],
            "Book a design review Monday 3pm IST with riya@example.com"
        );
        assert_eq!(
            line["rendered_calls"],
            json!(
                "<<call create_calendar_event {\"attendees\":[\"riya@example.com\"],\"start\":\"2026-10-05T15:00:00+05:30\",\"title\":\"Design review\"}>>"
            )
        );
        assert_eq!(
            line["roundtrip_calls"],
            json!([{"name": "create_calendar_event", "arguments": {"title": "Design review", "start": "2026-10-05T15:00:00+05:30", "attendees": ["riya@example.com"]}}])
        );
        let keys: Vec<&String> = line.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn a_no_call_case_renders_and_roundtrips_to_nothing() {
        let out = lines(&sample());
        assert_eq!(out[1]["id"], "ct-003");
        assert_eq!(out[1]["rendered_calls"], json!(""));
        assert_eq!(out[1]["roundtrip_calls"], json!([]));
    }

    #[test]
    fn bypassed_cases_report_the_native_request_and_still_validate_against_real_schemas() {
        let out = lines(&sample());
        // `strict` is dropped by the IR; the raw body says so, so the request stays native, but
        // the schema itself is supported and the round trip is a real schema-validated decode.
        let strict = &out[2];
        assert_eq!(strict["compacted"], json!(false));
        assert_eq!(strict["bypass"], json!("strict_or_unknown_tool_keys"));
        assert_eq!(
            strict["compact_request"]["tools"][0]["function"]["name"],
            "strict_tool"
        );
        assert!(
            strict["compact_request"]["messages"]
                .as_array()
                .unwrap()
                .len()
                == 2
        );
        assert_eq!(
            strict["roundtrip_calls"],
            json!([{"name": "strict_tool", "arguments": {"x": "1"}}])
        );
        // An unsupported schema with violating arguments: no round trip is manufactured.
        let pattern = &out[3];
        assert_eq!(pattern["compacted"], json!(false));
        assert_eq!(pattern["bypass"], json!("unsupported_schema"));
        assert_eq!(
            pattern["roundtrip_calls"],
            json!({"error": "unsupported_schema"})
        );
        // An unknown tool name is reported, and the run goes on.
        assert_eq!(out[4]["id"], "ct-missing");
        assert!(out[4]["error"].as_str().unwrap().contains("nope"));
        assert!(out[4].get("compact_request").is_none());
    }

    #[test]
    fn decoder_cases_are_fed_chunk_by_chunk_and_unsupported_catalogs_are_errors() {
        let set = sample();
        let out = lines(&set);
        let n = set.cases.len();
        assert_eq!(
            out[n],
            json!({"id": "dc-002", "decoded": {"calls": [{"name": "create_calendar_event", "arguments": {"title": "Retro", "start": "2026-10-04T10:00:00+05:30"}}]}})
        );
        assert_eq!(
            out[n + 1],
            json!({"id": "dc-004", "decoded": {"error": "unknown_tool"}})
        );
        assert_eq!(
            out[n + 2],
            json!({"id": "dc-005", "decoded": {"error": "invalid_arguments"}})
        );
        assert_eq!(
            out[n + 3],
            json!({"id": "dc-pattern", "decoded": {"error": "unsupported_schema"}})
        );
    }

    #[test]
    fn two_runs_are_byte_identical() {
        let set = sample();
        let mut a = Vec::new();
        let mut b = Vec::new();
        run_offline(&set, OFFLINE_MODEL_PLACEHOLDER, 1024, &mut a).unwrap();
        run_offline(&set, OFFLINE_MODEL_PLACEHOLDER, 1024, &mut b).unwrap();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn expected_answers_cannot_influence_the_request() {
        let mut set = sample();
        let before = lines(&set);
        for case in &mut set.cases {
            case.expected = vec![ExpectedCall {
                name: "send_email".into(),
                arguments: json!({"to": ["x@y"], "subject": "changed", "body": "changed"}),
            }];
        }
        let after = lines(&set);
        for (b, a) in before.iter().zip(after.iter()) {
            assert_eq!(b["compact_request"], a["compact_request"]);
            assert_eq!(b["compacted"], a["compacted"]);
            assert_eq!(b.get("bypass"), a.get("bypass"));
        }
    }

    #[test]
    fn the_model_comes_from_the_argument_not_the_case() {
        let set = sample();
        let line = case_line(&set.cases[0], &set.tools, "some-model", 7);
        assert_eq!(line["compact_request"]["model"], "some-model");
        assert_eq!(line["compact_request"]["max_tokens"], json!(7));
    }
}

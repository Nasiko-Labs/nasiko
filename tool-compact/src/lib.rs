//! Compact tool schemas without breaking tool calls.
//!
//! Provides schema compaction and a streaming decoder that transforms OpenAI-shaped
//! tool definitions into compact prompt definitions and decodes `<<call name {json}>>`
//! model replies back into standard [`ToolCall`] records.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod error;
pub mod json;
pub mod parse_compact;
pub mod schema;
pub mod types;
pub mod validate;

pub use error::Error;
pub use parse_compact::{decode_tools, parse_compact_definitions};
pub use schema::{Field, StrFormat, ToolSchema, Ty};
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolDef};

use serde_json::Value;

const MARKER: &[u8; 6] = b"<<call";
const MAX_CALL_BYTES: usize = 262_144; // 256 KiB
const MAX_DEPTH: usize = 64;
const MAX_CALLS: usize = 128;
const CALL_INSTRUCTION: &str = "To call a tool, emit: <<call name {json args}>>";

/// Render a tool call into the compact wire format: `<<call name {json args}>>`.
pub fn render_call(name: &str, arguments: &Value) -> String {
    let args_str = serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string());
    format!("<<call {name} {args_str}>>")
}

/// Encode OpenAI-shaped tool definitions into compact prompt definitions.
///
/// Returns `Err(Error::UnsupportedSchema)` if any tool uses schema keywords
/// not supported by the compact format, signalling the caller to bypass compaction.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    if tools.is_empty() {
        return Ok(CompactTools {
            definitions: String::new(),
            instructions: String::new(),
        });
    }

    let mut lines = Vec::new();
    for tool in tools {
        let canonical = schema::ToolSchema::from_tool_def(tool)?;
        lines.push(canonical.encode_compact());
    }

    Ok(CompactTools {
        definitions: lines.join("\n"),
        instructions: CALL_INSTRUCTION.to_string(),
    })
}

/// Decode all tool calls from text using the provided tool definitions.
///
/// Implemented as a thin wrapper over [`StreamDecoder`] to guarantee chunk-invariance.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, Error> {
    let mut decoder = StreamDecoder::new(tools)?;
    decoder.push(text)?;
    decoder.finish()
}

/// Streaming parser and validator for compact tool calls.
pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    state: DecoderState,
    calls: Vec<ToolCall>,
    last_byte: Option<u8>,
}

#[derive(Debug, Clone)]
enum DecoderState {
    Text {
        matched: usize,
    },
    AfterMarker,
    BeforeName,
    Name {
        buf: Vec<u8>,
    },
    BeforeJson {
        tool_name: String,
    },
    Json {
        tool_name: String,
        depth: usize,
        buf: Vec<u8>,
        in_string: bool,
        escape: bool,
    },
    AfterJson {
        tool_name: String,
        json_buf: Vec<u8>,
    },
    AfterJson2 {
        tool_name: String,
        json_buf: Vec<u8>,
    },
    Poisoned(Error),
}

impl<'a> StreamDecoder<'a> {
    /// Create a new streaming decoder against the provided tool definitions.
    pub fn new(tools: &'a [ToolDef]) -> Result<Self, Error> {
        Ok(Self {
            tools,
            state: DecoderState::Text { matched: 0 },
            calls: Vec::new(),
            last_byte: None,
        })
    }

    /// Feed a chunk of model output into the decoder.
    pub fn push(&mut self, chunk: &str) -> Result<(), Error> {
        for &byte in chunk.as_bytes() {
            if let Err(e) = self.step(byte) {
                self.state = DecoderState::Poisoned(e.clone());
                return Err(e);
            }
            self.last_byte = Some(byte);
        }
        Ok(())
    }

    fn step(&mut self, byte: u8) -> Result<(), Error> {
        match &mut self.state {
            DecoderState::Poisoned(e) => Err(e.clone()),
            DecoderState::Text { matched } => {
                if byte == MARKER[*matched] {
                    *matched += 1;
                    if *matched == MARKER.len() {
                        self.state = DecoderState::AfterMarker;
                    }
                } else if byte == b'<' {
                    if self.last_byte == Some(b'<') {
                        *matched = 2;
                    } else {
                        *matched = 1;
                    }
                } else {
                    *matched = 0;
                }
                Ok(())
            }
            DecoderState::AfterMarker => {
                if byte.is_ascii_whitespace() {
                    self.state = DecoderState::BeforeName;
                    Ok(())
                } else {
                    Err(Error::MalformedCall(
                        "expected whitespace after <<call".to_string(),
                    ))
                }
            }
            DecoderState::BeforeName => {
                if byte.is_ascii_whitespace() {
                    Ok(())
                } else if is_ident_byte(byte) {
                    self.state = DecoderState::Name { buf: vec![byte] };
                    Ok(())
                } else {
                    Err(Error::MalformedCall("expected tool name".to_string()))
                }
            }
            DecoderState::Name { buf } => {
                if is_ident_byte(byte) {
                    buf.push(byte);
                    Ok(())
                } else {
                    let tool_name = String::from_utf8(buf.clone()).map_err(|_| {
                        Error::MalformedCall("invalid tool name encoding".to_string())
                    })?;

                    // Check if tool name is known
                    if !self.tools.iter().any(|t| t.function.name == tool_name) {
                        return Err(Error::UnknownTool(tool_name));
                    }

                    if byte == b'{' {
                        self.state = DecoderState::Json {
                            tool_name,
                            depth: 1,
                            buf: vec![b'{'],
                            in_string: false,
                            escape: false,
                        };
                        Ok(())
                    } else if byte.is_ascii_whitespace() {
                        self.state = DecoderState::BeforeJson { tool_name };
                        Ok(())
                    } else {
                        Err(Error::MalformedCall(
                            "expected '{' or whitespace after tool name".to_string(),
                        ))
                    }
                }
            }
            DecoderState::BeforeJson { tool_name } => {
                if byte.is_ascii_whitespace() {
                    Ok(())
                } else if byte == b'{' {
                    let name = tool_name.clone();
                    self.state = DecoderState::Json {
                        tool_name: name,
                        depth: 1,
                        buf: vec![b'{'],
                        in_string: false,
                        escape: false,
                    };
                    Ok(())
                } else {
                    Err(Error::MalformedCall("expected '{'".to_string()))
                }
            }
            DecoderState::Json {
                tool_name,
                depth,
                buf,
                in_string,
                escape,
            } => {
                if buf.len() >= MAX_CALL_BYTES {
                    return Err(Error::LimitExceeded("call arguments exceeded 256 KiB"));
                }
                if *depth > MAX_DEPTH {
                    return Err(Error::LimitExceeded("nesting depth exceeded 64"));
                }

                buf.push(byte);
                if *in_string {
                    if *escape {
                        *escape = false;
                    } else if byte == b'\\' {
                        *escape = true;
                    } else if byte == b'"' {
                        *in_string = false;
                    }
                } else if byte == b'"' {
                    *in_string = true;
                } else if byte == b'{' {
                    *depth += 1;
                } else if byte == b'}' {
                    *depth -= 1;
                    if *depth == 0 {
                        let name = tool_name.clone();
                        let json_data = buf.clone();
                        self.state = DecoderState::AfterJson {
                            tool_name: name,
                            json_buf: json_data,
                        };
                    }
                }
                Ok(())
            }
            DecoderState::AfterJson {
                tool_name,
                json_buf,
            } => {
                if byte.is_ascii_whitespace() {
                    Ok(())
                } else if byte == b'>' {
                    let name = tool_name.clone();
                    let data = json_buf.clone();
                    self.state = DecoderState::AfterJson2 {
                        tool_name: name,
                        json_buf: data,
                    };
                    Ok(())
                } else {
                    Err(Error::MalformedCall(
                        "expected '>>' closing call".to_string(),
                    ))
                }
            }
            DecoderState::AfterJson2 {
                tool_name,
                json_buf,
            } => {
                if byte == b'>' {
                    if self.calls.len() >= MAX_CALLS {
                        return Err(Error::LimitExceeded("calls count exceeded 128"));
                    }
                    let name = tool_name.clone();
                    let buf = json_buf.clone();
                    self.state = DecoderState::Text { matched: 0 };
                    self.last_byte = None;
                    let call = process_call(self.tools, self.calls.len() + 1, &name, &buf)?;
                    self.calls.push(call);
                    Ok(())
                } else {
                    Err(Error::MalformedCall(
                        "expected second '>' closing call".to_string(),
                    ))
                }
            }
        }
    }

    /// Finalize parsing and return all accumulated valid tool calls.
    pub fn finish(self) -> Result<Vec<ToolCall>, Error> {
        match self.state {
            DecoderState::Text { .. } => Ok(self.calls),
            DecoderState::Poisoned(e) => Err(e),
            _ => Err(Error::UnterminatedCall),
        }
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn process_call(
    tools: &[ToolDef],
    call_id_num: usize,
    tool_name: &str,
    json_buf: &[u8],
) -> Result<ToolCall, Error> {
    let json_str = std::str::from_utf8(json_buf)
        .map_err(|_| Error::InvalidArguments("invalid utf-8 in call arguments".to_string()))?;

    // 1. Strict duplicate key check
    json::check_duplicate_keys(json_str)?;

    // 2. Parse JSON
    let value: Value =
        serde_json::from_str(json_str).map_err(|e| Error::InvalidArguments(e.to_string()))?;

    let obj = value.as_object().ok_or_else(|| {
        Error::InvalidArguments("tool arguments must be a JSON object".to_string())
    })?;

    // 3. Find tool definition
    let tool_def = tools
        .iter()
        .find(|t| t.function.name == tool_name)
        .ok_or_else(|| Error::UnknownTool(tool_name.to_string()))?;

    // 4. Validate against parameters schema
    if let Some(Value::Object(param_map)) = &tool_def.function.parameters {
        validate::validate_arguments(tool_name, obj, param_map)?;
    }

    let canonical_args = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());

    Ok(ToolCall {
        id: format!("call_{call_id_num}"),
        kind: "function".to_string(),
        function: FunctionCall {
            name: tool_name.to_string(),
            arguments: canonical_args,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::is_redundant_field_description;
    use serde_json::json;

    fn sample_calendar_tool() -> ToolDef {
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "create_calendar_event".to_string(),
                description: Some("Create an event in the user's calendar.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "duration_min": {"type": "integer", "description": "Duration in minutes"},
                        "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
        }
    }

    fn sample_email_tool() -> ToolDef {
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "send_email".to_string(),
                description: Some("Send an email from the user's account.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                        "subject": {"type": "string", "description": "Subject line"},
                        "body": {"type": "string", "description": "Plain-text body"},
                        "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        }
    }

    #[test]
    fn test_encode_sample_tools() {
        let tools = vec![sample_calendar_tool(), sample_email_tool()];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.definitions.contains("create_calendar_event("));
        assert!(compact.definitions.contains("title:str"));
        assert!(compact.definitions.contains("start:datetime"));
        assert!(compact.definitions.contains("duration_min?:int"));
        assert!(compact.definitions.contains("attendees?:[str]"));
        assert!(compact.definitions.contains("visibility?:public|private"));
        assert!(compact.definitions.contains("send_email("));
        assert!(compact.definitions.contains("to:[str]"));
        assert!(compact.definitions.contains("subject:str"));
        assert!(compact.definitions.contains("body:str"));
        assert_eq!(compact.instructions, CALL_INSTRUCTION);
    }

    #[test]
    fn test_deterministic_encoding() {
        let tools = vec![sample_calendar_tool(), sample_email_tool()];
        let c1 = encode_tools(&tools).unwrap();
        let c2 = encode_tools(&tools).unwrap();
        assert_eq!(c1.render(), c2.render());
    }

    #[test]
    fn test_description_r1_drop() {
        // "Event title" for field "title" in "create_calendar_event" drops
        assert!(is_redundant_field_description(
            "create_calendar_event",
            "title",
            "Event title"
        ));
    }

    #[test]
    fn test_description_r1_keep() {
        // "Start time, ISO 8601" keeps because ISO/8601 are non-redundant
        assert!(!is_redundant_field_description(
            "create_calendar_event",
            "start",
            "Start time, ISO 8601"
        ));
    }

    #[test]
    fn test_unsupported_schema_bypasses() {
        let mut tool = sample_calendar_tool();
        tool.function.parameters = Some(json!({
            "type": "object",
            "anyOf": [{"properties": {"a": {"type": "string"}}}]
        }));
        let res = encode_tools(&[tool]);
        assert!(matches!(res, Err(Error::UnsupportedSchema { .. })));
    }

    #[test]
    fn test_dc_001_valid_single_call() {
        let tools = vec![sample_calendar_tool()];
        let text = "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
    }

    #[test]
    fn test_dc_002_marker_split_across_chunks() {
        let tools = vec![sample_calendar_tool()];
        let chunks = [
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];
        let mut decoder = StreamDecoder::new(&tools).unwrap();
        for chunk in chunks {
            decoder.push(chunk).unwrap();
        }
        let calls = decoder.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], "Retro");
    }

    #[test]
    fn test_dc_003_gt_inside_string_argument() {
        let tools = vec![sample_email_tool()];
        let text = "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["subject"], "a >> b");
    }

    #[test]
    fn test_dc_004_unknown_tool() {
        let tools = vec![sample_calendar_tool()];
        let text = "<<call delete_everything {}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.code(), "unknown_tool");
        assert!(matches!(err, Error::UnknownTool(_)));
    }

    #[test]
    fn test_dc_005_missing_required_and_bad_enum() {
        let tools = vec![sample_calendar_tool()];
        let text = "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.code(), "invalid_arguments");
    }

    #[test]
    fn test_plain_answer_no_calls() {
        let tools = vec![sample_calendar_tool()];
        let text = "I cannot fulfill this request as I am just an AI.";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_multi_call_in_text() {
        let tools = vec![sample_email_tool(), sample_calendar_tool()];
        let text = "I will send the email:\n<<call send_email {\"to\":[\"a@b.com\"],\"subject\":\"s\",\"body\":\"b\"}>>\nand book it:\n<<call create_calendar_event {\"title\":\"T\",\"start\":\"2026-10-05T15:00:00Z\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "send_email");
        assert_eq!(calls[1].function.name, "create_calendar_event");
    }

    #[test]
    fn test_duplicate_key_rejected() {
        let tools = vec![sample_calendar_tool()];
        let text = "<<call create_calendar_event {\"title\":\"A\",\"title\":\"B\",\"start\":\"2026-10-05T15:00:00Z\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.code(), "invalid_arguments");
    }

    #[test]
    fn test_integer_strictness() {
        let tools = vec![sample_calendar_tool()];
        let text = "<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-05T15:00:00Z\",\"duration_min\":30.5}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.code(), "invalid_arguments");
    }

    #[test]
    fn test_triple_lt_overlap() {
        let tools = vec![sample_calendar_tool()];
        let text =
            "<<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-05T15:00:00Z\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn test_eval_sample_suite() {
        use std::collections::BTreeMap;
        use std::fs::File;
        use std::io::BufReader;

        let path = "compact-tools-eval.json";
        if !std::path::Path::new(path).exists() {
            return;
        }

        let file = File::open(path).unwrap();
        let val: Value = serde_json::from_reader(BufReader::new(file)).unwrap();

        let raw_tools = val["tools"].as_array().unwrap();
        let mut tool_index: BTreeMap<String, ToolDef> = BTreeMap::new();
        for t in raw_tools {
            let def: ToolDef = serde_json::from_value(t.clone()).unwrap();
            tool_index.insert(def.function.name.clone(), def);
        }

        // Test ct-001, ct-002, ct-003
        let cases = val["cases"].as_array().unwrap();
        for case in cases {
            let cid = case["id"].as_str().unwrap();
            let case_tool_names = case["tools"].as_array().unwrap();
            let defs: Vec<ToolDef> = case_tool_names
                .iter()
                .map(|n| tool_index[n.as_str().unwrap()].clone())
                .collect();

            let _compact = encode_tools(&defs).unwrap();
            let expected_calls = case["expected"].as_array().unwrap();
            let rendered = expected_calls
                .iter()
                .map(|e| render_call(e["name"].as_str().unwrap(), &e["arguments"]))
                .collect::<Vec<_>>()
                .join("\n");

            let decoded = decode_calls(&rendered, &defs).unwrap();
            assert_eq!(
                decoded.len(),
                expected_calls.len(),
                "case {cid} length mismatch"
            );
            for (actual, exp) in decoded.iter().zip(expected_calls.iter()) {
                assert_eq!(actual.function.name, exp["name"].as_str().unwrap());
                let actual_args: Value = serde_json::from_str(&actual.function.arguments).unwrap();
                assert_eq!(
                    actual_args, exp["arguments"],
                    "case {cid} arguments mismatch"
                );
            }
        }

        // Test dc-001 .. dc-005
        let dc_cases = val["decoder_cases"].as_array().unwrap();
        for dc in dc_cases {
            let dc_id = dc["id"].as_str().unwrap();
            let dc_tool_names = dc["tools"].as_array().unwrap();
            let defs: Vec<ToolDef> = dc_tool_names
                .iter()
                .map(|n| tool_index[n.as_str().unwrap()].clone())
                .collect();

            let mut decoder = StreamDecoder::new(&defs).unwrap();
            let chunks = dc["chunks"].as_array().unwrap();
            for ch in chunks {
                if decoder.push(ch.as_str().unwrap()).is_err() {
                    break;
                }
            }

            let exp = &dc["expected"];
            if let Some(exp_err) = exp.get("error").and_then(Value::as_str) {
                let err = decoder.finish().unwrap_err();
                assert_eq!(
                    err.code(),
                    exp_err,
                    "decoder case {dc_id} error code mismatch"
                );
            } else if let Some(exp_calls) = exp.get("calls").and_then(Value::as_array) {
                let calls = decoder.finish().unwrap();
                assert_eq!(
                    calls.len(),
                    exp_calls.len(),
                    "decoder case {dc_id} call count mismatch"
                );
                for (actual, exp_call) in calls.iter().zip(exp_calls.iter()) {
                    assert_eq!(actual.function.name, exp_call["name"].as_str().unwrap());
                    let actual_args: Value =
                        serde_json::from_str(&actual.function.arguments).unwrap();
                    assert_eq!(
                        actual_args, exp_call["arguments"],
                        "decoder case {dc_id} args mismatch"
                    );
                }
            }
        }
    }
}

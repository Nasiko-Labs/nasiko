//! Decoding: text with `<<call name {json}>>` markers → [`Vec<ToolCall>`].
//!
//! Both [`decode_calls`] and [`StreamDecoder`] share the same [`crate::scan`]
//! core so their behaviour is identical regardless of how the input is chunked.

use serde_json::Map;

use crate::scan::{
    PrefixScanner, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_DEPTH,
};
use crate::types::{FunctionCall, ToolCall, ToolDef};
use crate::validate;
use crate::{Error, Result};

/// Placeholder call id. The router replaces this with a real UUID before
/// forwarding to the provider.
pub(crate) const PLACEHOLDER_ID: &str = "call_compact_0";

// ── decode_calls ─────────────────────────────────────────────────────────────

/// Decode all `<<call name {json}>>` markers in `text` and validate each call
/// against the corresponding tool schema.
///
/// # Failure policy
///
/// If **any** call is invalid (unknown tool, type mismatch, missing required
/// field, etc.) the entire result is an error. There are no partial results.
///
/// # Determinism
///
/// The returned `arguments` string preserves the key order emitted by the model.
/// No sorting is performed.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::with_limits(DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_DEPTH);
    decoder.push(text)?;
    decoder.finish(tools)
}

// ── StreamDecoder ─────────────────────────────────────────────────────────────

/// Incremental decoder that handles call markers split at arbitrary byte
/// boundaries (including mid-`<<`, mid-name, mid-string, mid-escape, and
/// split multibyte UTF-8).
///
/// # Usage
///
/// ```rust
/// # use nasiko_tool_compact::{StreamDecoder, ToolDef};
/// # let tools: Vec<ToolDef> = vec![];
/// let mut dec = StreamDecoder::new();
/// dec.push("<<call ping {").unwrap();
/// dec.push("}}").unwrap();  // note: this is actually closing braces
/// // dec.finish(&tools) returns the completed calls.
/// ```
///
/// # Sharing the scanner core
///
/// Both [`decode_calls`] and `StreamDecoder` feed bytes into the same
/// [`crate::scan::PrefixScanner`], guaranteeing identical results regardless of
/// chunking.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    scanner: PrefixScanner,
    /// Calls collected so far (not yet validated).
    pending: Vec<RawCall>,
    max_body_bytes: usize,
    max_depth: usize,
}

#[derive(Debug, Clone)]
struct RawCall {
    name: String,
    body: String,
}

impl StreamDecoder {
    /// Create a decoder with default limits.
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_DEPTH)
    }

    /// Create a decoder with custom byte and depth limits.
    pub fn with_limits(max_body_bytes: usize, max_depth: usize) -> Self {
        Self {
            scanner: PrefixScanner::new(),
            pending: Vec::new(),
            max_body_bytes,
            max_depth,
        }
    }

    /// Push a chunk of text into the decoder.
    ///
    /// May return an error if a call body exceeds the configured byte or depth limit.
    /// Errors from one chunk do not corrupt state for subsequent pushes — the scanner
    /// resets to its scanning state on error.
    pub fn push(&mut self, chunk: &str) -> Result<()> {
        for byte in chunk.bytes() {
            match self
                .scanner
                .push_byte(byte, self.max_body_bytes, self.max_depth)
            {
                Ok(Some(found)) => {
                    self.pending.push(RawCall {
                        name: found.name,
                        body: found.body,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    // Reset scanner so the stream stays usable.
                    self.scanner = PrefixScanner::new();
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Finalise decoding and validate all collected calls against `tools`.
    ///
    /// Returns [`Error::UnterminatedCall`] if the scanner is currently in the
    /// middle of parsing a call body (i.e. the stream ended before `>>`).
    ///
    /// Returns an error if any call fails validation.
    ///
    /// After `finish`, the decoder is consumed (moved).
    pub fn finish(self, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
        if self.scanner.in_progress() {
            return Err(Error::UnterminatedCall);
        }

        let mut result = Vec::with_capacity(self.pending.len());
        for raw in self.pending {
            let call = build_call(tools, &raw.name, &raw.body)?;
            result.push(call);
        }
        Ok(result)
    }
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_call(tools: &[ToolDef], name: &str, body: &str) -> Result<ToolCall> {
    // Find the tool definition.
    let tool = tools
        .iter()
        .find(|t| t.function.name == name)
        .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

    // Parse the JSON body.
    let args: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        Error::InvalidCall(format!("JSON parse error in call to {:?}: {}", name, e))
    })?;

    if !args.is_object() {
        return Err(Error::InvalidCall(format!(
            "arguments for {:?} must be a JSON object, got {}",
            name,
            args_type_name(&args)
        )));
    }

    // Validate against the schema.
    if let Some(schema) = &tool.function.parameters {
        validate::validate(name, &args, schema)?;
    }

    // Re-serialize arguments as a compact JSON string (preserves key order).
    let arguments = serde_json::to_string(&args).map_err(|e| {
        Error::InvalidCall(format!("failed to re-serialize arguments: {}", e))
    })?;

    Ok(ToolCall {
        id: PLACEHOLDER_ID.to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments,
        },
        extra: Map::new(),
    })
}

fn args_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn ping_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "ping".into(),
                description: None,
                parameters: Some(json!({"type":"object","properties":{}})),
            },
            extra: Default::default(),
        }
    }

    fn email_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to":      {"type": "string"},
                        "subject": {"type": "string"},
                        "body":    {"type": "string"}
                    },
                    "required": ["to", "subject"]
                })),
            },
            extra: Default::default(),
        }
    }

    fn calendar_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title":       {"type": "string"},
                        "start_time":  {"type": "string", "format": "date-time"},
                        "duration_min":{"type": "integer"}
                    },
                    "required": ["title", "start_time", "duration_min"]
                })),
            },
            extra: Default::default(),
        }
    }

    #[test]
    fn simple_zero_arg_call() {
        let calls = decode_calls("<<call ping {}>>", &[ping_tool()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "ping");
        assert_eq!(calls[0].function.arguments, "{}");
    }

    #[test]
    fn call_with_args() {
        let calls = decode_calls(
            r#"<<call send_email {"to":"alice@example.com","subject":"Hello"}>>"#,
            &[email_tool()],
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        let args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["to"], "alice@example.com");
    }

    #[test]
    fn text_before_and_after_call() {
        let calls = decode_calls(
            r#"Sure, I will call: <<call ping {}>> — done."#,
            &[ping_tool()],
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn multiple_calls() {
        let tools = vec![ping_tool(), email_tool()];
        let text = r#"<<call ping {}>> and then <<call send_email {"to":"b@b.com","subject":"x"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "ping");
        assert_eq!(calls[1].function.name, "send_email");
    }

    #[test]
    fn plain_answer_no_calls() {
        let calls = decode_calls("The weather is fine today.", &[ping_tool()]).unwrap();
        assert_eq!(calls.len(), 0);
    }

    #[test]
    fn gt_inside_string_does_not_end_call() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"body": {"type": "string"}}
                })),
            },
            extra: Default::default(),
        };
        let calls = decode_calls(r#"<<call f {"body":"a >> b"}>>"#, &[tool]).unwrap();
        assert_eq!(calls.len(), 1);
        let args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["body"], "a >> b");
    }

    #[test]
    fn unknown_tool_returns_error() {
        let result = decode_calls("<<call nonexistent {}>>", &[ping_tool()]);
        assert!(matches!(result, Err(Error::UnknownTool(_))));
    }

    #[test]
    fn missing_required_field_is_error() {
        // send_email requires "to" and "subject"
        let result = decode_calls(
            r#"<<call send_email {"subject":"only subject"}>>"#,
            &[email_tool()],
        );
        assert!(matches!(result, Err(Error::InvalidArguments { .. })));
    }

    #[test]
    fn wrong_type_is_error() {
        // duration_min must be integer, "30" is a string.
        let result = decode_calls(
            r#"<<call create_calendar_event {"title":"t","start_time":"2026-10-03T10:00:00Z","duration_min":"30"}>>"#,
            &[calendar_tool()],
        );
        assert!(matches!(result, Err(Error::InvalidArguments { .. })));
    }

    #[test]
    fn invalid_json_is_error() {
        let result = decode_calls("<<call ping {bad json}>>", &[ping_tool()]);
        assert!(matches!(result, Err(Error::InvalidCall(_))));
    }

    #[test]
    fn unterminated_call_returns_error() {
        let mut dec = StreamDecoder::new();
        dec.push("<<call ping {").unwrap();
        let result = dec.finish(&[ping_tool()]);
        assert!(matches!(result, Err(Error::UnterminatedCall)));
    }

    #[test]
    fn stream_decoder_split_at_every_boundary() {
        let text = r#"<<call send_email {"to":"x@x.com","subject":"hi"}>>"#;
        let expected = decode_calls(text, &[email_tool()]).unwrap();

        // Split at every byte boundary.
        for split in 1..text.len() {
            let mut dec = StreamDecoder::new();
            dec.push(&text[..split]).unwrap();
            dec.push(&text[split..]).unwrap();
            let calls = dec.finish(&[email_tool()]).unwrap();
            assert_eq!(
                calls.len(),
                expected.len(),
                "split at {split}: wrong call count"
            );
            if !calls.is_empty() {
                assert_eq!(
                    calls[0].function.arguments,
                    expected[0].function.arguments,
                    "split at {split}: arguments differ"
                );
            }
        }
    }

    #[test]
    fn unicode_emoji_in_string_arg() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"msg": {"type": "string"}}
                })),
            },
            extra: Default::default(),
        };
        let text = "<<call f {\"msg\":\"Hello 🎉\"}>>".to_string();
        let calls = decode_calls(&text, &[tool]).unwrap();
        assert_eq!(calls.len(), 1);
        let args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["msg"], "Hello 🎉");
    }

    #[test]
    fn escaped_quote_in_string_arg() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"msg": {"type": "string"}}
                })),
            },
            extra: Default::default(),
        };
        let text = r#"<<call f {"msg":"say \"hello\""}>>"#;
        let calls = decode_calls(text, &[tool]).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn unicode_escape_in_string_arg() {
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"msg": {"type": "string"}}
                })),
            },
            extra: Default::default(),
        };
        let text = r#"<<call f {"msg":"\u00e9"}>>"#;
        let calls = decode_calls(text, &[tool]).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn all_invalid_means_whole_decode_errors() {
        let tools = vec![ping_tool(), email_tool()];
        // Second call is invalid (missing required "to").
        let text =
            r#"<<call ping {}>> <<call send_email {"subject":"oops"}>>"#;
        let result = decode_calls(text, &tools);
        assert!(
            result.is_err(),
            "should error on any invalid call, not return partial results"
        );
    }

    #[test]
    fn stray_double_lt_is_not_an_error() {
        let calls = decode_calls("<< hello world >>", &[ping_tool()]).unwrap();
        assert_eq!(calls.len(), 0);
    }

    #[test]
    fn empty_args_object_passes_no_required_check() {
        let calls = decode_calls("<<call ping {}>>", &[ping_tool()]).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn very_long_args_are_accepted_within_limit() {
        let val: String = "x".repeat(1000);
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"data": {"type": "string"}}
                })),
            },
            extra: Default::default(),
        };
        let text = format!(r#"<<call f {{"data":"{val}"}}>>"#);
        let calls = decode_calls(&text, &[tool]).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn call_inside_call_marker_in_string_does_not_nest() {
        // A `<<call` sequence appearing inside a string value must not start a new call.
        let tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "f".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}}
                })),
            },
            extra: Default::default(),
        };
        let text = r#"<<call f {"text":"<<call ping {}>>"}>>"#;
        let calls = decode_calls(text, &[tool]).unwrap();
        // Should decode the outer call only; the inner marker is in the string value.
        // (The inner text `<<call ping {}>>` ends up as the string value.)
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn valid_datetime_arg_passes() {
        let calls = decode_calls(
            r#"<<call create_calendar_event {"title":"mtg","start_time":"2026-10-03T10:00:00+05:30","duration_min":30}>>"#,
            &[calendar_tool()],
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn invalid_datetime_arg_fails() {
        let result = decode_calls(
            r#"<<call create_calendar_event {"title":"mtg","start_time":"2026-10-03","duration_min":30}>>"#,
            &[calendar_tool()],
        );
        assert!(matches!(result, Err(Error::InvalidArguments { .. })));
    }

    #[test]
    fn placeholder_id_is_set() {
        let calls = decode_calls("<<call ping {}>>", &[ping_tool()]).unwrap();
        assert_eq!(calls[0].id, PLACEHOLDER_ID);
        assert_eq!(calls[0].kind, "function");
    }
}

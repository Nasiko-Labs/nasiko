//! Incremental streaming decoder for chunked model output.
//!
//! The [`StreamDecoder`] buffers text chunks and extracts complete
//! `<<call ToolName {json}>>` markers as they become fully received.
//! It correctly handles markers split across arbitrary chunk boundaries.

use crate::decode::{CALL_CLOSE, CALL_OPEN};
use crate::error::DecodeError;
use crate::types::{FunctionCall, ToolCall, ToolDef, ToolSchema};

use std::collections::HashMap;

use serde_json::Value;

/// Result of attempting to extract a call from the buffer.
enum CallExtraction {
    /// No `<<call` opener found. Advance consumed_up_to by this amount.
    None(usize),
    /// Malformed or empty — skip to this absolute offset and continue.
    Skip(usize),
    /// Partial data — wait for more chunks.
    Incomplete,
    /// Complete call extracted with owned data.
    Found {
        name: String,
        args: String,
        marker_end: usize,
    },
}


/// Streaming decoder that handles `<<call ... >>` markers split across chunks.
///
/// # Usage
///
/// ```rust,ignore
/// let mut decoder = StreamDecoder::new(&tools, &compact.schemas);
/// for chunk in stream {
///     decoder.push(&chunk);
///     for call in decoder.flush() {
///         // Handle completed call
///     }
/// }
/// let remaining = decoder.finish()?;
/// ```
pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
    completed_calls: Vec<ToolCall>,
    call_counter: usize,
    /// Byte offset: everything before this in the buffer has been fully scanned
    /// and contained no un-paired `<<call` opener.
    consumed_up_to: usize,
}

impl StreamDecoder {
    /// Create a new streaming decoder for the given tool definitions.
    pub fn new(tools: &[ToolDef], _schemas: &[ToolSchema]) -> Self {
        Self {
            buffer: String::new(),
            tools: tools.to_vec(),
            completed_calls: Vec::new(),
            call_counter: 0,
            consumed_up_to: 0,
        }
    }

    /// Feed a text chunk. May be a partial marker, a complete call, or plain text.
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        self.scan_buffer();
    }

    /// Extract all fully-parsed calls accumulated so far. Drains the completed list.
    pub fn flush(&mut self) -> Vec<ToolCall> {
        std::mem::take(&mut self.completed_calls)
    }

    /// Finalize: parse any remaining buffer content.
    ///
    /// Returns an error if an incomplete `<<call` marker remains (the stream
    /// ended mid-marker).
    pub fn finish(mut self) -> Result<Vec<ToolCall>, DecodeError> {
        // One final scan in case the last chunk completed a marker.
        self.scan_buffer();

        // Check for an incomplete marker in the remaining buffer.
        let remaining = &self.buffer[self.consumed_up_to..];
        if remaining.contains("<<call ") {
            return Err(DecodeError::IncompleteMarker);
        }

        Ok(self.completed_calls)
    }

    /// Scan the buffer for complete `<<call ... >>` markers, validate each,
    /// and move `consumed_up_to` past fully processed regions.
    fn scan_buffer(&mut self) {
        // Clone tools to avoid holding an immutable borrow on self while mutating.
        let tools_snapshot = self.tools.clone();
        let schema_map = build_validation_map(&tools_snapshot);

        loop {
            // Phase 1: compute positions and extract owned data.
            // All borrows from self.buffer are confined inside this block.
            let extraction = self.extract_next_call();

            match extraction {
                CallExtraction::None(advance) => {
                    self.consumed_up_to += advance;
                    break;
                }
                CallExtraction::Skip(new_consumed) => {
                    self.consumed_up_to = new_consumed;
                    continue;
                }
                CallExtraction::Incomplete => break,
                CallExtraction::Found {
                    name,
                    args,
                    marker_end,
                } => {
                    if let Some(call) = self.try_decode_call(&name, &args, &schema_map) {
                        self.completed_calls.push(call);
                    }
                    self.consumed_up_to = marker_end;
                }
            }
        }
    }

    /// Attempt to extract the next complete call from the buffer.
    /// Returns owned data so borrows from `self.buffer` do not leak.
    fn extract_next_call(&self) -> CallExtraction {
        let buf = &self.buffer;
        let search_region = &buf[self.consumed_up_to..];

        let Some(open_rel) = search_region.find(CALL_OPEN) else {
            let margin = CALL_OPEN.len().saturating_sub(1);
            let safe = search_region.len().saturating_sub(margin);
            return CallExtraction::None(safe);
        };

        let open_abs = self.consumed_up_to + open_rel;
        let after_marker = open_abs + CALL_OPEN.len();

        let rest = &buf[after_marker..];
        let Some(name_end) = rest.find([' ', '{']) else {
            return CallExtraction::Incomplete;
        };
        let tool_name = rest[..name_end].trim();
        if tool_name.is_empty() {
            return CallExtraction::Skip(after_marker);
        }

        let json_rest = &rest[name_end..];
        let Some(json_start_rel) = json_rest.find('{') else {
            return CallExtraction::Incomplete;
        };
        let json_start = after_marker + name_end + json_start_rel;

        let json_region = &buf[json_start..];
        let Some(json_end_rel) = find_json_end(json_region) else {
            return CallExtraction::Incomplete;
        };
        let json_end = json_start + json_end_rel;

        let after_json = &buf[json_end..];
        let trimmed = after_json.trim_start();
        if !trimmed.starts_with(CALL_CLOSE) {
            if after_json.len() < CALL_CLOSE.len() + 4 {
                return CallExtraction::Incomplete;
            }
            return CallExtraction::Skip(json_end);
        }

        let close_offset = (after_json.len() - trimmed.len()) + CALL_CLOSE.len();
        let marker_end = json_end + close_offset;

        CallExtraction::Found {
            name: tool_name.to_string(),
            args: buf[json_start..json_end].to_string(),
            marker_end,
        }
    }

    /// Attempt to decode and validate a single call. Returns `None` on validation
    /// failure (logged but not fatal in streaming mode — individual errors don't
    /// abort the whole stream).
    fn try_decode_call(
        &mut self,
        tool_name: &str,
        args_str: &str,
        schema_map: &HashMap<&str, ValidationSchema>,
    ) -> Option<ToolCall> {
        // Check tool exists.
        let schema = schema_map.get(tool_name)?;

        // Unescape markers.
        let unescaped = args_str
            .replace("\\u003e\\u003e", ">>")
            .replace("\\u003c\\u003ccall", "<<call")
            .replace("\\u003E\\u003E", ">>")
            .replace("\\u003C\\u003Ccall", "<<call");

        // Parse JSON.
        let args: Value = serde_json::from_str(&unescaped).ok()?;

        // Validate required fields.
        let obj = args.as_object();
        for field in &schema.required_fields {
            if !obj.is_some_and(|o| o.contains_key(field.as_str())) {
                return None;
            }
        }

        // Validate enum values.
        if let Some(obj) = obj {
            for (field, allowed) in &schema.enum_values {
                if let Some(val) = obj.get(field.as_str()) {
                    let val_str = match val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if !allowed.contains(&val_str) {
                        return None;
                    }
                }
            }
        }

        let idx = self.call_counter;
        self.call_counter += 1;

        Some(ToolCall {
            id: format!("call_compact_{idx}"),
            kind: "function".into(),
            function: FunctionCall {
                name: tool_name.to_string(),
                arguments: serde_json::to_string(&args).unwrap_or_default(),
            },
            extra: serde_json::Map::new(),
        })
    }
}

/// Find the end of a JSON object (the byte *after* the matching `}`).
fn find_json_end(text: &str) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let bytes = text.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape_next = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }

    None
}

/// Lightweight validation info built from `ToolDef`.
struct ValidationSchema {
    required_fields: Vec<String>,
    enum_values: HashMap<String, Vec<String>>,
}

fn build_validation_map(tools: &[ToolDef]) -> HashMap<&str, ValidationSchema> {
    let mut map = HashMap::new();
    for tool in tools {
        let name = tool.function.name.as_str();
        let params = tool.function.parameters.as_ref();

        let required_fields = params
            .and_then(|p| p.get("required"))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let mut enum_values = HashMap::new();
        if let Some(props) = params
            .and_then(|p| p.get("properties"))
            .and_then(Value::as_object)
        {
            for (field_name, prop_schema) in props {
                if let Some(vals) = prop_schema.get("enum").and_then(Value::as_array) {
                    let allowed: Vec<String> = vals
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect();
                    if !allowed.is_empty() {
                        enum_values.insert(field_name.clone(), allowed);
                    }
                }
            }
        }

        map.insert(name, ValidationSchema {
            required_fields,
            enum_values,
        });
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn search_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "search".into(),
                description: Some("Search the web.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                })),
            },
            extra: serde_json::Map::new(),
        }
    }

    fn make_schemas(tools: &[ToolDef]) -> Vec<ToolSchema> {
        crate::encode_tools(tools).unwrap().schemas
    }

    #[test]
    fn stream_single_call_one_chunk() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push(r#"Hello! <<call search {"query":"rust"}>>"#);
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search");
    }

    #[test]
    fn stream_single_call_split_at_marker() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push("Hello! <<cal");
        assert!(dec.flush().is_empty());
        dec.push(r#"l search {"query":"rust"}>>"#);
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn stream_single_call_split_at_close_marker() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push(r#"<<call search {"query":"rust"}>"#);
        assert!(dec.flush().is_empty());
        dec.push(">");
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn stream_byte_by_byte() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        let full = r#"<<call search {"query":"test"}>>"#;
        for ch in full.chars() {
            dec.push(&ch.to_string());
        }
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search");
    }

    #[test]
    fn stream_multiple_calls() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push(r#"<<call search {"query":"a"}>> text <<call search {"query":"b"}>>"#);
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_compact_0");
        assert_eq!(calls[1].id, "call_compact_1");
    }

    #[test]
    fn stream_no_markers() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push("Just regular text, no tools.");
        let calls = dec.finish().unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn stream_incomplete_marker_is_error() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push(r#"<<call search {"query":"test"}"#); // No closing >>
        let result = dec.finish();
        assert!(result.is_err());
    }

    #[test]
    fn stream_flush_returns_completed_incrementally() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);

        dec.push(r#"<<call search {"query":"first"}>>"#);
        let batch1 = dec.flush();
        assert_eq!(batch1.len(), 1);

        dec.push(r#" <<call search {"query":"second"}>>"#);
        let batch2 = dec.flush();
        assert_eq!(batch2.len(), 1);

        let remaining = dec.finish().unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn stream_split_in_json_value() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push(r#"<<call search {"quer"#);
        assert!(dec.flush().is_empty());
        dec.push(r#"y":"hello world"}>>"#);
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn stream_split_in_tool_name() {
        let tools = vec![search_tool()];
        let schemas = make_schemas(&tools);
        let mut dec = StreamDecoder::new(&tools, &schemas);
        dec.push("<<call sear");
        assert!(dec.flush().is_empty());
        dec.push(r#"ch {"query":"x"}>>"#);
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search");
    }
}

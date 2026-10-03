use crate::{CompactError, Result, ToolCall, ToolDef};
use serde_json::Value;
use std::collections::HashMap;

const OPEN: &str = "<<call ";

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools.to_vec());
    let mut calls = decoder.push(text)?;
    calls.extend(decoder.finish()?);
    Ok(calls)
}

#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: HashMap<String, ToolDef>,
    buffer: String,
    setup_error: Option<CompactError>,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        let setup_error = crate::encode_tools(&tools).err();
        Self {
            tools: tools.into_iter().map(|t| (t.name.clone(), t)).collect(),
            buffer: String::new(),
            setup_error,
        }
    }

    /// Add a text chunk and return calls that are complete in the accumulated stream.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);
        self.drain(false)
    }

    /// Mark the stream complete. An unfinished call is reported as an error.
    pub fn finish(&mut self) -> Result<Vec<ToolCall>> {
        self.drain(true)
    }

    fn drain(&mut self, final_chunk: bool) -> Result<Vec<ToolCall>> {
        if let Some(error) = &self.setup_error {
            return Err(error.clone());
        }
        let mut calls = Vec::new();
        loop {
            let Some(start) = self.buffer.find(OPEN) else {
                if final_chunk {
                    self.buffer.clear();
                } else {
                    let keep = partial_suffix_len(&self.buffer, OPEN);
                    let n = self.buffer.len().saturating_sub(keep);
                    self.buffer.drain(..n);
                }
                return Ok(calls);
            };
            self.buffer.drain(..start + OPEN.len());
            let Some(space) = self.buffer.find(char::is_whitespace) else {
                if final_chunk {
                    return Err(CompactError::MalformedCall(
                        "missing tool name or arguments".into(),
                    ));
                }
                self.buffer.insert_str(0, OPEN);
                return Ok(calls);
            };
            let name = self.buffer[..space].to_owned();
            self.buffer.drain(..space);
            while self.buffer.starts_with(char::is_whitespace) {
                self.buffer.remove(0);
            }
            let Some((end, consumed)) = json_object_end(&self.buffer) else {
                if final_chunk {
                    return Err(CompactError::IncompleteCall);
                }
                self.buffer = format!("{OPEN}{name} {}", self.buffer);
                return Ok(calls);
            };
            let raw = self.buffer[..end].trim().to_owned();
            let value: Value =
                serde_json::from_str(&raw).map_err(|e| CompactError::InvalidJson(e.to_string()))?;
            if !value.is_object() {
                return Err(CompactError::MalformedCall(
                    "arguments must be a JSON object".into(),
                ));
            }
            let tail = &self.buffer[consumed..];
            let ws = tail.len() - tail.trim_start().len();
            if !tail[ws..].starts_with(">>") {
                if final_chunk {
                    return Err(CompactError::MalformedCall(
                        "expected >> after JSON arguments".into(),
                    ));
                }
                self.buffer = format!("{OPEN}{name} {}", self.buffer);
                return Ok(calls);
            }
            self.buffer.drain(..consumed + ws + 2);
            let tool = self
                .tools
                .get(&name)
                .ok_or_else(|| CompactError::UnknownTool(name.clone()))?;
            if let Some(schema) = &tool.parameters {
                validate(&value, schema, "$", &name)?;
            }
            calls.push(ToolCall {
                name,
                arguments: serde_json::to_string(&value)
                    .map_err(|e| CompactError::InvalidJson(e.to_string()))?,
            });
        }
    }
}

fn partial_suffix_len(s: &str, marker: &str) -> usize {
    (1..=s.len().min(marker.len()))
        .rev()
        .find(|&n| s.is_char_boundary(s.len() - n) && marker.starts_with(&s[s.len() - n..]))
        .unwrap_or(0)
}

/// Find the end of a JSON object, respecting quoted strings and escapes. The
/// returned second index is the first byte after the complete JSON value.
fn json_object_end(s: &str) -> Option<(usize, usize)> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (i, ch) in s[start..].char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + i + 1;
                    return Some((end, end));
                }
            }
            _ => {}
        }
    }
    None
}

fn validate(value: &Value, schema: &Value, path: &str, tool: &str) -> Result<()> {
    let fail = |reason: String| CompactError::InvalidArguments {
        tool: tool.into(),
        reason,
    };
    let obj = schema
        .as_object()
        .ok_or_else(|| fail(format!("{path}: schema must be an object")))?;
    if let Some(en) = obj.get("enum").and_then(Value::as_array) {
        if !en.contains(value) {
            return Err(fail(format!("{path}: value is not in enum")));
        }
    }
    match obj.get("type").and_then(Value::as_str).unwrap_or("") {
        "object" => {
            let got = value
                .as_object()
                .ok_or_else(|| fail(format!("{path}: expected object")))?;
            let props = obj.get("properties").and_then(Value::as_object);
            for required in obj
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !got.contains_key(required) {
                    return Err(fail(format!("{path}.{required}: required field missing")));
                }
            }
            for (key, val) in got {
                if let Some(child) = props.and_then(|p| p.get(key)) {
                    validate(val, child, &format!("{path}.{key}"), tool)?;
                } else if obj.get("additionalProperties") == Some(&Value::Bool(false)) {
                    return Err(fail(format!("{path}.{key}: unexpected field")));
                }
            }
        }
        "array" => {
            let arr = value
                .as_array()
                .ok_or_else(|| fail(format!("{path}: expected array")))?;
            if let Some(items) = obj.get("items") {
                for (i, item) in arr.iter().enumerate() {
                    validate(item, items, &format!("{path}[{i}]"), tool)?;
                }
            }
        }
        "string" => {
            if !value.is_string() {
                return Err(fail(format!("{path}: expected string")));
            }
        }
        "integer" => {
            let is_integer = value.as_i64().is_some()
                || value.as_u64().is_some()
                || value
                    .as_f64()
                    .is_some_and(|n| n.is_finite() && n.fract() == 0.0);
            if !is_integer {
                return Err(fail(format!("{path}: expected integer")));
            }
        }
        "number" => {
            if !value.is_number() {
                return Err(fail(format!("{path}: expected number")));
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err(fail(format!("{path}: expected boolean")));
            }
        }
        "null" => {
            if !value.is_null() {
                return Err(fail(format!("{path}: expected null")));
            }
        }
        "" => {}
        other => return Err(fail(format!("{path}: unsupported schema type {other}"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "create_event".into(),
            description: None,
            parameters: Some(json!({
                "type":"object", "properties":{"title":{"type":"string"}, "attendees":{"type":"array", "items":{"type":"string"}}}, "required":["title"], "additionalProperties":false
            })),
        }]
    }

    #[test]
    fn decodes_multiple_calls_and_ignores_surrounding_text() {
        let calls = decode_calls("hello <<call create_event {\"title\":\"one\"}>> middle <<call create_event {\"title\":\"two\"}>> bye", &tools()).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].arguments, r#"{"title":"two"}"#);
    }

    #[test]
    fn terminator_inside_string_does_not_end_call() {
        let calls = decode_calls(r#"<<call create_event {"title":"a >> b"}>>"#, &tools()).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].arguments).unwrap()["title"],
            "a >> b"
        );
    }

    #[test]
    fn rejects_bad_or_unknown_calls_and_invalid_arguments() {
        assert!(matches!(
            decode_calls("<<call nope {}>>", &tools()),
            Err(CompactError::UnknownTool(_))
        ));
        assert!(matches!(
            decode_calls("<<call create_event {\"attendees\":3}>>", &tools()),
            Err(CompactError::InvalidArguments { .. })
        ));
        assert!(decode_calls("<<call create_event {oops}>>", &tools()).is_err());
    }

    #[test]
    fn decoder_rejects_unsupported_tool_schema_and_non_integer_numbers() {
        let unsupported = vec![ToolDef {
            name: "create_event".into(),
            description: None,
            parameters: Some(json!({
                "type":"object", "properties":{"title":{"type":"string", "minLength":2}}
            })),
        }];
        assert!(matches!(
            decode_calls("<<call create_event {\"title\":\"ok\"}>>", &unsupported),
            Err(CompactError::UnsupportedSchema(_))
        ));

        let integer_tool = vec![ToolDef {
            name: "count".into(),
            description: None,
            parameters: Some(json!({
                "type":"object", "properties":{"value":{"type":"integer"}}, "required":["value"]
            })),
        }];
        assert!(decode_calls("<<call count {\"value\":3.0}>>", &integer_tool).is_ok());
        assert!(matches!(
            decode_calls("<<call count {\"value\":3.5}>>", &integer_tool),
            Err(CompactError::InvalidArguments { .. })
        ));
    }

    #[test]
    fn stream_handles_markers_split_across_chunks() {
        let mut stream = StreamDecoder::new(tools());
        assert!(stream.push("text <<ca").unwrap().is_empty());
        assert!(
            stream
                .push("ll create_event {\"title\":")
                .unwrap()
                .is_empty()
        );
        assert!(stream.push("\"ok >> yes\"}>").unwrap().is_empty());
        let calls = stream.push(">").unwrap();
        assert_eq!(calls.len(), 1);
        assert!(stream.finish().unwrap().is_empty());
    }

    #[test]
    fn plain_text_has_no_calls() {
        assert!(decode_calls("hello there", &tools()).unwrap().is_empty());
    }
}

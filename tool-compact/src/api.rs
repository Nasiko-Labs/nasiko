use crate::compact_decoder::decode_compact_calls;
use crate::compact_encoder::{schema_from_def, CompactTools};
use crate::error::{Result, ToolCompactError};
use crate::schema::ToolRegistry;
use crate::types::{FunctionCall, FunctionDef, ToolCall, ToolDef};
use serde_json::{json, Map, Value};

/// Decodes model output into OpenAI-shaped tool calls, validated against `tools`.
///
/// * Plain text with no `<<call ...>>` returns an empty list.
/// * Fail closed: unknown tool, bad JSON, missing required field, wrong type, enum or
///   date-time violation return an error, never a guessed call.
/// * Ids are placeholders (`call_1`, `call_2`, ...); the router assigns real ids.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut registry = ToolRegistry::new();
    for def in tools {
        registry.register(schema_from_def(def)?);
    }
    decode_compact_calls(text, &registry)?
        .into_iter()
        .enumerate()
        .map(|(i, (name, args))| -> Result<ToolCall> {
            let arguments = serde_json::to_string(&args)
                .map_err(|e| ToolCompactError::SerializationError(e.to_string()))?;
            Ok(ToolCall {
                id: format!("call_{}", i + 1),
                kind: "function".to_string(),
                function: FunctionCall { name, arguments },
                extra: Map::new(),
            })
        })
        .collect::<Result<Vec<ToolCall>>>()
}

/// Parses compact definitions back into OpenAI-shaped tool definitions.
///
/// Meaning is preserved (types, required vs optional, enums, nested objects, arrays,
/// date-time, descriptions). Key order, whitespace inside descriptions and the original
/// order of `required` are not.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    compact
        .definitions
        .iter()
        .map(|line| parse_tool_line(line))
        .collect()
}

struct Param {
    name: String,
    required: bool,
    schema: Value,
}

struct P<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> P<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, i: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
    }

    fn fail<T>(&self, msg: &str) -> Result<T> {
        Err(ToolCompactError::Malformed(format!(
            "compact definition: {msg} at byte {}",
            self.i
        )))
    }

    fn expect(&mut self, c: char) -> Result<()> {
        if self.eat(c) {
            Ok(())
        } else {
            self.fail(&format!("expected '{c}'"))
        }
    }

    fn ident(&mut self) -> Result<String> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            return self.fail("expected a name");
        }
        Ok(self.s[start..self.i].to_string())
    }

    fn params(&mut self, close: char) -> Result<Vec<Param>> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(close) {
                break;
            }
            out.push(self.param()?);
            self.skip_ws();
            if !self.eat(',') {
                break;
            }
        }
        self.skip_ws();
        self.expect(close)?;
        Ok(out)
    }

    fn param(&mut self) -> Result<Param> {
        let name = self.ident()?;
        let required = !self.eat('?');
        self.expect(':')?;
        let mut schema = self.ty()?;
        self.skip_ws();
        if self.eat('"') {
            let start = self.i;
            match self.s[start..].find('"') {
                Some(n) => {
                    let desc = self.s[start..start + n].to_string();
                    self.i = start + n + 1;
                    if let Value::Object(m) = &mut schema {
                        m.insert("description".to_string(), Value::String(desc));
                    }
                }
                None => return self.fail("unterminated description"),
            }
        }
        Ok(Param {
            name,
            required,
            schema,
        })
    }

    fn ty(&mut self) -> Result<Value> {
        if self.eat('[') {
            let item = self.ident()?;
            self.expect(']')?;
            return match prim_schema(&item) {
                Some(s) => Ok(json!({"type": "array", "items": s})),
                None => self.fail("array items must be primitive"),
            };
        }
        if self.eat('{') {
            let params = self.params('}')?;
            return Ok(object_schema(params));
        }
        let first = self.ident()?;
        if self.peek() == Some('|') {
            let mut vals = vec![Value::String(first)];
            while self.eat('|') {
                vals.push(Value::String(self.ident()?));
            }
            return Ok(json!({"type": "string", "enum": vals}));
        }
        if let Some(s) = prim_schema(&first) {
            return Ok(s);
        }
        if first == "object" {
            return Ok(json!({"type": "object"}));
        }
        Ok(json!({"type": "string", "enum": [first]}))
    }
}

fn prim_schema(t: &str) -> Option<Value> {
    Some(match t {
        "str" => json!({"type": "string"}),
        "int" => json!({"type": "integer"}),
        "num" => json!({"type": "number"}),
        "bool" => json!({"type": "boolean"}),
        "datetime" => json!({"type": "string", "format": "date-time"}),
        _ => return None,
    })
}

fn object_schema(params: Vec<Param>) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for p in params {
        if p.required {
            required.push(Value::String(p.name.clone()));
        }
        props.insert(p.name, p.schema);
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String("object".to_string()));
    obj.insert("properties".to_string(), Value::Object(props));
    if !required.is_empty() {
        obj.insert("required".to_string(), Value::Array(required));
    }
    Value::Object(obj)
}

fn parse_tool_line(line: &str) -> Result<ToolDef> {
    let mut p = P::new(line);
    let name = p.ident()?;
    p.expect('(')?;
    let params = p.params(')')?;
    let rest = &line[p.i..];
    let description = if rest.is_empty() {
        None
    } else if let Some(d) = rest.strip_prefix(" - ") {
        Some(d.to_string())
    } else {
        return p.fail("unexpected text after the parameter list");
    };
    let parameters = if params.is_empty() {
        None
    } else {
        Some(object_schema(params))
    };
    Ok(ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name,
            description,
            parameters,
            extra: Map::new(),
        },
        extra: Map::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compact_encoder::encode_tools;

    fn calendar() -> ToolDef {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "create_calendar_event",
                "description": "Create an event in the user's calendar.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "duration_min": {"type": "integer", "description": "Duration in minutes"},
                        "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn decodes_into_openai_shaped_calls() {
        let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>"#;
        let calls = decode_calls(text, &[calendar()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].kind, "function");
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(
            args,
            json!({"title": "Design review", "start": "2026-10-05T15:00:00+05:30", "attendees": ["riya@example.com"]})
        );
    }

    #[test]
    fn several_calls_get_sequential_ids() {
        let text = r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00+05:30"}>> <<call create_calendar_event {"title":"B","start":"2026-10-06T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &[calendar()]).unwrap();
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[1].id, "call_2");
    }

    #[test]
    fn plain_answer_returns_no_calls() {
        assert!(decode_calls("I cannot check the weather.", &[calendar()])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unknown_tool_fails_closed() {
        let err = decode_calls(r#"<<call get_weather {"city":"x"}>>"#, &[calendar()]).unwrap_err();
        assert!(matches!(err, ToolCompactError::UnknownTool(_)));
    }

    #[test]
    fn invalid_datetime_fails_closed() {
        let text = r#"<<call create_calendar_event {"title":"A","start":"tomorrow"}>>"#;
        let err = decode_calls(text, &[calendar()]).unwrap_err();
        assert!(matches!(err, ToolCompactError::InvalidArguments { .. }));
    }

    #[test]
    fn enum_violation_fails_closed() {
        let text = r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let err = decode_calls(text, &[calendar()]).unwrap_err();
        assert!(matches!(err, ToolCompactError::InvalidArguments { .. }));
    }

    #[test]
    fn decode_tools_preserves_schema_meaning() {
        let orig = calendar();
        let compact = encode_tools(&[orig.clone()]).unwrap();
        let back = decode_tools(&compact).unwrap();
        assert_eq!(
            schema_from_def(&orig).unwrap(),
            schema_from_def(&back[0]).unwrap()
        );
    }

    #[test]
    fn encoding_is_stable_after_a_round_trip() {
        let first = encode_tools(&[calendar()]).unwrap();
        let again = encode_tools(&decode_tools(&first).unwrap()).unwrap();
        assert_eq!(first, again);
    }

    #[test]
    fn nested_objects_round_trip() {
        let t: ToolDef = serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "search",
                "description": "Search places.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filter": {
                            "type": "object",
                            "properties": {"city": {"type": "string"}, "limit": {"type": "integer"}},
                            "required": ["city"]
                        }
                    },
                    "required": ["filter"]
                }
            }
        }))
        .unwrap();
        let back = decode_tools(&encode_tools(&[t.clone()]).unwrap()).unwrap();
        assert_eq!(
            schema_from_def(&t).unwrap(),
            schema_from_def(&back[0]).unwrap()
        );
    }

    #[test]
    fn tool_without_parameters_round_trips() {
        let t: ToolDef =
            serde_json::from_value(json!({"type":"function","function":{"name":"ping"}})).unwrap();
        let back = decode_tools(&encode_tools(&[t.clone()]).unwrap()).unwrap();
        assert_eq!(t, back[0]);
    }
}

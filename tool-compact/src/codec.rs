use crate::{CompactError, CompactTools, MAX_BYTES, MAX_DEPTH, ToolDef, check_tools, name_char};
use serde_json::{Map, Value};

fn json(v: &Value) -> String {
    serde_json::to_string(v).expect("JSON Value is serializable")
}
fn field(name: &str) -> String {
    if !name.is_empty() && name.bytes().all(name_char) {
        name.into()
    } else {
        json(&Value::String(name.into()))
    }
}

fn encode_schema(s: &Value, root: bool) -> String {
    let m = s.as_object().expect("checked schema");
    let kind = s["type"].as_str().expect("checked type");
    let mut out = match kind {
        "object" => {
            let mut out = if let Some(properties) = s.get("properties").and_then(Value::as_object) {
                let required: Vec<&str> = s
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|v| v.as_str().unwrap()).collect())
                    .unwrap_or_default();
                // Required order is preserved, including in decode_tools.
                let mut names = required.clone();
                names.extend(
                    properties
                        .keys()
                        .map(String::as_str)
                        .filter(|name| !required.contains(name)),
                );
                let fields: Vec<String> = names
                    .into_iter()
                    .map(|name| {
                        format!(
                            "{}{}:{}",
                            field(name),
                            if required.contains(&name) { "" } else { "?" },
                            encode_schema(&properties[name], false)
                        )
                    })
                    .collect();
                if root {
                    format!("({})", fields.join(","))
                } else {
                    format!("{{{}}}", fields.join(","))
                }
            } else {
                "obj".into()
            };
            if m.contains_key("required") {
                out.push('!');
            }
            out
        }
        "array" => s
            .get("items")
            .map(|v| format!("[{}]", encode_schema(v, false)))
            .unwrap_or("array".into()),
        "string" => {
            let mut out = String::from("str");
            if let Some(format) = s.get("format").and_then(Value::as_str) {
                out.push('/');
                out.push_str(format);
            }
            out
        }
        "integer" => "int".into(),
        "number" => "num".into(),
        "boolean" => "bool".into(),
        "null" => "null".into(),
        _ => unreachable!("checked type"),
    };
    if let Some(e) = s.get("enum") {
        out.push('=');
        out.push_str(&json(e));
    }
    if let Some(d) = s.get("description") {
        out.push('#');
        out.push_str(&json(d));
    }
    let annotations: Map<String, Value> = m
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "type" | "properties" | "required" | "items" | "format" | "enum" | "description"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !annotations.is_empty() {
        out.push('@');
        out.push_str(&json(&Value::Object(annotations)));
    }
    out
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    check_tools(tools)?;
    let mut definitions = String::from("ct1\n");
    for tool in tools {
        definitions.push_str(&tool.name);
        if tool.parameters.get("properties").is_none() {
            definitions.push(':');
        }
        definitions.push_str(&encode_schema(&tool.parameters, true));
        if let Some(description) = &tool.description {
            definitions.push_str(" - ");
            definitions.push_str(&json(&Value::String(description.clone())));
        }
        definitions.push('\n');
    }
    if definitions.len() > MAX_BYTES {
        return Err(CompactError::LimitExceeded);
    }
    Ok(CompactTools { definitions })
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }
    fn take(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn require(&mut self, b: u8) -> Result<(), CompactError> {
        if self.take(b) {
            Ok(())
        } else {
            Err(CompactError::MalformedOutput(
                "invalid compact schema syntax".into(),
            ))
        }
    }
    fn atom(&mut self) -> Result<String, CompactError> {
        let start = self.pos;
        while self.peek().is_some_and(name_char) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(CompactError::MalformedOutput("expected identifier".into()));
        }
        Ok(self.text[start..self.pos].into())
    }
    fn json(&mut self) -> Result<Value, CompactError> {
        let mut stream = serde_json::Deserializer::from_str(&self.text[self.pos..])
            .into_iter::<super::decode::UniqueValue>();
        let value = stream
            .next()
            .ok_or_else(|| CompactError::MalformedOutput("missing JSON".into()))?
            .map_err(|e| CompactError::MalformedOutput(e.to_string()))?;
        self.pos += stream.byte_offset();
        Ok(value.0)
    }
    fn schema(&mut self, depth: usize) -> Result<Value, CompactError> {
        if depth > MAX_DEPTH {
            return Err(CompactError::LimitExceeded);
        }
        let mut m = Map::new();
        if matches!(self.peek(), Some(b'{' | b'(')) {
            let closing = if self.take(b'(') {
                b')'
            } else {
                self.require(b'{')?;
                b'}'
            };
            m.insert("type".into(), Value::String("object".into()));
            let mut properties = Map::new();
            let mut required = Vec::new();
            if !self.take(closing) {
                loop {
                    let name = if self.peek() == Some(b'"') {
                        self.json()?
                            .as_str()
                            .ok_or_else(|| {
                                CompactError::MalformedOutput(
                                    "property name must be a string".into(),
                                )
                            })?
                            .to_string()
                    } else {
                        self.atom()?
                    };
                    let optional = self.take(b'?');
                    self.require(b':')?;
                    let schema = self.schema(depth + 1)?;
                    if properties.insert(name.clone(), schema).is_some() {
                        return Err(CompactError::MalformedOutput("duplicate property".into()));
                    }
                    if !optional {
                        required.push(Value::String(name));
                    }
                    if self.take(closing) {
                        break;
                    }
                    self.require(b',')?;
                }
            }
            m.insert("properties".into(), Value::Object(properties));
            if self.take(b'!') {
                m.insert("required".into(), Value::Array(required));
            } else if !required.is_empty() {
                return Err(CompactError::MalformedOutput(
                    "required marker missing".into(),
                ));
            }
        } else if self.take(b'[') {
            m.insert("type".into(), Value::String("array".into()));
            m.insert("items".into(), self.schema(depth + 1)?);
            self.require(b']')?;
        } else {
            let kind = self.atom()?;
            let native = match kind.as_str() {
                "obj" => "object",
                "array" => "array",
                "str" => "string",
                "int" => "integer",
                "num" => "number",
                "bool" => "boolean",
                "null" => "null",
                _ => return Err(CompactError::MalformedOutput("unknown schema type".into())),
            };
            m.insert("type".into(), Value::String(native.into()));
            if kind == "obj" && self.take(b'!') {
                m.insert("required".into(), Value::Array(vec![]));
            }
            if kind == "str" && self.take(b'/') {
                m.insert("format".into(), Value::String(self.atom()?));
            }
        }
        if self.take(b'=') {
            m.insert("enum".into(), self.json()?);
        }
        if self.take(b'#') {
            m.insert("description".into(), self.json()?);
        }
        if self.take(b'@') {
            let annotations = self
                .json()?
                .as_object()
                .ok_or_else(|| {
                    CompactError::MalformedOutput("annotations must be an object".into())
                })?
                .clone();
            for (key, value) in annotations {
                if matches!(
                    key.as_str(),
                    "type"
                        | "properties"
                        | "required"
                        | "items"
                        | "format"
                        | "enum"
                        | "description"
                ) || m.insert(key, value).is_some()
                {
                    return Err(CompactError::MalformedOutput(
                        "structural annotation override".into(),
                    ));
                }
            }
        }
        Ok(Value::Object(m))
    }
}

pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    if compact.definitions.len() > MAX_BYTES {
        return Err(CompactError::LimitExceeded);
    }
    let text = compact.catalog()?;
    let mut parser = Parser { text, pos: 0 };
    let mut tools = Vec::new();
    while parser.pos < text.len() {
        if parser.take(b'\n') {
            continue;
        }
        let name = parser.atom()?;
        if parser.peek() != Some(b'(') {
            parser.require(b':')?;
        }
        let parameters = parser.schema(0)?;
        let description = if text[parser.pos..].starts_with(" - ") {
            parser.pos += 3;
            Some(
                parser
                    .json()?
                    .as_str()
                    .ok_or_else(|| {
                        CompactError::MalformedOutput("description must be a string".into())
                    })?
                    .to_string(),
            )
        } else {
            None
        };
        if parser.pos < text.len() {
            parser.require(b'\n')?;
        }
        tools.push(ToolDef {
            name,
            description,
            parameters,
        });
        if tools.len() > crate::MAX_TOOLS {
            return Err(CompactError::LimitExceeded);
        }
    }
    check_tools(&tools)?;
    Ok(tools)
}

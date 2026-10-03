use super::render::{CALL_SUFFIX, bare_description};
use super::{
    CanonicalTool, MAX_DEPTH, MAX_PROPERTIES, MAX_TOOLS, Property, SchemaKind, SchemaNode,
    analyze_tools,
};
use crate::{CompactError, FunctionDef, Result, ToolDef};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn parse_tools(text: &str) -> Result<Vec<ToolDef>> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(CompactError::LimitExceeded("compact document bytes"));
    }
    let body = if text == "TOOLS" {
        ""
    } else {
        text.strip_prefix("TOOLS\n")
            .ok_or(CompactError::InvalidGrammar)?
    };
    // Accept the previous framed representation as well as the lean renderer.
    let body = body
        .strip_suffix(CALL_SUFFIX)
        .and_then(|body| {
            body.strip_suffix('\n')
                .or_else(|| body.is_empty().then_some(body))
        })
        .unwrap_or(body);
    let mut tools = Vec::new();
    for line in body.lines() {
        if tools.len() >= MAX_TOOLS {
            return Err(CompactError::LimitExceeded("tool count"));
        }
        let tool = Cursor { remaining: line }.tool()?;
        tools.push(ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: tool.name,
                description: tool.description,
                parameters: tool.parameters.as_ref().map(to_schema),
                extra: Map::new(),
            },
            extra: Map::new(),
        });
    }
    analyze_tools(&tools)?;
    Ok(tools)
}

struct Cursor<'a> {
    remaining: &'a str,
}
impl Cursor<'_> {
    fn tool(&mut self) -> Result<CanonicalTool> {
        let name = self.token(&['('])?.to_owned();
        self.expect("(")?;
        let open_empty = self.take("...");
        let properties = if open_empty {
            BTreeMap::new()
        } else {
            self.properties(')', 0)?
        };
        self.expect(")")?;
        let closed = self.take("!");
        let description = self.description()?;
        let parameters = if !properties.is_empty() || open_empty || closed || description.is_some()
        {
            Some(SchemaNode {
                kind: SchemaKind::Object {
                    properties,
                    additional_properties: !closed,
                },
                description,
                format: None,
                enum_values: None,
            })
        } else {
            None
        };
        let description = if self.take(" - ") {
            Some(self.description_text(&[])?)
        } else {
            None
        };
        if !self.remaining.is_empty() {
            return Err(CompactError::InvalidGrammar);
        }
        Ok(CanonicalTool {
            name,
            description,
            parameters,
        })
    }

    fn properties(&mut self, close: char, depth: usize) -> Result<BTreeMap<String, Property>> {
        let mut properties = BTreeMap::new();
        if self.remaining.starts_with(close) {
            return Ok(properties);
        }
        loop {
            if properties.len() >= MAX_PROPERTIES {
                return Err(CompactError::LimitExceeded("property count"));
            }
            let name = self.token(&['?', ':'])?.to_owned();
            let required = !self.take("?");
            self.expect(":")?;
            let schema = self.node(depth + 1)?;
            if properties
                .insert(name, Property { required, schema })
                .is_some()
            {
                return Err(CompactError::InvalidGrammar);
            }
            if !self.take(",") {
                break;
            }
        }
        Ok(properties)
    }

    fn node(&mut self, depth: usize) -> Result<SchemaNode> {
        if depth >= MAX_DEPTH {
            return Err(CompactError::LimitExceeded("compact schema depth"));
        }
        let (kind, format) = if self.take("[") {
            let child = self.node(depth + 1)?;
            self.expect("]")?;
            (SchemaKind::Array(Box::new(child)), None)
        } else if self.take("{") {
            let properties = self.properties('}', depth)?;
            self.expect("}")?;
            (
                SchemaKind::Object {
                    properties,
                    additional_properties: !self.take("!"),
                },
                None,
            )
        } else if self.take("datetime") {
            (SchemaKind::String, Some("date-time".into()))
        } else if self.take("str") {
            let format = if self.take("<") {
                let format = self.string()?;
                self.expect(">")?;
                Some(format)
            } else {
                None
            };
            (SchemaKind::String, format)
        } else if self.take("int") {
            (SchemaKind::Integer, None)
        } else if self.take("num") {
            (SchemaKind::Number, None)
        } else if self.take("bool") {
            (SchemaKind::Boolean, None)
        } else {
            return Err(CompactError::InvalidGrammar);
        };
        let enum_values = if self.take("=") {
            Some(self.enum_values(&kind)?)
        } else {
            None
        };
        Ok(SchemaNode {
            kind,
            format,
            enum_values,
            description: self.description()?,
        })
    }

    fn enum_values(&mut self, kind: &SchemaKind) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        loop {
            let value = if self.remaining.starts_with('"') {
                Value::String(self.string()?)
            } else {
                let text = self.token(&['|', '#', '(', ',', ']', '}', ')'])?;
                if matches!(kind, SchemaKind::String) {
                    Value::String(text.into())
                } else {
                    serde_json::from_str(text).map_err(|_| CompactError::InvalidGrammar)?
                }
            };
            values.push(value);
            if !self.take("|") {
                break;
            }
        }
        Ok(values)
    }

    fn description(&mut self) -> Result<Option<String>> {
        if self.take("#") {
            Ok(Some(self.string()?))
        } else if self.take("(") {
            let text = self.description_text(&[')'])?;
            self.expect(")")?;
            Ok(Some(text))
        } else {
            Ok(None)
        }
    }

    fn description_text(&mut self, delimiters: &[char]) -> Result<String> {
        if self.remaining.starts_with('"') {
            return self.string();
        }
        let text = self.token(delimiters)?;
        if !bare_description(text) {
            return Err(CompactError::InvalidGrammar);
        }
        Ok(text.into())
    }

    fn string(&mut self) -> Result<String> {
        if !self.remaining.starts_with('"') {
            return Err(CompactError::InvalidGrammar);
        }
        let mut escaped = false;
        for (index, character) in self.remaining.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                let end = index + 1;
                let text = self
                    .remaining
                    .get(..end)
                    .ok_or(CompactError::InvalidGrammar)?;
                let value = serde_json::from_str(text).map_err(|_| CompactError::InvalidGrammar)?;
                self.remaining = self
                    .remaining
                    .get(end..)
                    .ok_or(CompactError::InvalidGrammar)?;
                return Ok(value);
            }
        }
        Err(CompactError::InvalidGrammar)
    }

    fn token(&mut self, delimiters: &[char]) -> Result<&str> {
        let end = self
            .remaining
            .find(|character: char| delimiters.contains(&character))
            .unwrap_or(self.remaining.len());
        let (token, remaining) = self.remaining.split_at(end);
        self.remaining = remaining;
        if token.is_empty() {
            return Err(CompactError::InvalidGrammar);
        }
        Ok(token)
    }

    fn take(&mut self, token: &str) -> bool {
        if let Some(remaining) = self.remaining.strip_prefix(token) {
            self.remaining = remaining;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, token: &str) -> Result<()> {
        if self.take(token) {
            Ok(())
        } else {
            Err(CompactError::InvalidGrammar)
        }
    }
}

fn to_schema(node: &SchemaNode) -> Value {
    let mut schema = match &node.kind {
        SchemaKind::String => json!({"type":"string"}),
        SchemaKind::Integer => json!({"type":"integer"}),
        SchemaKind::Number => json!({"type":"number"}),
        SchemaKind::Boolean => json!({"type":"boolean"}),
        SchemaKind::Array(items) => json!({"type":"array","items":to_schema(items)}),
        SchemaKind::Object {
            properties,
            additional_properties,
        } => {
            let fields: Map<String, Value> = properties
                .iter()
                .map(|(name, property)| (name.clone(), to_schema(&property.schema)))
                .collect();
            let required: Vec<&str> = properties
                .iter()
                .filter(|(_, property)| property.required)
                .map(|(name, _)| name.as_str())
                .collect();
            json!({"type":"object","properties":fields,"required":required,"additionalProperties":additional_properties})
        }
    };
    if let Some(value) = &node.description {
        schema["description"] = json!(value);
    }
    if let Some(value) = &node.format {
        schema["format"] = json!(value);
    }
    if let Some(values) = &node.enum_values {
        schema["enum"] = json!(values);
    }
    schema
}

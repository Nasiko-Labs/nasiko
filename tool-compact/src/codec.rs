use serde_json::{Map, Value};

use crate::schema::{Catalog, MAX_SCHEMA_BYTES, MAX_SCHEMA_DEPTH, is_name};
use crate::{CompactError, CompactTools, Result, ToolDef, strict_json};

/// Encode supported definitions without dropping descriptions or constraints.
/// Unsupported features return an error so the caller can keep its native request.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    Catalog::new(tools)?;
    let mut definitions = String::new();
    for tool in tools {
        if !definitions.is_empty() {
            definitions.push('\n');
        }
        definitions.push_str(&tool.name);
        definitions.push('(');
        match &tool.parameters {
            Some(schema) => encode_schema(schema, &mut definitions)?,
            None => definitions.push('-'),
        }
        definitions.push(')');
        if let Some(description) = &tool.description {
            definitions.push('@');
            definitions.push_str(&json_string(description)?);
        }
    }
    Ok(CompactTools { definitions })
}

/// Reconstruct definitions from the compact text itself, then verify the supported subset.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    if compact.definitions.len() > MAX_SCHEMA_BYTES {
        return Err(CompactError::LimitExceeded);
    }
    let mut parser = Parser {
        remaining: &compact.definitions,
    };
    let mut tools = Vec::new();
    while !parser.remaining.is_empty() {
        let name = parser.name()?;
        parser.expect("(")?;
        let parameters = if parser.take("-") {
            None
        } else {
            Some(parser.schema(0)?)
        };
        parser.expect(")")?;
        let description = if parser.take("@") {
            Some(parser.string()?)
        } else {
            None
        };
        tools.push(ToolDef {
            name,
            parameters,
            description,
        });
        if !parser.remaining.is_empty() {
            parser.expect("\n")?;
        }
    }
    Catalog::new(&tools)?;
    Ok(tools)
}

fn encode_schema(schema: &Value, output: &mut String) -> Result<()> {
    if let Some(boolean) = schema.as_bool() {
        output.push_str(if boolean { "true" } else { "false" });
        return Ok(());
    }
    let mut metadata = schema
        .as_object()
        .cloned()
        .ok_or(CompactError::MalformedOutput)?;
    let description = metadata.remove("description");
    let enumeration = metadata.remove("enum");
    encode_base(&mut metadata, output)?;
    if let Some(enumeration) = enumeration {
        output.push('=');
        output.push_str(&enumeration.to_string());
    }
    if let Some(description) = description {
        output.push('@');
        output.push_str(&description.to_string());
    }
    if !metadata.is_empty() {
        output.push('~');
        output.push_str(&Value::Object(metadata).to_string());
    }
    Ok(())
}

fn encode_base(metadata: &mut Map<String, Value>, output: &mut String) -> Result<()> {
    let kind = metadata
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if kind.as_deref() == Some("object") && can_inline_properties(metadata) {
        return encode_properties(metadata, output);
    }
    if kind.as_deref() == Some("array") && metadata.contains_key("items") {
        metadata.remove("type");
        let items = metadata
            .remove("items")
            .ok_or(CompactError::MalformedOutput)?;
        output.push('[');
        encode_schema(&items, output)?;
        output.push(']');
        return Ok(());
    }
    if let Some(kind) = kind {
        metadata.remove("type");
        output.push_str(&kind);
    } else {
        // Type unions remain verbatim constraints; absent type is genuinely unconstrained.
        output.push_str("any");
    }
    Ok(())
}

fn can_inline_properties(metadata: &Map<String, Value>) -> bool {
    let Some(properties) = metadata.get("properties").and_then(Value::as_object) else {
        return false;
    };
    metadata
        .get("required")
        .and_then(Value::as_array)
        .is_none_or(|required| {
            required.iter().all(|name| {
                name.as_str()
                    .is_some_and(|name| properties.contains_key(name))
            })
        })
}

fn encode_properties(metadata: &mut Map<String, Value>, output: &mut String) -> Result<()> {
    metadata.remove("type");
    let properties = metadata
        .remove("properties")
        .ok_or(CompactError::MalformedOutput)?;
    let properties = properties
        .as_object()
        .ok_or(CompactError::MalformedOutput)?;
    let required = metadata
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !required.is_empty() {
        metadata.remove("required");
    }
    output.push('{');
    let mut first = true;
    // Required-field order carries the original required array, without a sidecar or duplicate names.
    for name in &required {
        let name = name.as_str().ok_or(CompactError::MalformedOutput)?;
        let schema = properties.get(name).ok_or(CompactError::MalformedOutput)?;
        if !first {
            output.push(',');
        }
        first = false;
        output.push_str(&field_name(name)?);
        output.push(':');
        encode_schema(schema, output)?;
    }
    // Sort explicitly: deterministic even when a host enables serde_json/preserve_order.
    let mut optional: Vec<_> = properties
        .iter()
        .filter(|(name, _)| !required.iter().any(|v| v.as_str() == Some(name)))
        .collect();
    optional.sort_by_key(|(name, _)| *name);
    for (name, schema) in optional {
        if !first {
            output.push(',');
        }
        first = false;
        output.push_str(&field_name(name)?);
        output.push_str("?:");
        encode_schema(schema, output)?;
    }
    output.push('}');
    Ok(())
}

fn field_name(name: &str) -> Result<String> {
    if is_name(name) {
        Ok(name.into())
    } else {
        json_string(name)
    }
}

fn json_string(text: &str) -> Result<String> {
    serde_json::to_string(text).map_err(|_| CompactError::MalformedOutput)
}

struct Parser<'a> {
    remaining: &'a str,
}

impl Parser<'_> {
    fn take(&mut self, token: &str) -> bool {
        if let Some(rest) = self.remaining.strip_prefix(token) {
            self.remaining = rest;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<()> {
        if self.take(token) {
            Ok(())
        } else {
            Err(CompactError::MalformedOutput)
        }
    }

    fn name(&mut self) -> Result<String> {
        if self.remaining.starts_with('"') {
            return self.string();
        }
        let length = self
            .remaining
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
            .count();
        if length == 0 {
            return Err(CompactError::MalformedOutput);
        }
        let name = self
            .remaining
            .get(..length)
            .ok_or(CompactError::MalformedOutput)?
            .to_owned();
        self.remaining = self
            .remaining
            .get(length..)
            .ok_or(CompactError::MalformedOutput)?;
        Ok(name)
    }

    fn json(&mut self) -> Result<Value> {
        let (value, length) =
            strict_json::prefix(self.remaining).map_err(|_| CompactError::MalformedOutput)?;
        self.remaining = self
            .remaining
            .get(length..)
            .ok_or(CompactError::MalformedOutput)?;
        Ok(value)
    }

    fn string(&mut self) -> Result<String> {
        self.json()?
            .as_str()
            .map(str::to_owned)
            .ok_or(CompactError::MalformedOutput)
    }

    fn schema(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_SCHEMA_DEPTH {
            return Err(CompactError::LimitExceeded);
        }
        let mut schema = if self.take("{") {
            self.properties(depth + 1)?
        } else if self.take("[") {
            let items = self.schema(depth + 1)?;
            self.expect("]")?;
            serde_json::json!({"type":"array", "items":items})
        } else {
            match self.name()?.as_str() {
                "true" => return Ok(Value::Bool(true)),
                "false" => return Ok(Value::Bool(false)),
                "any" => serde_json::json!({}),
                kind @ ("string" | "integer" | "number" | "boolean" | "null" | "object"
                | "array") => serde_json::json!({"type":kind}),
                _ => return Err(CompactError::MalformedOutput),
            }
        };
        let object = schema
            .as_object_mut()
            .ok_or(CompactError::MalformedOutput)?;
        if self.take("=") {
            object.insert("enum".into(), self.json()?);
        }
        if self.take("@") {
            object.insert("description".into(), Value::String(self.string()?));
        }
        if self.take("~") {
            let metadata = self.json()?;
            for (key, value) in metadata.as_object().ok_or(CompactError::MalformedOutput)? {
                if object.insert(key.clone(), value.clone()).is_some() {
                    return Err(CompactError::MalformedOutput);
                }
            }
        }
        Ok(schema)
    }

    fn properties(&mut self, depth: usize) -> Result<Value> {
        let mut properties = Map::new();
        let mut required = Vec::new();
        if !self.take("}") {
            loop {
                let name = self.name()?;
                let optional = self.take("?");
                self.expect(":")?;
                let schema = self.schema(depth)?;
                if !optional {
                    required.push(Value::String(name.clone()));
                }
                if properties.insert(name, schema).is_some() {
                    return Err(CompactError::MalformedOutput);
                }
                if self.take("}") {
                    break;
                }
                self.expect(",")?;
            }
        }
        let mut schema = serde_json::json!({"type":"object", "properties":properties});
        if !required.is_empty() {
            schema["required"] = Value::Array(required);
        }
        Ok(schema)
    }
}

use std::collections::HashSet;
use serde_json::{Map, Value};

use crate::error::CompactError;
use crate::limits::Limits;
use crate::schema::{CompactField, CompactSchema, CompactToolDef, Metadata, PrimitiveType, TypeExpr};
use crate::strict_json::StrictJsonParser;
use crate::types::{CompactTools, ToolDef};

struct SchemaParser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
    limits: &'a Limits,
}

impl<'a> SchemaParser<'a> {
    fn new(input: &'a str, limits: &'a Limits) -> Self {
        Self {
            input,
            bytes: input.as_bytes(),
            pos: 0,
            depth: 0,
            limits,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn consume(&mut self, expected: u8) -> Result<(), CompactError> {
        if self.pos < self.bytes.len() && self.bytes[self.pos] == expected {
            self.pos += 1;
            Ok(())
        } else {
            Err(CompactError::InvalidSchema {
                reason: format!("expected '{}'", expected as char),
                pointer: None,
            })
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.input[self.pos..].starts_with(s)
    }

    fn parse_identifier(&mut self) -> Result<String, CompactError> {
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return Err(CompactError::InvalidSchema {
                reason: "expected identifier".to_string(),
                pointer: None,
            });
        }
        let id = &self.input[start..self.pos];
        if id.len() > self.limits.max_tool_name_bytes {
            return Err(CompactError::LimitExceeded {
                limit: "max_tool_name_bytes",
                value: id.len(),
                max: self.limits.max_tool_name_bytes,
            });
        }
        Ok(id.to_string())
    }

    fn parse_json_string(&mut self) -> Result<String, CompactError> {
        let sub = &self.input[self.pos..];
        let mut p = StrictJsonParser::new(sub, self.limits);
        let val = p.parse_value()?;
        self.pos += p.position();
        match val {
            Value::String(s) => Ok(s),
            _ => Err(CompactError::InvalidSchema {
                reason: "expected JSON string".to_string(),
                pointer: None,
            }),
        }
    }

    fn parse_property_name(&mut self) -> Result<String, CompactError> {
        if self.peek() == Some(b'"') {
            self.parse_json_string()
        } else {
            self.parse_identifier()
        }
    }

    fn parse_metadata(&mut self) -> Result<Option<Metadata>, CompactError> {
        if self.peek() != Some(b'@') {
            return Ok(None);
        }
        self.pos += 1; // skip '@'

        let sub = &self.input[self.pos..];
        let mut p = StrictJsonParser::new(sub, self.limits);
        let (val, consumed) = p.parse_object()?;
        self.pos += consumed;

        let obj = val.as_object().ok_or_else(|| CompactError::InvalidSchema {
            reason: "metadata must be a JSON object".to_string(),
            pointer: None,
        })?;

        let mut meta = Metadata::default();
        for (k, v) in obj {
            match k.as_str() {
                "$schema" => meta.schema_uri = v.as_str().map(|s| s.to_string()),
                "additionalProperties" => meta.additional_properties = v.as_bool(),
                "title" => meta.title = v.as_str().map(|s| s.to_string()),
                "minLength" => meta.min_length = v.as_u64(),
                "maxLength" => meta.max_length = v.as_u64(),
                "minItems" => meta.min_items = v.as_u64(),
                "maxItems" => meta.max_items = v.as_u64(),
                "uniqueItems" => meta.unique_items = v.as_bool(),
                "minProperties" => meta.min_properties = v.as_u64(),
                "maxProperties" => meta.max_properties = v.as_u64(),
                "required" => {
                    if let Some(arr) = v.as_array() {
                        if arr.is_empty() {
                            meta.empty_required = true;
                        }
                    }
                }
                _ => {
                    return Err(CompactError::InvalidSchema {
                        reason: format!("unknown metadata keyword '{}'", k),
                        pointer: None,
                    });
                }
            }
        }

        Ok(Some(meta))
    }

    fn parse_schema_description(&mut self) -> Result<Option<String>, CompactError> {
        if self.peek() == Some(b'~') {
            self.pos += 1;
            let s = self.parse_json_string()?;
            Ok(Some(s))
        } else {
            Ok(None)
        }
    }

    fn parse_tool_description(&mut self) -> Result<Option<String>, CompactError> {
        if self.peek() == Some(b'#') {
            self.pos += 1;
            let s = self.parse_json_string()?;
            Ok(Some(s))
        } else {
            Ok(None)
        }
    }

    fn parse_schema(&mut self) -> Result<CompactSchema, CompactError> {
        self.depth += 1;
        if self.depth > self.limits.max_schema_depth {
            return Err(CompactError::LimitExceeded {
                limit: "max_schema_depth",
                value: self.depth,
                max: self.limits.max_schema_depth,
            });
        }

        let type_expr = if self.starts_with("string(date-time)") {
            self.pos += "string(date-time)".len();
            TypeExpr::StringDateTime
        } else if self.starts_with("string") {
            self.pos += "string".len();
            TypeExpr::Primitive(PrimitiveType::String)
        } else if self.starts_with("integer") {
            self.pos += "integer".len();
            TypeExpr::Primitive(PrimitiveType::Integer)
        } else if self.starts_with("number") {
            self.pos += "number".len();
            TypeExpr::Primitive(PrimitiveType::Number)
        } else if self.starts_with("boolean") {
            self.pos += "boolean".len();
            TypeExpr::Primitive(PrimitiveType::Boolean)
        } else if self.starts_with("null") {
            self.pos += "null".len();
            TypeExpr::Primitive(PrimitiveType::Null)
        } else if self.starts_with("enum(") {
            self.pos += "enum(".len();
            let mut variants = Vec::new();
            if self.peek() != Some(b')') {
                loop {
                    let s = self.parse_json_string()?;
                    variants.push(s);
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
            }
            self.consume(b')')?;
            TypeExpr::StringEnum(variants)
        } else if self.starts_with("array<") {
            self.pos += "array<".len();
            let inner = self.parse_schema()?;
            self.consume(b'>')?;
            TypeExpr::Array(Box::new(inner))
        } else if self.starts_with("object{") {
            self.pos += "object{".len();
            let mut fields = Vec::new();
            if self.peek() != Some(b'}') {
                fields = self.parse_fields(b'}')?;
            }
            self.consume(b'}')?;
            TypeExpr::Object(fields)
        } else {
            return Err(CompactError::InvalidSchema {
                reason: "unknown type expression".to_string(),
                pointer: None,
            });
        };

        let metadata = self.parse_metadata()?;
        let description = self.parse_schema_description()?;

        self.depth -= 1;
        Ok(CompactSchema {
            type_expr,
            metadata,
            description,
        })
    }

    fn parse_fields(&mut self, term: u8) -> Result<Vec<CompactField>, CompactError> {
        let mut fields = Vec::new();
        let mut seen_names = HashSet::new();

        while self.pos < self.bytes.len() && self.peek() != Some(term) {
            let name = self.parse_property_name()?;
            if !seen_names.insert(name.clone()) {
                return Err(CompactError::InvalidSchema {
                    reason: format!("duplicate field name '{}'", name),
                    pointer: None,
                });
            }

            let optional = if self.peek() == Some(b'?') {
                self.pos += 1;
                true
            } else {
                false
            };

            self.consume(b':')?;
            let schema = self.parse_schema()?;

            fields.push(CompactField {
                name,
                optional,
                schema,
            });

            if self.peek() == Some(b',') {
                self.pos += 1;
            } else {
                break;
            }
        }

        Ok(fields)
    }

    fn parse_tool_definition(&mut self) -> Result<CompactToolDef, CompactError> {
        let name = self.parse_identifier()?;
        self.consume(b'(')?;
        let mut fields = Vec::new();
        if self.peek() != Some(b')') {
            fields = self.parse_fields(b')')?;
        }
        self.consume(b')')?;

        let metadata = self.parse_metadata()?;
        let schema_description = self.parse_schema_description()?;
        let tool_description = self.parse_tool_description()?;

        Ok(CompactToolDef {
            name,
            fields,
            metadata,
            schema_description,
            tool_description,
        })
    }
}

pub fn reconstruct_schema(schema: &CompactSchema) -> Value {
    let mut map = Map::new();

    match &schema.type_expr {
        TypeExpr::Primitive(p) => {
            map.insert("type".to_string(), Value::String(p.as_str().to_string()));
        }
        TypeExpr::StringDateTime => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            map.insert("format".to_string(), Value::String("date-time".to_string()));
        }
        TypeExpr::StringEnum(variants) => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            let arr = variants.iter().map(|v| Value::String(v.clone())).collect();
            map.insert("enum".to_string(), Value::Array(arr));
        }
        TypeExpr::Array(item) => {
            map.insert("type".to_string(), Value::String("array".to_string()));
            map.insert("items".to_string(), reconstruct_schema(item));
        }
        TypeExpr::Object(fields) => {
            map.insert("type".to_string(), Value::String("object".to_string()));
            let mut props = Map::new();
            let mut req = Vec::new();

            for f in fields {
                props.insert(f.name.clone(), reconstruct_schema(&f.schema));
                if !f.optional {
                    req.push(Value::String(f.name.clone()));
                }
            }
            map.insert("properties".to_string(), Value::Object(props));

            if !req.is_empty() {
                map.insert("required".to_string(), Value::Array(req));
            } else if let Some(ref m) = schema.metadata {
                if m.empty_required {
                    map.insert("required".to_string(), Value::Array(Vec::new()));
                }
            }
        }
    }

    if let Some(ref desc) = schema.description {
        map.insert("description".to_string(), Value::String(desc.clone()));
    }

    if let Some(ref meta) = schema.metadata {
        if let Some(ref uri) = meta.schema_uri {
            map.insert("$schema".to_string(), Value::String(uri.clone()));
        }
        if let Some(add) = meta.additional_properties {
            map.insert("additionalProperties".to_string(), Value::Bool(add));
        }
        if let Some(ref t) = meta.title {
            map.insert("title".to_string(), Value::String(t.clone()));
        }
        if let Some(min) = meta.min_length {
            map.insert("minLength".to_string(), Value::from(min));
        }
        if let Some(max) = meta.max_length {
            map.insert("maxLength".to_string(), Value::from(max));
        }
        if let Some(min) = meta.min_items {
            map.insert("minItems".to_string(), Value::from(min));
        }
        if let Some(max) = meta.max_items {
            map.insert("maxItems".to_string(), Value::from(max));
        }
        if let Some(u) = meta.unique_items {
            map.insert("uniqueItems".to_string(), Value::Bool(u));
        }
        if let Some(min) = meta.min_properties {
            map.insert("minProperties".to_string(), Value::from(min));
        }
        if let Some(max) = meta.max_properties {
            map.insert("maxProperties".to_string(), Value::from(max));
        }
    }

    Value::Object(map)
}

pub fn reconstruct_root_schema(tool: &CompactToolDef) -> Value {
    let dummy_schema = CompactSchema {
        type_expr: TypeExpr::Object(tool.fields.clone()),
        metadata: tool.metadata.clone(),
        description: tool.schema_description.clone(),
    };
    reconstruct_schema(&dummy_schema)
}

pub fn decode_tools_with_limits(compact: &CompactTools, limits: &Limits) -> Result<Vec<ToolDef>, CompactError> {
    if compact.text.len() > limits.max_schema_bytes {
        return Err(CompactError::LimitExceeded {
            limit: "max_schema_bytes",
            value: compact.text.len(),
            max: limits.max_schema_bytes,
        });
    }

    let mut lines = compact.text.lines();
    let first = lines.next().ok_or_else(|| CompactError::InvalidSchema {
        reason: "empty compact tools document".to_string(),
        pointer: None,
    })?;

    if first != crate::encode::LEGEND {
        return Err(CompactError::InvalidSchema {
            reason: "document must begin with fixed CTP/1 instructions legend".to_string(),
            pointer: None,
        });
    }

    let mut tools = Vec::new();
    let mut seen_names = HashSet::new();

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }

        let mut parser = SchemaParser::new(line, limits);
        let tool_def = parser.parse_tool_definition()?;
        if parser.pos != parser.bytes.len() {
            return Err(CompactError::InvalidSchema {
                reason: format!("unexpected trailing content in tool line: '{}'", &line[parser.pos..]),
                pointer: None,
            });
        }

        if !seen_names.insert(tool_def.name.clone()) {
            return Err(CompactError::DuplicateToolName {
                name: tool_def.name.clone(),
            });
        }

        let root_parameters = reconstruct_root_schema(&tool_def);
        tools.push(ToolDef {
            name: tool_def.name,
            description: tool_def.tool_description,
            parameters: root_parameters,
        });
    }

    if tools.len() > limits.max_tools {
        return Err(CompactError::LimitExceeded {
            limit: "max_tools",
            value: tools.len(),
            max: limits.max_tools,
        });
    }

    Ok(tools)
}

pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    decode_tools_with_limits(compact, &Limits::default())
}

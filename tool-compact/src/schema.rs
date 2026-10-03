//! Canonical schema representation and conversions for JSON Schema and compact representations.

use crate::error::Error;
use crate::types::{FunctionDef, ToolDef};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Supported string formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StrFormat {
    DateTime,
    Date,
    Time,
    Email,
    Uri,
    Uuid,
}

impl StrFormat {
    pub fn as_json_format_str(&self) -> &'static str {
        match self {
            StrFormat::DateTime => "date-time",
            StrFormat::Date => "date",
            StrFormat::Time => "time",
            StrFormat::Email => "email",
            StrFormat::Uri => "uri",
            StrFormat::Uuid => "uuid",
        }
    }

    pub fn compact_name(&self) -> &'static str {
        match self {
            StrFormat::DateTime => "datetime",
            StrFormat::Date => "date",
            StrFormat::Time => "time",
            StrFormat::Email => "email",
            StrFormat::Uri => "uri",
            StrFormat::Uuid => "uuid",
        }
    }

    pub fn from_json_format_str(s: &str) -> Option<Self> {
        match s {
            "date-time" => Some(StrFormat::DateTime),
            "date" => Some(StrFormat::Date),
            "time" => Some(StrFormat::Time),
            "email" => Some(StrFormat::Email),
            "uri" => Some(StrFormat::Uri),
            "uuid" => Some(StrFormat::Uuid),
            _ => None,
        }
    }

    pub fn from_compact_name(s: &str) -> Option<Self> {
        match s {
            "datetime" => Some(StrFormat::DateTime),
            "date" => Some(StrFormat::Date),
            "time" => Some(StrFormat::Time),
            "email" => Some(StrFormat::Email),
            "uri" => Some(StrFormat::Uri),
            "uuid" => Some(StrFormat::Uuid),
            _ => None,
        }
    }
}

/// Canonical type representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Num,
    Bool,
    Null,
    Format(StrFormat),
    Enum(Vec<String>),
    Array(Box<Ty>),
    Object(Vec<Field>),
}

impl Ty {
    /// Convert canonical type back into a JSON Schema Value.
    pub fn to_json_schema(&self) -> Value {
        match self {
            Ty::Str => json!({"type": "string"}),
            Ty::Int => json!({"type": "integer"}),
            Ty::Num => json!({"type": "number"}),
            Ty::Bool => json!({"type": "boolean"}),
            Ty::Null => json!({"type": "null"}),
            Ty::Format(fmt) => json!({
                "type": "string",
                "format": fmt.as_json_format_str()
            }),
            Ty::Enum(variants) => json!({
                "type": "string",
                "enum": variants
            }),
            Ty::Array(item_ty) => json!({
                "type": "array",
                "items": item_ty.to_json_schema()
            }),
            Ty::Object(fields) => {
                let mut props = Map::new();
                let mut required = Vec::new();
                for f in fields {
                    let mut f_schema = f.ty.to_json_schema();
                    if let Some(desc) = &f.description
                        && let Value::Object(ref mut map) = f_schema
                    {
                        map.insert("description".to_string(), Value::String(desc.clone()));
                    }
                    props.insert(f.name.clone(), f_schema);
                    if f.required {
                        required.push(Value::String(f.name.clone()));
                    }
                }
                let mut obj = json!({
                    "type": "object",
                    "properties": props,
                    "additionalProperties": false
                });
                if !required.is_empty() {
                    obj["required"] = Value::Array(required);
                }
                obj
            }
        }
    }

    /// Render canonical type to its compact string representation.
    pub fn encode_compact(
        &self,
        tool_name: &str,
        field_name: &str,
        description: Option<&str>,
    ) -> String {
        let ty_str = match self {
            Ty::Str => "str".to_string(),
            Ty::Int => "int".to_string(),
            Ty::Num => "num".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Null => "null".to_string(),
            Ty::Format(fmt) => fmt.compact_name().to_string(),
            Ty::Enum(variants) => variants.join("|"),
            Ty::Array(item_ty) => {
                format!("[{}]", item_ty.encode_compact(tool_name, field_name, None))
            }
            Ty::Object(fields) => {
                let inner = fields
                    .iter()
                    .map(|f| f.encode_compact(tool_name))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{inner}}}")
            }
        };

        if let Some(desc) = description
            && !is_redundant_field_description(tool_name, field_name, desc)
        {
            let collapsed = collapse_whitespace(desc);
            if !collapsed.is_empty() {
                let escaped = serde_json::to_string(&collapsed)
                    .unwrap_or_else(|_| format!("\"{collapsed}\""));
                return format!("{ty_str} {escaped}");
            }
        }
        ty_str
    }
}

/// A parameter or object property field in canonical schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub required: bool,
    pub ty: Ty,
    pub description: Option<String>,
}

impl Field {
    /// Render field into compact prompt notation: `name:type "desc"` or `name?:type`.
    pub fn encode_compact(&self, tool_name: &str) -> String {
        let opt_marker = if self.required { "" } else { "?" };
        let ty_encoded = self
            .ty
            .encode_compact(tool_name, &self.name, self.description.as_deref());
        format!("{}{opt_marker}:{ty_encoded}", self.name)
    }
}

/// Canonical tool schema: preserves tool name, description, and parameter fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSchema {
    pub name: String,
    pub description: Option<String>,
    pub params: Vec<Field>,
}

impl ToolSchema {
    /// Parse an OpenAI-style ToolDef into canonical schema.
    ///
    /// Fails closed with `Error::UnsupportedSchema` if any unsupported JSON Schema
    /// keywords or malformed types are encountered.
    pub fn from_tool_def(tool: &ToolDef) -> Result<Self, Error> {
        let name = &tool.function.name;
        if !is_valid_ident(name) {
            return Err(Error::UnsupportedSchema {
                tool: name.clone(),
                path: "name".to_string(),
                reason: "tool name contains invalid characters".to_string(),
            });
        }

        let params = match &tool.function.parameters {
            None => Vec::new(),
            Some(Value::Object(param_map)) => parse_parameters_object(name, "", param_map)?,
            Some(_) => {
                return Err(Error::UnsupportedSchema {
                    tool: name.clone(),
                    path: "parameters".to_string(),
                    reason: "parameters must be a JSON object".to_string(),
                });
            }
        };

        Ok(ToolSchema {
            name: name.clone(),
            description: tool.function.description.clone(),
            params,
        })
    }

    /// Convert canonical schema back to an OpenAI-style ToolDef.
    pub fn to_tool_def(&self) -> ToolDef {
        let parameters_val = if self.params.is_empty() {
            Some(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }))
        } else {
            let mut props = Map::new();
            let mut required = Vec::new();
            for f in &self.params {
                let mut f_schema = f.ty.to_json_schema();
                if let Some(desc) = &f.description
                    && let Value::Object(ref mut map) = f_schema
                {
                    map.insert("description".to_string(), Value::String(desc.clone()));
                }
                props.insert(f.name.clone(), f_schema);
                if f.required {
                    required.push(Value::String(f.name.clone()));
                }
            }
            let mut obj = json!({
                "type": "object",
                "properties": props,
                "additionalProperties": false
            });
            if !required.is_empty() {
                obj["required"] = Value::Array(required);
            }
            Some(obj)
        };

        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: self.name.clone(),
                description: self.description.clone(),
                parameters: parameters_val,
            },
        }
    }

    /// Render canonical tool schema to a single compact definition line.
    pub fn encode_compact(&self) -> String {
        let fields_str = self
            .params
            .iter()
            .map(|f| f.encode_compact(&self.name))
            .collect::<Vec<_>>()
            .join(", ");

        let mut line = format!("{}({fields_str})", self.name);
        if let Some(desc) = &self.description {
            let collapsed = collapse_whitespace(desc);
            if !collapsed.is_empty() {
                line.push_str(" - ");
                line.push_str(&collapsed);
            }
        }
        line
    }

    /// Check semantic equality between two ToolDefs.
    pub fn semantically_equal(a: &ToolDef, b: &ToolDef) -> bool {
        let s_a = match ToolSchema::from_tool_def(a) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let s_b = match ToolSchema::from_tool_def(b) {
            Ok(s) => s,
            Err(_) => return false,
        };

        if s_a.name != s_b.name {
            return false;
        }

        let desc_a = s_a.description.as_deref().map(collapse_whitespace);
        let desc_b = s_b.description.as_deref().map(collapse_whitespace);
        if desc_a != desc_b {
            return false;
        }

        if s_a.params.len() != s_b.params.len() {
            return false;
        }

        // Compare fields order-independently by field name
        let mut map_b: BTreeMap<&str, &Field> = BTreeMap::new();
        for f in &s_b.params {
            map_b.insert(&f.name, f);
        }

        for f_a in &s_a.params {
            match map_b.get(f_a.name.as_str()) {
                Some(f_b) => {
                    if f_a.required != f_b.required {
                        return false;
                    }
                    if !types_semantically_equal(&f_a.ty, &f_b.ty) {
                        return false;
                    }
                    // For descriptions: if both present, normalized text must match
                    if let (Some(da), Some(db)) = (&f_a.description, &f_b.description)
                        && collapse_whitespace(da) != collapse_whitespace(db)
                    {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }
}

fn types_semantically_equal(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Str, Ty::Str)
        | (Ty::Int, Ty::Int)
        | (Ty::Num, Ty::Num)
        | (Ty::Bool, Ty::Bool)
        | (Ty::Null, Ty::Null) => true,
        (Ty::Format(f1), Ty::Format(f2)) => f1 == f2,
        (Ty::Enum(v1), Ty::Enum(v2)) => v1 == v2,
        (Ty::Array(i1), Ty::Array(i2)) => types_semantically_equal(i1, i2),
        (Ty::Object(fields1), Ty::Object(fields2)) => {
            if fields1.len() != fields2.len() {
                return false;
            }
            let mut map2: BTreeMap<&str, &Field> = BTreeMap::new();
            for f in fields2 {
                map2.insert(&f.name, f);
            }
            for f1 in fields1 {
                match map2.get(f1.name.as_str()) {
                    Some(f2) => {
                        if f1.required != f2.required || !types_semantically_equal(&f1.ty, &f2.ty) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        _ => false,
    }
}

fn parse_parameters_object(
    tool_name: &str,
    path_prefix: &str,
    param_map: &Map<String, Value>,
) -> Result<Vec<Field>, Error> {
    // 1. Check for unsupported root schema keywords
    for unsupported in [
        "anyOf",
        "oneOf",
        "allOf",
        "$ref",
        "$defs",
        "not",
        "patternProperties",
        "if",
        "then",
        "else",
    ] {
        if param_map.contains_key(unsupported) {
            let path = if path_prefix.is_empty() {
                unsupported.to_string()
            } else {
                format!("{path_prefix}.{unsupported}")
            };
            return Err(Error::UnsupportedSchema {
                tool: tool_name.to_string(),
                path,
                reason: format!("unsupported schema keyword '{unsupported}'"),
            });
        }
    }

    // 2. Reject additionalProperties: true or schema
    if let Some(ap) = param_map.get("additionalProperties")
        && (ap.as_bool() == Some(true) || ap.is_object())
    {
        let path = if path_prefix.is_empty() {
            "additionalProperties".to_string()
        } else {
            format!("{path_prefix}.additionalProperties")
        };
        return Err(Error::UnsupportedSchema {
            tool: tool_name.to_string(),
            path,
            reason: "additionalProperties must be false or omitted".to_string(),
        });
    }

    let required_fields: Vec<&str> = param_map
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let properties = match param_map.get("properties") {
        None => return Ok(Vec::new()),
        Some(Value::Object(map)) => map,
        Some(_) => {
            let path = if path_prefix.is_empty() {
                "properties".to_string()
            } else {
                format!("{path_prefix}.properties")
            };
            return Err(Error::UnsupportedSchema {
                tool: tool_name.to_string(),
                path,
                reason: "properties must be an object".to_string(),
            });
        }
    };

    let mut fields = Vec::new();
    for (fname, fschema) in properties {
        if !is_valid_ident(fname) {
            let path = if path_prefix.is_empty() {
                format!("properties.{fname}")
            } else {
                format!("{path_prefix}.properties.{fname}")
            };
            return Err(Error::UnsupportedSchema {
                tool: tool_name.to_string(),
                path,
                reason: "field name contains invalid characters".to_string(),
            });
        }

        let fpath = if path_prefix.is_empty() {
            format!("properties.{fname}")
        } else {
            format!("{path_prefix}.properties.{fname}")
        };

        let fobj = fschema
            .as_object()
            .ok_or_else(|| Error::UnsupportedSchema {
                tool: tool_name.to_string(),
                path: fpath.clone(),
                reason: "field schema must be an object".to_string(),
            })?;

        // Check unsupported field keywords
        for unsupported in [
            "anyOf",
            "oneOf",
            "allOf",
            "$ref",
            "$defs",
            "not",
            "pattern",
            "default",
            "minimum",
            "maximum",
            "minLength",
            "maxLength",
            "minItems",
            "maxItems",
            "uniqueItems",
            "patternProperties",
        ] {
            if fobj.contains_key(unsupported) {
                return Err(Error::UnsupportedSchema {
                    tool: tool_name.to_string(),
                    path: format!("{fpath}.{unsupported}"),
                    reason: format!("unsupported field keyword '{unsupported}'"),
                });
            }
        }

        let is_req = required_fields.contains(&fname.as_str());
        let ty = parse_field_type(tool_name, &fpath, fobj)?;
        let description = fobj
            .get("description")
            .and_then(Value::as_str)
            .map(|s| s.to_string());

        fields.push(Field {
            name: fname.clone(),
            required: is_req,
            ty,
            description,
        });
    }

    Ok(fields)
}

fn parse_field_type(tool_name: &str, path: &str, fobj: &Map<String, Value>) -> Result<Ty, Error> {
    // 1. Check enum
    if let Some(enum_val) = fobj.get("enum") {
        let arr = enum_val
            .as_array()
            .ok_or_else(|| Error::UnsupportedSchema {
                tool: tool_name.to_string(),
                path: format!("{path}.enum"),
                reason: "enum must be an array".to_string(),
            })?;

        let mut variants = Vec::new();
        for item in arr {
            let s = item.as_str().ok_or_else(|| Error::UnsupportedSchema {
                tool: tool_name.to_string(),
                path: format!("{path}.enum"),
                reason: "enum members must be strings in v1".to_string(),
            })?;
            variants.push(s.to_string());
        }
        return Ok(Ty::Enum(variants));
    }

    // 2. Type keyword
    let type_str =
        fobj.get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::UnsupportedSchema {
                tool: tool_name.to_string(),
                path: format!("{path}.type"),
                reason: "field missing 'type' keyword".to_string(),
            })?;

    match type_str {
        "string" => {
            if let Some(fmt_str) = fobj.get("format").and_then(Value::as_str) {
                let fmt = StrFormat::from_json_format_str(fmt_str).ok_or_else(|| {
                    Error::UnsupportedSchema {
                        tool: tool_name.to_string(),
                        path: format!("{path}.format"),
                        reason: format!("unsupported string format '{fmt_str}'"),
                    }
                })?;
                Ok(Ty::Format(fmt))
            } else {
                Ok(Ty::Str)
            }
        }
        "integer" => Ok(Ty::Int),
        "number" => Ok(Ty::Num),
        "boolean" => Ok(Ty::Bool),
        "null" => Ok(Ty::Null),
        "array" => {
            let items = fobj
                .get("items")
                .and_then(Value::as_object)
                .ok_or_else(|| Error::UnsupportedSchema {
                    tool: tool_name.to_string(),
                    path: format!("{path}.items"),
                    reason: "array must have an 'items' schema object".to_string(),
                })?;
            let item_ty = parse_field_type(tool_name, &format!("{path}.items"), items)?;
            Ok(Ty::Array(Box::new(item_ty)))
        }
        "object" => {
            let nested_fields = parse_parameters_object(tool_name, path, fobj)?;
            Ok(Ty::Object(nested_fields))
        }
        other => Err(Error::UnsupportedSchema {
            tool: tool_name.to_string(),
            path: format!("{path}.type"),
            reason: format!("unsupported type '{other}'"),
        }),
    }
}

pub fn is_valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn is_redundant_field_description(tool_name: &str, field_name: &str, desc: &str) -> bool {
    let stop_words: [&str; 18] = [
        "the", "a", "an", "of", "to", "for", "in", "is", "its", "string", "integer", "number",
        "boolean", "array", "list", "value", "field", "event",
    ];
    let mut allowed: Vec<String> = stop_words.iter().map(|s| s.to_string()).collect();
    for part in tool_name.split('_') {
        let lower = part.to_ascii_lowercase();
        if !lower.is_empty() {
            allowed.push(lower);
        }
    }
    for part in field_name.split('_') {
        let lower = part.to_ascii_lowercase();
        if !lower.is_empty() {
            allowed.push(lower);
        }
    }

    let cleaned = desc.trim_end_matches('.');
    for word in cleaned.split_whitespace() {
        let word_clean: String = word
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if word_clean.is_empty() {
            continue;
        }
        if !allowed.iter().any(|a| a == &word_clean) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_schema_round_trip() {
        let tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "test_tool".to_string(),
                description: Some("Test description".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "User name"},
                        "age": {"type": "integer"},
                        "tags": {"type": "array", "items": {"type": "string"}},
                        "created_at": {"type": "string", "format": "date-time"}
                    },
                    "required": ["name"]
                })),
            },
        };

        let schema = ToolSchema::from_tool_def(&tool).unwrap();
        let tool_back = schema.to_tool_def();
        assert!(ToolSchema::semantically_equal(&tool, &tool_back));
    }

    #[test]
    fn test_unsupported_schema_keywords_rejected() {
        let unsupported_keywords = [
            json!({"type": "object", "anyOf": []}),
            json!({"type": "object", "oneOf": []}),
            json!({"type": "object", "allOf": []}),
            json!({"type": "object", "$ref": "#/defs/Foo"}),
            json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "^[a-z]+$"}}}),
            json!({"type": "object", "properties": {"a": {"type": "integer", "minimum": 0}}}),
            json!({"type": "object", "properties": {"a": {"type": "string", "default": "hello"}}}),
        ];

        for kw in unsupported_keywords {
            let tool = ToolDef {
                kind: "function".to_string(),
                function: FunctionDef {
                    name: "bad_tool".to_string(),
                    description: None,
                    parameters: Some(kw),
                },
            };
            assert!(matches!(
                ToolSchema::from_tool_def(&tool),
                Err(Error::UnsupportedSchema { .. })
            ));
        }
    }
}

use serde_json::{Map, Value};

use crate::{CompactError, CompactTools, Result, ToolDef};

const MAX_DEPTH: usize = 16;
const ALLOWED: &[&str] = &[
    "type",
    "properties",
    "required",
    "items",
    "description",
    "enum",
    "format",
    "additionalProperties",
];

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut lines = Vec::with_capacity(tools.len());
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        if !valid_name(&tool.name) || !names.insert(&tool.name) {
            return Err(CompactError::InvalidDefinition(format!(
                "invalid or duplicate tool name: {}",
                tool.name
            )));
        }
        let mut line = tool.name.clone();
        line.push('(');
        if let Some(parameters) = &tool.parameters {
            check_schema(parameters, 0)?;
            if type_name(parameters)? != "object" {
                return Err(CompactError::UnsupportedSchema(
                    "parameters must be an object".into(),
                ));
            }
            if nullable(parameters) {
                return Err(CompactError::UnsupportedSchema(
                    "nullable parameters object".into(),
                ));
            }
            line.push_str(&render_fields(parameters, 0)?);
        }
        line.push(')');
        if let Some(parameters) = &tool.parameters {
            line.push_str(object_flag(parameters)?);
            if let Some(description) = parameters.get("description").and_then(Value::as_str) {
                line.push('^');
                line.push_str(&quote(&normalize(description)));
            }
        } else {
            line.push('-'); // Absent parameters, distinct from an empty object.
        }
        if let Some(description) = &tool.description {
            line.push('~');
            line.push_str(&quote(&normalize(description)));
        }
        lines.push(line);
    }
    Ok(CompactTools {
        text: lines.join("\n"),
    })
}

pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    if compact.text.is_empty() {
        return Ok(Vec::new());
    }
    compact.text.lines().map(parse_tool).collect()
}

fn parse_tool(line: &str) -> Result<ToolDef> {
    let mut p = Parser::new(line);
    let name = p.ident()?;
    p.expect('(')?;
    let (properties, required) = p.fields(')')?;
    p.expect(')')?;
    let parameters = if p.take('-') {
        if !properties.is_empty() {
            return Err(CompactError::InvalidDefinition(
                "absent parameters have fields".into(),
            ));
        }
        None
    } else {
        let mut schema = object_schema(properties, required, p.flag());
        if p.take('^') {
            schema
                .as_object_mut()
                .unwrap()
                .insert("description".into(), Value::String(p.string()?));
        }
        Some(schema)
    };
    let description = if p.take('~') { Some(p.string()?) } else { None };
    if !p.done() {
        return Err(CompactError::InvalidDefinition(
            "trailing declaration text".into(),
        ));
    }
    let tool = ToolDef {
        name,
        description,
        parameters,
    };
    if !valid_name(&tool.name) {
        return Err(CompactError::InvalidDefinition("invalid tool name".into()));
    }
    if let Some(schema) = &tool.parameters {
        check_schema(schema, 0)?;
    }
    Ok(tool)
}

fn render_fields(schema: &Value, depth: usize) -> Result<String> {
    let properties = schema.get("properties").and_then(Value::as_object);
    let required = schema.get("required").and_then(Value::as_array);
    let mut fields = Vec::new();
    if let Some(properties) = properties {
        let ordered_names = required
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(properties.keys().map(String::as_str).filter(|name| {
                !required.is_some_and(|r| r.iter().any(|x| x.as_str() == Some(*name)))
            }));
        for name in ordered_names {
            let child = &properties[name];
            if !valid_name(name) {
                return Err(CompactError::UnsupportedSchema(format!(
                    "property name: {name}"
                )));
            }
            let is_required = required.is_some_and(|r| r.iter().any(|x| x.as_str() == Some(name)));
            let field = format!(
                "{}{}:{}",
                name,
                if is_required { '!' } else { '?' },
                render_type(child, depth + 1)?
            );
            fields.push(field);
        }
    }
    Ok(fields.join(","))
}

fn render_type(schema: &Value, depth: usize) -> Result<String> {
    if depth > MAX_DEPTH {
        return Err(CompactError::UnsupportedSchema("schema too deep".into()));
    }
    let kind = type_name(schema)?;
    let mut out = match kind {
        "string" => "str".to_string(),
        "integer" => "int".to_string(),
        "number" => "num".to_string(),
        "boolean" => "bool".to_string(),
        "null" => "null".to_string(),
        "array" => format!(
            "[{}]",
            render_type(
                schema
                    .get("items")
                    .ok_or_else(|| CompactError::UnsupportedSchema("array missing items".into()))?,
                depth + 1
            )?
        ),
        "object" => format!(
            "obj{}{{{}}}",
            object_flag(schema)?,
            render_fields(schema, depth + 1)?
        ),
        _ => return Err(CompactError::UnsupportedSchema(format!("type: {kind}"))),
    };
    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        if kind != "string"
            || !format
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(CompactError::UnsupportedSchema(
                "invalid format hint".into(),
            ));
        }
        out.push('@');
        out.push_str(format);
    }
    if let Some(enums) = schema.get("enum").and_then(Value::as_array) {
        let values = enums
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(",");
        out.push('{');
        out.push_str(&values);
        out.push('}');
    }
    if nullable(schema) {
        out.push_str("|null");
    }
    if let Some(description) = schema.get("description").and_then(Value::as_str) {
        out.push('~');
        out.push_str(&quote(&normalize(description)));
    }
    Ok(out)
}

fn object_flag(schema: &Value) -> Result<&'static str> {
    match schema.get("additionalProperties") {
        None => Ok(""),
        Some(Value::Bool(false)) => Ok("!"),
        Some(Value::Bool(true)) => Ok("+"),
        _ => Err(CompactError::UnsupportedSchema(
            "additionalProperties must be boolean".into(),
        )),
    }
}

fn type_name(schema: &Value) -> Result<&str> {
    let value = schema
        .get("type")
        .ok_or_else(|| CompactError::UnsupportedSchema("missing type".into()))?;
    match value {
        Value::String(s) => Ok(s),
        Value::Array(types) if types.len() == 2 && types.iter().any(|v| v == "null") => types
            .iter()
            .find_map(|v| v.as_str().filter(|s| *s != "null"))
            .ok_or_else(|| CompactError::UnsupportedSchema("invalid nullable type".into())),
        _ => Err(CompactError::UnsupportedSchema(
            "unsupported type union".into(),
        )),
    }
}

fn nullable(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_array).is_some()
}

fn check_schema(schema: &Value, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(CompactError::UnsupportedSchema("schema too deep".into()));
    }
    let obj = schema
        .as_object()
        .ok_or_else(|| CompactError::UnsupportedSchema("schema is not an object".into()))?;
    for key in obj.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return Err(CompactError::UnsupportedSchema(format!("keyword: {key}")));
        }
    }
    let kind = type_name(schema)?;
    if ![
        "string", "integer", "number", "boolean", "null", "array", "object",
    ]
    .contains(&kind)
    {
        return Err(CompactError::UnsupportedSchema(format!("type: {kind}")));
    }
    if schema.get("description").is_some_and(|v| !v.is_string()) {
        return Err(CompactError::UnsupportedSchema(
            "description must be a string".into(),
        ));
    }
    if let Some(format) = schema.get("format")
        && (kind != "string" || !format.is_string())
    {
        return Err(CompactError::UnsupportedSchema(
            "format on non-string".into(),
        ));
    }
    if let Some(enums) = schema.get("enum") {
        if kind == "object" || kind == "array" {
            return Err(CompactError::UnsupportedSchema("complex enum".into()));
        }
        let values = enums
            .as_array()
            .ok_or_else(|| CompactError::UnsupportedSchema("enum must be an array".into()))?;
        if values.is_empty()
            || values
                .iter()
                .any(|v| !primitive_matches(kind, v, nullable(schema)))
        {
            return Err(CompactError::UnsupportedSchema("enum type mismatch".into()));
        }
    }
    match kind {
        "object" => {
            if schema.get("items").is_some() {
                return Err(CompactError::UnsupportedSchema("items on object".into()));
            }
            object_flag(schema)?;
            let properties = match schema.get("properties") {
                Some(v) => v.as_object().ok_or_else(|| {
                    CompactError::UnsupportedSchema("properties must be an object".into())
                })?,
                None => {
                    if schema.get("required").is_some() {
                        return Err(CompactError::UnsupportedSchema(
                            "required without properties".into(),
                        ));
                    }
                    return Ok(());
                }
            };
            for (name, child) in properties {
                if !valid_name(name) {
                    return Err(CompactError::UnsupportedSchema(format!(
                        "property name: {name}"
                    )));
                }
                check_schema(child, depth + 1)?;
            }
            if let Some(required) = schema.get("required") {
                let required = required.as_array().ok_or_else(|| {
                    CompactError::UnsupportedSchema("required must be an array".into())
                })?;
                let mut seen = std::collections::HashSet::new();
                for item in required {
                    let name = item.as_str().ok_or_else(|| {
                        CompactError::UnsupportedSchema("required name must be a string".into())
                    })?;
                    if !properties.contains_key(name) || !seen.insert(name) {
                        return Err(CompactError::UnsupportedSchema(format!(
                            "required property absent: {name}"
                        )));
                    }
                }
            }
        }
        "array" => {
            if ["properties", "required", "additionalProperties"]
                .iter()
                .any(|k| schema.get(*k).is_some())
            {
                return Err(CompactError::UnsupportedSchema(
                    "object keyword on array".into(),
                ));
            }
            check_schema(
                schema
                    .get("items")
                    .ok_or_else(|| CompactError::UnsupportedSchema("array missing items".into()))?,
                depth + 1,
            )?;
        }
        _ => {
            if ["properties", "required", "items", "additionalProperties"]
                .iter()
                .any(|k| schema.get(*k).is_some())
            {
                return Err(CompactError::UnsupportedSchema(
                    "object/array keyword on primitive".into(),
                ));
            }
        }
    }
    Ok(())
}

pub fn validate_arguments(schema: Option<&Value>, arguments: &Value) -> Result<()> {
    let args = arguments
        .as_object()
        .ok_or_else(|| CompactError::InvalidArguments("arguments must be an object".into()))?;
    if let Some(schema) = schema {
        check_schema(schema, 0)?;
        if type_name(schema)? != "object" {
            return Err(CompactError::UnsupportedSchema(
                "parameters must be object".into(),
            ));
        }
        validate_object(schema, args, 0)?;
    } else if !args.is_empty() {
        return Err(CompactError::InvalidArguments(
            "tool takes no arguments".into(),
        ));
    }
    Ok(())
}

fn validate_object(schema: &Value, args: &Map<String, Value>, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(CompactError::InvalidArguments("arguments too deep".into()));
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for field in required {
            let name = field.as_str().unwrap();
            if !args.contains_key(name) {
                return Err(CompactError::InvalidArguments(format!(
                    "missing required field: {name}"
                )));
            }
        }
    }
    let props = schema.get("properties").and_then(Value::as_object);
    for (name, value) in args {
        if let Some(child) = props.and_then(|p| p.get(name)) {
            validate_value(child, value, depth + 1)?;
        } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            return Err(CompactError::InvalidArguments(format!(
                "unknown field: {name}"
            )));
        }
    }
    Ok(())
}

fn validate_value(schema: &Value, value: &Value, depth: usize) -> Result<()> {
    let kind = type_name(schema)?;
    if !primitive_matches(kind, value, nullable(schema)) {
        return Err(CompactError::InvalidArguments(format!(
            "expected {kind}, got {value}"
        )));
    }
    if let Some(enums) = schema.get("enum").and_then(Value::as_array)
        && !enums.contains(value)
    {
        return Err(CompactError::InvalidArguments("enum violation".into()));
    }
    if value.is_null() {
        return Ok(());
    }
    match kind {
        "object" => validate_object(schema, value.as_object().unwrap(), depth)?,
        "array" => {
            let items = schema.get("items").unwrap();
            for item in value.as_array().unwrap() {
                validate_value(items, item, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn primitive_matches(kind: &str, value: &Value, nullable: bool) -> bool {
    if nullable && value.is_null() {
        return true;
    }
    match kind {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization")
}

fn object_schema(
    properties: Map<String, Value>,
    required: Vec<Value>,
    flag: Option<bool>,
) -> Value {
    let mut obj = Map::new();
    obj.insert("type".into(), Value::String("object".into()));
    if !properties.is_empty() {
        obj.insert("properties".into(), Value::Object(properties));
    }
    if !required.is_empty() {
        obj.insert("required".into(), Value::Array(required));
    }
    if let Some(closed) = flag {
        obj.insert("additionalProperties".into(), Value::Bool(!closed));
    }
    Value::Object(obj)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}
impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }
    fn done(&self) -> bool {
        self.pos == self.src.len()
    }
    fn take(&mut self, ch: char) -> bool {
        if self.rest().starts_with(ch) {
            self.pos += ch.len_utf8();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, ch: char) -> Result<()> {
        if self.take(ch) {
            Ok(())
        } else {
            Err(CompactError::InvalidDefinition(format!("expected {ch}")))
        }
    }
    fn flag(&mut self) -> Option<bool> {
        if self.take('!') {
            Some(true)
        } else if self.take('+') {
            Some(false)
        } else {
            None
        }
    }
    fn ident(&mut self) -> Result<String> {
        let n = self
            .rest()
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
            .count();
        if n == 0 {
            return Err(CompactError::InvalidDefinition(
                "expected identifier".into(),
            ));
        }
        let out = self.rest()[..n].to_string();
        self.pos += n;
        Ok(out)
    }
    fn string(&mut self) -> Result<String> {
        let mut des = serde_json::Deserializer::from_str(self.rest()).into_iter::<String>();
        let s = des
            .next()
            .ok_or_else(|| CompactError::InvalidDefinition("expected string".into()))?
            .map_err(|e| CompactError::InvalidDefinition(e.to_string()))?;
        self.pos += des.byte_offset();
        Ok(s)
    }
    fn fields(&mut self, end: char) -> Result<(Map<String, Value>, Vec<Value>)> {
        let mut properties = Map::new();
        let mut required = Vec::new();
        if self.rest().starts_with(end) {
            return Ok((properties, required));
        }
        loop {
            let name = self.ident()?;
            let is_required = if self.take('!') {
                true
            } else {
                self.expect('?')?;
                false
            };
            self.expect(':')?;
            let schema = self.schema_type()?;
            if properties.insert(name.clone(), schema).is_some() {
                return Err(CompactError::InvalidDefinition("duplicate property".into()));
            }
            if is_required {
                required.push(Value::String(name));
            }
            if !self.take(',') {
                break;
            }
        }
        Ok((properties, required))
    }
    fn schema_type(&mut self) -> Result<Value> {
        let mut schema = if self.take('[') {
            let item = self.schema_type()?;
            self.expect(']')?;
            serde_json::json!({"type":"array","items":item})
        } else {
            let token = self.ident()?;
            if token == "obj" {
                let flag = self.flag();
                self.expect('{')?;
                let (properties, required) = self.fields('}')?;
                self.expect('}')?;
                object_schema(properties, required, flag)
            } else {
                let kind = match token.as_str() {
                    "str" => "string",
                    "int" => "integer",
                    "num" => "number",
                    "bool" => "boolean",
                    "null" => "null",
                    _ => {
                        return Err(CompactError::InvalidDefinition(format!(
                            "unknown type: {token}"
                        )));
                    }
                };
                serde_json::json!({"type":kind})
            }
        };
        if self.take('@') {
            let format = self.ident()?;
            schema
                .as_object_mut()
                .unwrap()
                .insert("format".into(), Value::String(format));
        }
        if self.take('{') {
            let mut values = Vec::new();
            loop {
                let mut des = serde_json::Deserializer::from_str(self.rest()).into_iter::<Value>();
                let value = des
                    .next()
                    .ok_or_else(|| CompactError::InvalidDefinition("expected enum value".into()))?
                    .map_err(|e| CompactError::InvalidDefinition(e.to_string()))?;
                self.pos += des.byte_offset();
                values.push(value);
                if !self.take(',') {
                    break;
                }
            }
            self.expect('}')?;
            schema
                .as_object_mut()
                .unwrap()
                .insert("enum".into(), Value::Array(values));
        }
        if self.rest().starts_with("|null") {
            self.pos += 5;
            let kind = schema
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| CompactError::InvalidDefinition("invalid nullable type".into()))?
                .to_string();
            schema
                .as_object_mut()
                .unwrap()
                .insert("type".into(), serde_json::json!([kind, "null"]));
        }
        if self.take('~') {
            schema
                .as_object_mut()
                .unwrap()
                .insert("description".into(), Value::String(self.string()?));
        }
        Ok(schema)
    }
}

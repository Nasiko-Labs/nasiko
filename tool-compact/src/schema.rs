use crate::error::{Result, ToolCompactError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Primitive and composite data types supported by Nasiko Tool Compact schemas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
}

impl std::fmt::Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueType::String => write!(f, "string"),
            ValueType::Integer => write!(f, "int"),
            ValueType::Number => write!(f, "number"),
            ValueType::Boolean => write!(f, "bool"),
            ValueType::Array => write!(f, "array"),
            ValueType::Object => write!(f, "object"),
        }
    }
}

/// Schema definition for an individual tool parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParameterSchema {
    pub name: String,
    pub val_type: ValueType,
    pub description: Option<String>,
    pub required: bool,
    pub enum_values: Option<Vec<String>>,
    pub default: Option<serde_json::Value>,
    pub item_type: Option<Box<ValueType>>,
    pub properties: Option<Vec<ParameterSchema>>,
    #[serde(default)]
    pub format: Option<String>,
}

impl ParameterSchema {
    pub fn new(name: impl Into<String>, val_type: ValueType, required: bool) -> Self {
        Self {
            name: name.into(),
            val_type,
            description: None,
            required,
            enum_values: None,
            default: None,
            item_type: None,
            properties: None,
            format: None,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_enum(mut self, values: Vec<String>) -> Self {
        self.enum_values = Some(values);
        self
    }

    pub fn with_default(mut self, val: serde_json::Value) -> Self {
        self.default = Some(val);
        self
    }

    pub fn with_item_type(mut self, item_type: ValueType) -> Self {
        self.item_type = Some(Box::new(item_type));
        self
    }

    pub fn with_properties(mut self, props: Vec<ParameterSchema>) -> Self {
        self.properties = Some(props);
        self
    }
}

/// Schema definition for a tool/function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ParameterSchema>,
}

impl ToolSchema {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: Vec::new(),
        }
    }

    pub fn with_parameter(mut self, param: ParameterSchema) -> Self {
        self.parameters.push(param);
        self
    }

    pub fn get_parameter(&self, name: &str) -> Option<&ParameterSchema> {
        self.parameters.iter().find(|p| p.name == name)
    }

    /// Convert from an OpenAI-style tool definition.
    /// Accepts `{"type":"function","function":{...}}` or the bare function object.
    /// Returns `SchemaError("unsupported schema ...")` for anything we cannot express
    /// losslessly; callers must then bypass compaction.
    pub fn from_json_value(value: &Value) -> Result<Self> {
        let func = value.get("function").unwrap_or(value);
        let name = func
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCompactError::SchemaError("Tool schema missing 'name'".into()))?;
        let description = func
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut parameters = Vec::new();
        if let Some(params) = func.get("parameters").or_else(|| func.get("input_schema")) {
            check_keywords(params, name)?;
            if let Some(t) = params.get("type") {
                if t.as_str() != Some("object") {
                    return Err(bad(name, "parameters.type must be 'object'"));
                }
            }
            parameters = parse_object_props(params, name)?;
        }

        Ok(Self {
            name: name.to_string(),
            description,
            parameters,
        })
    }
}

const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "$ref",
    "$defs",
    "definitions",
    "patternProperties",
    "pattern",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
    "const",
    "if",
    "then",
    "else",
];

fn bad(path: &str, msg: &str) -> ToolCompactError {
    ToolCompactError::SchemaError(format!("unsupported schema at '{path}': {msg}"))
}

fn check_keywords(def: &Value, path: &str) -> Result<()> {
    if let Some(obj) = def.as_object() {
        for k in UNSUPPORTED_KEYWORDS {
            if obj.contains_key(*k) {
                return Err(bad(path, &format!("keyword '{k}'")));
            }
        }
        if let Some(ap) = obj.get("additionalProperties") {
            if *ap != Value::Bool(false) {
                return Err(bad(path, "additionalProperties other than false"));
            }
        }
    }
    Ok(())
}

fn primitive(t: &str) -> Option<ValueType> {
    match t {
        "string" => Some(ValueType::String),
        "integer" => Some(ValueType::Integer),
        "number" => Some(ValueType::Number),
        "boolean" => Some(ValueType::Boolean),
        _ => None,
    }
}

fn parse_object_props(def: &Value, path: &str) -> Result<Vec<ParameterSchema>> {
    let required: Vec<String> = match def.get("required") {
        None => Vec::new(),
        Some(r) => r
            .as_array()
            .ok_or_else(|| bad(path, "'required' must be an array"))?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| bad(path, "'required' entries must be strings"))
            })
            .collect::<Result<Vec<_>>>()?,
    };
    let empty = serde_json::Map::new();
    let props = match def.get("properties") {
        None => &empty,
        Some(p) => p
            .as_object()
            .ok_or_else(|| bad(path, "'properties' must be an object"))?,
    };
    for r in &required {
        if !props.contains_key(r) {
            return Err(bad(
                path,
                &format!("required field '{r}' is not in properties"),
            ));
        }
    }
    props
        .iter()
        .map(|(n, d)| parse_parameter_json(n, d, required.contains(n), path))
        .collect()
}

fn parse_parameter_json(
    name: &str,
    def: &Value,
    required: bool,
    parent: &str,
) -> Result<ParameterSchema> {
    let path = format!("{parent}.{name}");
    check_keywords(def, &path)?;

    let type_str = def
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| bad(&path, "missing or non-string 'type'"))?;
    let val_type = match type_str {
        "array" => ValueType::Array,
        "object" => ValueType::Object,
        other => primitive(other).ok_or_else(|| bad(&path, &format!("type '{other}'")))?,
    };

    let description = def
        .get("description")
        .and_then(|d| d.as_str())
        .map(String::from);
    let default = def.get("default").cloned();
    let format = def.get("format").and_then(|f| f.as_str()).map(String::from);

    let enum_values = match def.get("enum") {
        None => None,
        Some(e) => {
            if val_type != ValueType::String {
                return Err(bad(&path, "enum on a non-string type"));
            }
            let arr = e
                .as_array()
                .ok_or_else(|| bad(&path, "'enum' must be an array"))?;
            Some(
                arr.iter()
                    .map(|v| {
                        v.as_str()
                            .map(String::from)
                            .ok_or_else(|| bad(&path, "non-string enum value"))
                    })
                    .collect::<Result<Vec<_>>>()?,
            )
        }
    };

    let item_type = if val_type == ValueType::Array {
        let items = def
            .get("items")
            .ok_or_else(|| bad(&path, "array without 'items'"))?;
        check_keywords(items, &path)?;
        let t = items
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| bad(&path, "array 'items' without a string 'type'"))?;
        let item =
            primitive(t).ok_or_else(|| bad(&path, &format!("array items of type '{t}'")))?;
        Some(Box::new(item))
    } else {
        None
    };

    let properties = if val_type == ValueType::Object && def.get("properties").is_some() {
        Some(parse_object_props(def, &path)?)
    } else {
        None
    };

    Ok(ParameterSchema {
        name: name.to_string(),
        val_type,
        description,
        required,
        enum_values,
        default,
        item_type,
        properties,
        format,
    })
}

/// Registry holding available tool schemas for lookup and validation.
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolSchema>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, schema: ToolSchema) {
        self.tools.insert(schema.name.clone(), schema);
    }

    pub fn get(&self, name: &str) -> Option<&ToolSchema> {
        self.tools.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolSchema> {
        self.tools.values()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
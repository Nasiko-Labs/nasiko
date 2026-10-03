//! The internal schema model shared by encoding, [`crate::decode_tools`] and validation.
//!
//! A JSON Schema is parsed once into a [`Node`] tree that mirrors the subset of keywords this
//! crate understands. Validation reads the tree with JSON Schema semantics; the encoder renders
//! it. Anything the compact text cannot carry exactly is recorded as a [`Blocker`] instead of
//! being dropped, so a tool is either compacted losslessly or bypassed — never degraded.
//!
//! [`to_json`] turns a tree back into JSON Schema. Applied to the tree parsed from the original
//! schema it gives the canonical form; applied to the tree parsed from the compact text it gives
//! what [`crate::decode_tools`] returns. The round-trip guarantee is that the two are equal.

use regex::Regex;
use serde_json::{Map, Number, Value};

use crate::error::UnsupportedFeature;
use crate::models::ToolDef;

/// A JSON Schema primitive type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    String,
    Integer,
    Number,
    Boolean,
    Null,
    Object,
    Array,
}

impl Kind {
    pub(crate) const ALL: [Kind; 7] = [
        Kind::String,
        Kind::Integer,
        Kind::Number,
        Kind::Boolean,
        Kind::Null,
        Kind::Object,
        Kind::Array,
    ];

    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Kind::String => "string",
            Kind::Integer => "integer",
            Kind::Number => "number",
            Kind::Boolean => "boolean",
            Kind::Null => "null",
            Kind::Object => "object",
            Kind::Array => "array",
        }
    }

    pub(crate) fn from_keyword(word: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.keyword() == word)
    }

    /// The kind an enum or const value implies. Integer-valued numbers count as integers unless
    /// `prefer_number` (some other value in the same enum is fractional).
    pub(crate) fn of_value(value: &Value, prefer_number: bool) -> Kind {
        match value {
            Value::String(_) => Kind::String,
            Value::Bool(_) => Kind::Boolean,
            Value::Null => Kind::Null,
            Value::Number(n) if !prefer_number && (n.is_i64() || n.is_u64()) => Kind::Integer,
            Value::Number(_) => Kind::Number,
            Value::Array(_) => Kind::Array,
            Value::Object(_) => Kind::Object,
        }
    }
}

/// The kinds an enum's values imply, in first-appearance order. When the declared `type`
/// equals this, the compact text does not need to spell the type out.
pub(crate) fn implied_kinds(values: &[Value]) -> Vec<Kind> {
    let prefer_number = values
        .iter()
        .any(|v| matches!(v, Value::Number(n) if !(n.is_i64() || n.is_u64())));
    let mut kinds = Vec::new();
    for v in values {
        let kind = Kind::of_value(v, prefer_number);
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

/// A compiled `pattern`. Equality is on the source text, which is what the schema states.
#[derive(Debug, Clone)]
pub(crate) struct Pattern {
    pub source: String,
    pub regex: Regex,
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Pattern {
    pub(crate) fn compile(source: &str) -> Option<Pattern> {
        Regex::new(source).ok().map(|regex| Pattern {
            source: source.to_string(),
            regex,
        })
    }
}

/// The validation keywords the grammar renders as annotations.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Bounds {
    pub minimum: Option<Number>,
    pub maximum: Option<Number>,
    pub exclusive_minimum: Option<Number>,
    pub exclusive_maximum: Option<Number>,
    pub multiple_of: Option<Number>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub pattern: Option<Pattern>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: Option<bool>,
    pub min_properties: Option<u64>,
    pub max_properties: Option<u64>,
}

/// `additionalProperties`, kept as written.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum Additional {
    #[default]
    Unspecified,
    Allowed(bool),
    Schema(Box<Node>),
}

/// The object keywords of one schema node.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ObjectShape {
    /// `None` when the schema has no `properties` keyword (a free-form object).
    pub properties: Option<Vec<Prop>>,
    /// `None` when the schema has no `required` keyword.
    pub required: Option<Vec<String>>,
    pub additional: Additional,
}

impl ObjectShape {
    pub(crate) fn is_required(&self, name: &str) -> bool {
        self.required
            .as_ref()
            .is_some_and(|r| r.iter().any(|n| n == name))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prop {
    pub name: String,
    pub node: Node,
}

/// One schema node: the keywords this crate understands, as written.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Node {
    /// Empty when the schema has no `type` keyword.
    pub types: Vec<Kind>,
    pub format: Option<String>,
    pub enum_values: Option<Vec<Value>>,
    pub const_value: Option<Value>,
    pub bounds: Bounds,
    pub items: Option<Box<Node>>,
    pub object: Option<ObjectShape>,
    pub description: Option<String>,
    pub default: Option<Value>,
    pub any_of: Option<Vec<Node>>,
    pub one_of: Option<Vec<Node>>,
    pub all_of: Option<Vec<Node>>,
    /// The first assertion keyword this crate cannot enforce. Any call that reaches this node is
    /// rejected rather than passed unchecked.
    pub unvalidatable: Option<String>,
}

/// Something the compact text cannot carry exactly, and where it is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Blocker {
    pub path: String,
    pub feature: UnsupportedFeature,
}

/// A tool's schema, parsed for validation, plus everything that prevents compacting it.
#[derive(Debug, Clone)]
pub(crate) struct ToolSchema {
    pub name: String,
    pub description: Option<String>,
    pub strict: Option<bool>,
    pub root: Node,
    pub blockers: Vec<Blocker>,
}

/// Characters a tool name may use in the compact grammar: OpenAI's function-name set.
pub(crate) fn is_safe_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Characters a property name may use in the compact grammar.
pub(crate) fn is_safe_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Characters a `format` value may use inside `<…>`.
pub(crate) fn is_safe_format(format: &str) -> bool {
    !format.is_empty()
        && format
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Annotation-only keywords: ignored by validation (as JSON Schema specifies), but the compact
/// text has no place for them, so they block compaction.
const ANNOTATION_KEYWORDS: &[&str] = &[
    "title",
    "examples",
    "$schema",
    "$id",
    "$comment",
    "$anchor",
    "deprecated",
    "readOnly",
    "writeOnly",
    "contentMediaType",
    "contentEncoding",
];

/// Assertion keywords this crate does not implement. A call that must satisfy one cannot be
/// proven valid, so it fails closed.
const UNVALIDATABLE_KEYWORDS: &[&str] = &[
    "not",
    "if",
    "then",
    "else",
    "patternProperties",
    "prefixItems",
    "additionalItems",
    "dependentRequired",
    "dependentSchemas",
    "dependencies",
    "propertyNames",
    "contains",
    "minContains",
    "maxContains",
    "unevaluatedProperties",
    "unevaluatedItems",
];

/// Depth guard for `$ref` chains and nesting; deeper schemas are not something a model is shown.
const MAX_DEPTH: usize = 64;

/// Parse a tool's schema.
///
/// A tool without `parameters` gets the empty object schema `{"type":"object","properties":{}}`
/// — the canonical form of "takes no arguments".
pub(crate) fn analyze(tool: &ToolDef) -> ToolSchema {
    let mut parser = Parser {
        defs: None,
        ref_stack: Vec::new(),
        blockers: Vec::new(),
    };
    if !is_safe_tool_name(&tool.name) {
        parser.block("/name", UnsupportedFeature::UnsafeName);
    }
    if let Some(key) = tool.extra.keys().next() {
        parser.block(
            &format!("/function/{key}"),
            UnsupportedFeature::Keyword(key.clone()),
        );
    }
    let root = match &tool.parameters {
        None => empty_object(),
        Some(params) => {
            parser.defs = params
                .get("$defs")
                .or_else(|| params.get("definitions"))
                .and_then(Value::as_object)
                .cloned();
            let root = parser.parse(params, "/parameters", 0);
            if root.types != [Kind::Object] {
                parser.block("/parameters", UnsupportedFeature::NonObjectParameters);
            }
            root
        }
    };
    ToolSchema {
        name: tool.name.clone(),
        description: tool.description.clone(),
        strict: tool.strict,
        root,
        blockers: parser.blockers,
    }
}

pub(crate) fn empty_object() -> Node {
    Node {
        types: vec![Kind::Object],
        object: Some(ObjectShape {
            properties: Some(Vec::new()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

struct Parser {
    defs: Option<Map<String, Value>>,
    ref_stack: Vec<String>,
    blockers: Vec<Blocker>,
}

impl Parser {
    fn block(&mut self, path: &str, feature: UnsupportedFeature) {
        self.blockers.push(Blocker {
            path: path.to_string(),
            feature,
        });
    }

    fn block_keyword(&mut self, path: &str, keyword: &str) {
        self.block(
            &format!("{path}/{keyword}"),
            UnsupportedFeature::Keyword(keyword.to_string()),
        );
    }

    fn parse(&mut self, schema: &Value, path: &str, depth: usize) -> Node {
        let mut node = Node::default();
        if depth > MAX_DEPTH {
            node.unvalidatable = Some("nesting depth".into());
            self.block(path, UnsupportedFeature::Keyword("nesting depth".into()));
            return node;
        }
        let Some(map) = schema.as_object() else {
            // Boolean schemas (`true`/`false`) and non-schemas: valid JSON Schema in the first
            // case, but nothing the compact text renders.
            node.unvalidatable = match schema {
                Value::Bool(true) => None,
                _ => Some("non-object schema".into()),
            };
            self.block(path, UnsupportedFeature::Keyword("boolean schema".into()));
            return node;
        };
        for (keyword, value) in map {
            self.keyword(&mut node, keyword, value, path, depth);
        }
        node
    }

    fn keyword(&mut self, node: &mut Node, keyword: &str, value: &Value, path: &str, depth: usize) {
        match keyword {
            "type" => self.parse_type(node, value, path),
            "format" => match value.as_str() {
                Some(f) => {
                    if !is_safe_format(f) {
                        self.block(&format!("{path}/format"), UnsupportedFeature::UnsafeName);
                    }
                    node.format = Some(f.to_string());
                }
                None => self.invalid(node, path, keyword),
            },
            "enum" => match value.as_array() {
                Some(values) => {
                    if values.is_empty()
                        || values.iter().any(|v| v.is_array() || v.is_object())
                        || values.iter().all(Value::is_null)
                    {
                        self.block(&format!("{path}/enum"), UnsupportedFeature::Enum);
                    }
                    node.enum_values = Some(values.clone());
                }
                None => self.invalid(node, path, keyword),
            },
            "const" => {
                if value.is_array() || value.is_object() {
                    self.block_keyword(path, keyword);
                }
                node.const_value = Some(value.clone());
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
                let Some(n) = value.as_number().cloned() else {
                    // Draft-4 boolean `exclusiveMinimum` and friends: not this dialect.
                    return self.invalid(node, path, keyword);
                };
                let b = &mut node.bounds;
                let slot = match keyword {
                    "minimum" => &mut b.minimum,
                    "maximum" => &mut b.maximum,
                    "exclusiveMinimum" => &mut b.exclusive_minimum,
                    "exclusiveMaximum" => &mut b.exclusive_maximum,
                    _ => &mut b.multiple_of,
                };
                *slot = Some(n);
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" | "minProperties"
            | "maxProperties" => {
                let Some(n) = value.as_u64() else {
                    return self.invalid(node, path, keyword);
                };
                if matches!(keyword, "minProperties" | "maxProperties") {
                    self.block_keyword(path, keyword);
                }
                let b = &mut node.bounds;
                let slot = match keyword {
                    "minLength" => &mut b.min_length,
                    "maxLength" => &mut b.max_length,
                    "minItems" => &mut b.min_items,
                    "maxItems" => &mut b.max_items,
                    "minProperties" => &mut b.min_properties,
                    _ => &mut b.max_properties,
                };
                *slot = Some(n);
            }
            "pattern" => match value.as_str() {
                Some(source) => match Pattern::compile(source) {
                    Some(p) => node.bounds.pattern = Some(p),
                    None => {
                        node.unvalidatable.get_or_insert_with(|| "pattern".into());
                        self.block(&format!("{path}/pattern"), UnsupportedFeature::Pattern);
                    }
                },
                None => self.invalid(node, path, keyword),
            },
            "uniqueItems" => match value.as_bool() {
                Some(b) => node.bounds.unique_items = Some(b),
                None => self.invalid(node, path, keyword),
            },
            "items" => {
                if value.is_array() {
                    // Draft-7 tuple form.
                    node.unvalidatable.get_or_insert_with(|| "items".into());
                    self.block_keyword(path, keyword);
                } else {
                    let child = self.parse(value, &format!("{path}/items"), depth + 1);
                    node.items = Some(Box::new(child));
                }
            }
            "properties" => match value.as_object() {
                Some(props) => {
                    let mut parsed = Vec::with_capacity(props.len());
                    for (name, schema) in props {
                        let prop_path = format!("{path}/properties/{name}");
                        if !is_safe_key(name) {
                            self.block(&prop_path, UnsupportedFeature::UnsafeName);
                        }
                        let child = self.parse(schema, &prop_path, depth + 1);
                        parsed.push(Prop {
                            name: name.clone(),
                            node: child,
                        });
                    }
                    node.object.get_or_insert_with(Default::default).properties = Some(parsed);
                }
                None => self.invalid(node, path, keyword),
            },
            "required" => match value.as_array() {
                Some(names) if names.iter().all(Value::is_string) => {
                    let names = names
                        .iter()
                        .filter_map(|n| n.as_str().map(str::to_string))
                        .collect();
                    node.object.get_or_insert_with(Default::default).required = Some(names);
                }
                _ => self.invalid(node, path, keyword),
            },
            "additionalProperties" => {
                let additional = match value {
                    Value::Bool(b) => Additional::Allowed(*b),
                    other => {
                        self.block_keyword(path, keyword);
                        let child =
                            self.parse(other, &format!("{path}/additionalProperties"), depth + 1);
                        Additional::Schema(Box::new(child))
                    }
                };
                node.object.get_or_insert_with(Default::default).additional = additional;
            }
            "description" => match value.as_str() {
                Some(d) => node.description = Some(d.to_string()),
                None => self.invalid(node, path, keyword),
            },
            "default" => node.default = Some(value.clone()),
            "anyOf" | "oneOf" | "allOf" => {
                self.block_keyword(path, keyword);
                let Some(branches) = value.as_array().filter(|b| !b.is_empty()) else {
                    return self.invalid(node, path, keyword);
                };
                let parsed = branches
                    .iter()
                    .enumerate()
                    .map(|(i, b)| self.parse(b, &format!("{path}/{keyword}/{i}"), depth + 1))
                    .collect();
                match keyword {
                    "anyOf" => node.any_of = Some(parsed),
                    "oneOf" => node.one_of = Some(parsed),
                    _ => node.all_of = Some(parsed),
                }
            }
            "$ref" => {
                self.block_keyword(path, keyword);
                let resolved = self.resolve_ref(value, path, depth);
                node.all_of.get_or_insert_with(Vec::new).push(resolved);
            }
            "$defs" | "definitions" => self.block_keyword(path, keyword),
            kw if ANNOTATION_KEYWORDS.contains(&kw) => self.block_keyword(path, kw),
            kw if UNVALIDATABLE_KEYWORDS.contains(&kw) => {
                node.unvalidatable.get_or_insert_with(|| kw.to_string());
                self.block_keyword(path, kw);
            }
            // Unknown keywords are annotations under JSON Schema, so validation ignores them —
            // but the compact text would lose them, so they block compaction.
            kw => self.block_keyword(path, kw),
        }
    }

    fn parse_type(&mut self, node: &mut Node, value: &Value, path: &str) {
        let words: Vec<&str> = match value {
            Value::String(s) => vec![s.as_str()],
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let kinds: Option<Vec<Kind>> = words.iter().map(|w| Kind::from_keyword(w)).collect();
        match kinds {
            Some(kinds)
                if !kinds.is_empty()
                    && Some(kinds.len()) == value.as_array().map(Vec::len).or(Some(1)) =>
            {
                node.types = kinds;
                if node.types.contains(&Kind::Object) {
                    node.object.get_or_insert_with(Default::default);
                }
            }
            _ => self.invalid(node, path, "type"),
        }
    }

    /// A keyword whose value is not valid for it. The schema itself is malformed, so neither
    /// validating against it nor compacting it can be trusted.
    fn invalid(&mut self, node: &mut Node, path: &str, keyword: &str) {
        node.unvalidatable
            .get_or_insert_with(|| format!("malformed `{keyword}`"));
        self.block_keyword(path, keyword);
    }

    fn resolve_ref(&mut self, value: &Value, path: &str, depth: usize) -> Node {
        let target = value.as_str().and_then(|r| {
            r.strip_prefix("#/$defs/")
                .or_else(|| r.strip_prefix("#/definitions/"))
                .map(str::to_string)
        });
        let schema = target
            .as_ref()
            .and_then(|name| self.defs.as_ref().and_then(|d| d.get(name)).cloned());
        let (Some(name), Some(schema)) = (target, schema) else {
            return Node {
                unvalidatable: Some("$ref".into()),
                ..Default::default()
            };
        };
        if self.ref_stack.contains(&name) {
            self.block(path, UnsupportedFeature::RecursiveRef);
            return Node {
                unvalidatable: Some("recursive $ref".into()),
                ..Default::default()
            };
        }
        self.ref_stack.push(name.clone());
        let node = self.parse(&schema, &format!("#/$defs/{name}"), depth + 1);
        self.ref_stack.pop();
        node
    }
}

/// Serialize a tree back to JSON Schema (the canonical form).
///
/// Canonical means: keys in `serde_json`'s order, `required: []` omitted. Everything else is
/// exactly what the tree holds.
pub(crate) fn to_json(node: &Node) -> Value {
    let mut m = Map::new();
    match node.types.as_slice() {
        [] => {}
        [single] => {
            m.insert("type".into(), Value::from(single.keyword()));
        }
        many => {
            let list = many.iter().map(|k| Value::from(k.keyword())).collect();
            m.insert("type".into(), Value::Array(list));
        }
    }
    if let Some(f) = &node.format {
        m.insert("format".into(), Value::from(f.as_str()));
    }
    if let Some(values) = &node.enum_values {
        m.insert("enum".into(), Value::Array(values.clone()));
    }
    if let Some(c) = &node.const_value {
        m.insert("const".into(), c.clone());
    }
    insert_bounds(&mut m, &node.bounds);
    if let Some(items) = &node.items {
        m.insert("items".into(), to_json(items));
    }
    if let Some(shape) = &node.object {
        if let Some(props) = &shape.properties {
            let props = props
                .iter()
                .map(|p| (p.name.clone(), to_json(&p.node)))
                .collect();
            m.insert("properties".into(), Value::Object(props));
        }
        if let Some(required) = shape.required.as_ref().filter(|r| !r.is_empty()) {
            let list = required.iter().map(|n| Value::from(n.as_str())).collect();
            m.insert("required".into(), Value::Array(list));
        }
        match &shape.additional {
            Additional::Unspecified => {}
            Additional::Allowed(b) => {
                m.insert("additionalProperties".into(), Value::Bool(*b));
            }
            Additional::Schema(s) => {
                m.insert("additionalProperties".into(), to_json(s));
            }
        }
    }
    if let Some(d) = &node.description {
        m.insert("description".into(), Value::from(d.as_str()));
    }
    if let Some(d) = &node.default {
        m.insert("default".into(), d.clone());
    }
    for (key, branches) in [
        ("anyOf", &node.any_of),
        ("oneOf", &node.one_of),
        ("allOf", &node.all_of),
    ] {
        if let Some(branches) = branches {
            m.insert(
                key.into(),
                Value::Array(branches.iter().map(to_json).collect()),
            );
        }
    }
    Value::Object(m)
}

fn insert_bounds(m: &mut Map<String, Value>, b: &Bounds) {
    let numbers = [
        ("minimum", &b.minimum),
        ("maximum", &b.maximum),
        ("exclusiveMinimum", &b.exclusive_minimum),
        ("exclusiveMaximum", &b.exclusive_maximum),
        ("multipleOf", &b.multiple_of),
    ];
    for (key, n) in numbers {
        if let Some(n) = n {
            m.insert(key.into(), Value::Number(n.clone()));
        }
    }
    let counts = [
        ("minLength", b.min_length),
        ("maxLength", b.max_length),
        ("minItems", b.min_items),
        ("maxItems", b.max_items),
        ("minProperties", b.min_properties),
        ("maxProperties", b.max_properties),
    ];
    for (key, n) in counts {
        if let Some(n) = n {
            m.insert(key.into(), Value::from(n));
        }
    }
    if let Some(p) = &b.pattern {
        m.insert("pattern".into(), Value::from(p.source.as_str()));
    }
    if let Some(u) = b.unique_items {
        m.insert("uniqueItems".into(), Value::Bool(u));
    }
}

/// The canonical JSON Schema of a tool's parameters, as [`crate::decode_tools`] reports it for
/// a faithful round trip.
#[cfg(test)]
pub(crate) fn canonical_parameters(tool: &ToolDef) -> Value {
    to_json(&analyze(tool).root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(params: Value) -> ToolDef {
        ToolDef::new("t", Some("A tool.".into()), Some(params))
    }

    fn first_blocker(params: Value) -> Option<Blocker> {
        analyze(&tool(params)).blockers.into_iter().next()
    }

    #[test]
    fn each_unsupported_keyword_blocks_with_its_path() {
        let cases = [
            ("$ref", json!("#/$defs/X")),
            ("anyOf", json!([{"type": "string"}])),
            ("oneOf", json!([{"type": "string"}])),
            ("allOf", json!([{"type": "string"}])),
            ("not", json!({"type": "string"})),
            ("if", json!({"type": "string"})),
            ("patternProperties", json!({"^x": {"type": "string"}})),
            ("prefixItems", json!([{"type": "string"}])),
            ("dependentRequired", json!({"a": ["b"]})),
            ("title", json!("A title")),
            ("examples", json!(["x"])),
            ("x-order", json!(1)),
            ("minProperties", json!(1)),
        ];
        for (keyword, value) in cases {
            let mut prop = Map::new();
            prop.insert("type".into(), json!("string"));
            prop.insert(keyword.into(), value);
            let blocker = first_blocker(json!({
                "type": "object",
                "properties": {"a": Value::Object(prop)}
            }))
            .unwrap_or_else(|| panic!("`{keyword}` did not block"));
            assert_eq!(
                blocker.path,
                format!("/parameters/properties/a/{keyword}"),
                "{keyword}"
            );
            assert_eq!(blocker.feature.as_label(), "unsupported_keyword");
        }
    }

    #[test]
    fn required_naming_an_undeclared_property_is_left_to_the_encoder_but_parses() {
        let schema = analyze(&tool(json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "required": ["a", "ghost"]
        })));
        let shape = schema.root.object.unwrap();
        assert_eq!(shape.required.unwrap(), vec!["a", "ghost"]);
    }

    #[test]
    fn unsafe_names_block() {
        let bad_key = first_blocker(json!({
            "type": "object",
            "properties": {"has space": {"type": "string"}}
        }))
        .unwrap();
        assert_eq!(bad_key.feature, UnsupportedFeature::UnsafeName);

        let bad_tool = analyze(&ToolDef::new("bad name", None, None));
        assert_eq!(bad_tool.blockers[0].feature, UnsupportedFeature::UnsafeName);
    }

    #[test]
    fn inexpressible_enums_block() {
        for values in [json!([]), json!([{"a": 1}]), json!([[1]]), json!([null]), json!([null, null])] {
            let blocker = first_blocker(json!({
                "type": "object",
                "properties": {"a": {"enum": values}}
            }))
            .unwrap();
            assert_eq!(blocker.feature, UnsupportedFeature::Enum);
        }
    }

    #[test]
    fn a_pattern_the_regex_engine_cannot_compile_blocks_and_is_unvalidatable() {
        let schema = analyze(&tool(json!({
            "type": "object",
            "properties": {"a": {"type": "string", "pattern": "(?<=x)y"}}
        })));
        assert_eq!(schema.blockers[0].feature, UnsupportedFeature::Pattern);
        let shape = schema.root.object.unwrap();
        let a = &shape.properties.unwrap()[0].node;
        assert_eq!(a.unvalidatable.as_deref(), Some("pattern"));
    }

    #[test]
    fn unknown_function_level_fields_block() {
        let mut t = ToolDef::new("t", None, None);
        t.extra
            .insert("cache_control".into(), json!({"type": "ephemeral"}));
        let schema = analyze(&t);
        assert_eq!(schema.blockers[0].path, "/function/cache_control");
    }

    #[test]
    fn a_tool_without_parameters_canonicalises_to_the_empty_object() {
        let schema = analyze(&ToolDef::new("ping", None, None));
        assert!(schema.blockers.is_empty());
        assert_eq!(
            to_json(&schema.root),
            json!({"type": "object", "properties": {}})
        );
    }

    #[test]
    fn plain_shapes_parse_without_blockers_and_serialize_back_exactly() {
        let params = json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "n": {"type": "integer", "minimum": 1, "maximum": 10},
                "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 3},
                "kind": {"type": "string", "enum": ["a", "b"]},
                "maybe": {"type": ["string", "null"]},
                "meta": {"type": "object"},
                "nested": {
                    "type": "object",
                    "properties": {"x": {"type": "number"}},
                    "required": ["x"],
                    "additionalProperties": false
                }
            },
            "required": ["title", "start"]
        });
        let schema = analyze(&tool(params.clone()));
        assert!(schema.blockers.is_empty(), "{:?}", schema.blockers);
        assert_eq!(to_json(&schema.root), params);
    }

    #[test]
    fn non_object_parameters_block() {
        let blocker = first_blocker(json!({"type": "string"})).unwrap();
        assert_eq!(blocker.feature, UnsupportedFeature::NonObjectParameters);
    }

    #[test]
    fn recursive_refs_block_and_are_unvalidatable() {
        let schema = analyze(&tool(json!({
            "type": "object",
            "properties": {"tree": {"$ref": "#/$defs/Node"}},
            "$defs": {"Node": {
                "type": "object",
                "properties": {"child": {"$ref": "#/$defs/Node"}}
            }}
        })));
        assert!(
            schema
                .blockers
                .iter()
                .any(|b| b.feature == UnsupportedFeature::RecursiveRef)
        );
    }
}

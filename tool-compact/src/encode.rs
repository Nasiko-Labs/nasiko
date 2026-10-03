//! Rendering tool schemas as compact definitions, and calls in the call grammar.
//!
//! The renderer only ever writes what [`crate::decode_tools`] reads back. Where a schema holds
//! something with no place in the text, rendering stops with [`CompactError::Unsupported`]
//! rather than writing an approximation.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::error::{CompactError, UnsupportedFeature};
use crate::grammar::{alias_for_format, escape_description, literal};
use crate::models::{CompactTools, ToolCall, ToolDef};
use crate::schema::{self, Additional, Bounds, Kind, Node, ObjectShape, ToolSchema, implied_kinds};

/// The call-format instruction placed before the definitions.
///
/// Every token here is paid on every request, so it is the shortest wording that was measured to
/// work. The quoted-key example is not decoration: with a bare `{JSON args}` placeholder, a small
/// model mirrored the definitions' `key: type` style and wrote `{city: "Paris"}` (rejected, since
/// decoding never repairs JSON), or dropped the closing `>>`. Optionality is carried by the
/// definitions themselves (`?`).
pub const INSTRUCTION: &str =
    r#"Call tools with <<call NAME {"key": value}>> (strict JSON, quoted keys), one per line."#;

/// Render `tools` as compact definitions.
///
/// Fails with [`CompactError::Unsupported`] — the caller's cue to send the native definitions
/// — when any tool uses something the compact text cannot carry exactly, or two tools share a
/// name. The result is a pure function of the input.
///
/// ```
/// use nasiko_tool_compact::{encode_tools, ToolDef};
/// use serde_json::json;
///
/// let tool = ToolDef::new(
///     "get_weather",
///     Some("Current weather for a city.".into()),
///     Some(json!({
///         "type": "object",
///         "properties": {
///             "city": {"type": "string", "description": "City name"},
///             "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}
///         },
///         "required": ["city"]
///     })),
/// );
/// let compact = encode_tools(&[tool]).unwrap();
/// assert_eq!(
///     compact.definitions(),
///     "get_weather: Current weather for a city.\n city: string # City name\n unit?: celsius|fahrenheit"
/// );
/// ```
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut names = BTreeSet::new();
    let mut lines = Vec::new();
    for tool in tools {
        if !names.insert(tool.name.as_str()) {
            return Err(unsupported(
                &tool.name,
                "/name",
                UnsupportedFeature::DuplicateTool,
            ));
        }
        let schema = schema::analyze(tool);
        if let Some(blocker) = schema.blockers.first() {
            return Err(unsupported(
                &tool.name,
                &blocker.path,
                blocker.feature.clone(),
            ));
        }
        Renderer { tool: &schema.name }.tool(&schema, &mut lines)?;
    }
    Ok(CompactTools::from_definitions(lines.join("\n")))
}

/// Write calls in the call grammar, one per line: `<<call NAME {json}>>`.
///
/// ```
/// use nasiko_tool_compact::{render_calls, ToolCall};
/// use serde_json::json;
///
/// let call = ToolCall { name: "ping".into(), arguments: json!({"n": 1}).as_object().unwrap().clone() };
/// assert_eq!(render_calls(&[call]), r#"<<call ping {"n":1}>>"#);
/// assert_eq!(render_calls(&[]), "");
/// ```
pub fn render_calls(calls: &[ToolCall]) -> String {
    calls
        .iter()
        .map(|c| format!("<<call {} {}>>", c.name, c.arguments_json()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn unsupported(tool: &str, path: &str, feature: UnsupportedFeature) -> CompactError {
    CompactError::Unsupported {
        tool: tool.to_string(),
        path: path.to_string(),
        feature,
    }
}

struct Renderer<'a> {
    tool: &'a str,
}

impl Renderer<'_> {
    fn keyword(&self, path: &str, keyword: &str) -> CompactError {
        unsupported(
            self.tool,
            &format!("{path}/{keyword}"),
            UnsupportedFeature::Keyword(keyword.to_string()),
        )
    }

    fn tool(&self, schema: &ToolSchema, lines: &mut Vec<String>) -> Result<(), CompactError> {
        let root = &schema.root;
        if let Some(extra) = root_extra(root) {
            return Err(self.keyword("/parameters", extra));
        }
        let shape = root
            .object
            .as_ref()
            .ok_or_else(|| self.keyword("/parameters", "properties"))?;
        let mut annotations = self.object_annotations(shape);
        if schema.parameters_absent {
            annotations.push("noparams".into());
        }
        match schema.strict {
            Some(true) => annotations.push("strict".into()),
            Some(false) => annotations.push("nonstrict".into()),
            None => {}
        }
        let mut header = with_annotations(schema.name.clone(), &annotations);
        header.push(':');
        if let Some(description) = &schema.description {
            header.push(' ');
            header.push_str(&escape_description(description));
        }
        lines.push(header);
        self.properties(shape, 1, "/parameters", lines)
    }

    fn object_annotations(&self, shape: &ObjectShape) -> Vec<String> {
        let mut out = Vec::new();
        if shape.properties.is_none() {
            out.push("any".to_string());
        }
        // `required: []` reads back differently from no `required` at all, so it is marked.
        if shape.required.as_ref().is_some_and(Vec::is_empty) {
            out.push("noreq".to_string());
        }
        match shape.additional {
            Additional::Allowed(false) => out.push("closed".into()),
            Additional::Allowed(true) => out.push("open".into()),
            Additional::Unspecified | Additional::Schema(_) => {}
        }
        out
    }

    /// One line per property: required ones first, in `required` order (so the round trip
    /// reproduces that order exactly), then optional ones.
    fn properties(
        &self,
        shape: &ObjectShape,
        depth: usize,
        path: &str,
        lines: &mut Vec<String>,
    ) -> Result<(), CompactError> {
        let Some(props) = &shape.properties else {
            // A free-form object has no property lines to mark required ones on.
            if shape.required.as_ref().is_some_and(|r| !r.is_empty()) {
                return Err(unsupported(
                    self.tool,
                    &format!("{path}/required"),
                    UnsupportedFeature::InconsistentRequired,
                ));
            }
            return Ok(());
        };
        let required = shape.required.as_deref().unwrap_or_default();
        let unique: BTreeSet<&str> = required.iter().map(String::as_str).collect();
        if unique.len() != required.len() {
            return Err(self.keyword(path, "required"));
        }
        for name in required {
            let prop = props.iter().find(|p| &p.name == name).ok_or_else(|| {
                unsupported(
                    self.tool,
                    &format!("{path}/required"),
                    UnsupportedFeature::InconsistentRequired,
                )
            })?;
            self.property(&prop.name, &prop.node, true, depth, path, lines)?;
        }
        for prop in props.iter().filter(|p| !shape.is_required(&p.name)) {
            self.property(&prop.name, &prop.node, false, depth, path, lines)?;
        }
        Ok(())
    }

    fn property(
        &self,
        name: &str,
        node: &Node,
        required: bool,
        depth: usize,
        parent: &str,
        lines: &mut Vec<String>,
    ) -> Result<(), CompactError> {
        let path = format!("{parent}/properties/{name}");
        let (type_expr, children) = self.type_expr(node, &path)?;
        let mut line = format!(
            "{}{name}{}: {type_expr}",
            " ".repeat(depth),
            if required { "" } else { "?" }
        );
        if let Some(default) = &node.default {
            line.push_str(" = ");
            line.push_str(&default.to_string());
        }
        if let Some(description) = &node.description {
            line.push_str(" #");
            if !description.is_empty() {
                line.push(' ');
                line.push_str(&escape_description(description));
            }
        }
        lines.push(line);
        match children {
            Some(shape) => self.properties(shape, depth + 1, &path, lines),
            None => Ok(()),
        }
    }

    /// The type expression for `node`, plus the object whose properties become indented child
    /// lines (an object, or the innermost object of an array of objects).
    fn type_expr<'n>(
        &self,
        node: &'n Node,
        path: &str,
    ) -> Result<(String, Option<&'n ObjectShape>), CompactError> {
        if node.const_value.is_some() && node.enum_values.is_some() {
            return Err(self.keyword(path, "const"));
        }
        if let Some(value) = &node.const_value {
            return self
                .literal_expr(node, std::slice::from_ref(value), true, path)
                .map(|s| (s, None));
        }
        if let Some(values) = &node.enum_values {
            return self
                .literal_expr(node, values, false, path)
                .map(|s| (s, None));
        }
        match node.types.as_slice() {
            [] => {
                if node.items.is_some() || node.object.is_some() || node.format.is_some() {
                    return Err(self.keyword(path, "type"));
                }
                Ok((
                    with_annotations("any".into(), &bound_annotations(&node.bounds)),
                    None,
                ))
            }
            [kind] => self.kind_expr(node, *kind, path),
            kinds => self.union_expr(node, kinds, path),
        }
    }

    /// `a|b|c`, or one value marked `const`. The declared `type` is spelled out only when it
    /// differs from what the values imply.
    fn literal_expr(
        &self,
        node: &Node,
        values: &[Value],
        is_const: bool,
        path: &str,
    ) -> Result<String, CompactError> {
        if node.items.is_some() || node.format.is_some() || node.object.is_some() {
            let keyword = if is_const { "const" } else { "enum" };
            return Err(self.keyword(path, keyword));
        }
        let mut annotations = bound_annotations(&node.bounds);
        if is_const {
            annotations.push("const".into());
        }
        if node.types != implied_kinds(values) {
            annotations.push(format!("type={}", kinds_label(&node.types)));
        }
        let expr = values.iter().map(literal).collect::<Vec<_>>().join("|");
        Ok(with_annotations(expr, &annotations))
    }

    /// `string|null` and friends. Keywords other than `type` may belong to at most one non-null
    /// kind; they are written on that kind's term.
    fn union_expr<'n>(
        &self,
        node: &'n Node,
        kinds: &[Kind],
        path: &str,
    ) -> Result<(String, Option<&'n ObjectShape>), CompactError> {
        let carriers: Vec<Kind> = kinds.iter().copied().filter(|k| *k != Kind::Null).collect();
        if carriers.len() > 1 && has_kind_keywords(node) {
            return Err(self.keyword(path, "type"));
        }
        let mut parts = Vec::with_capacity(kinds.len());
        let mut children = None;
        for kind in kinds {
            if carriers.first() == Some(kind) {
                let (expr, shape) = self.kind_expr(node, *kind, path)?;
                parts.push(expr);
                children = shape;
            } else {
                parts.push(kind.keyword().to_string());
            }
        }
        Ok((parts.join("|"), children))
    }

    fn kind_expr<'n>(
        &self,
        node: &'n Node,
        kind: Kind,
        path: &str,
    ) -> Result<(String, Option<&'n ObjectShape>), CompactError> {
        if kind != Kind::Array && node.items.is_some() {
            return Err(self.keyword(path, "items"));
        }
        if kind != Kind::Object && node.object.is_some() {
            return Err(self.keyword(path, "properties"));
        }
        let mut annotations = bound_annotations(&node.bounds);
        let mut children = None;
        let base = match kind {
            Kind::Array => {
                if node.format.is_some() {
                    return Err(self.keyword(path, "format"));
                }
                match &node.items {
                    None => "array".to_string(),
                    Some(item) => {
                        let items_path = format!("{path}/items");
                        if item.description.is_some() {
                            return Err(self.keyword(&items_path, "description"));
                        }
                        if item.default.is_some() {
                            return Err(self.keyword(&items_path, "default"));
                        }
                        let (inner, shape) = self.type_expr(item, &items_path)?;
                        children = shape;
                        // Values are always grouped: `(a)[]`, never `a[]`, which would read as
                        // an array of a type named `a`.
                        let is_values = item.enum_values.is_some() || item.const_value.is_some();
                        if is_values || inner.contains(['|', ' ']) {
                            format!("({inner})[]")
                        } else {
                            format!("{inner}[]")
                        }
                    }
                }
            }
            Kind::Object => {
                let shape = node
                    .object
                    .as_ref()
                    .ok_or_else(|| self.keyword(path, "type"))?;
                annotations.extend(self.object_annotations(shape));
                if shape.properties.as_ref().is_some_and(|p| !p.is_empty()) {
                    children = Some(shape);
                }
                with_format("object", node.format.as_deref())
            }
            Kind::String => match node.format.as_deref() {
                Some(format) => match alias_for_format(format) {
                    Some(alias) => alias.to_string(),
                    None => format!("string<{format}>"),
                },
                None => "string".to_string(),
            },
            other => with_format(other.keyword(), node.format.as_deref()),
        };
        Ok((with_annotations(base, &annotations), children))
    }
}

/// The first keyword on the root `parameters` object that has no place in a header line.
fn root_extra(root: &Node) -> Option<&'static str> {
    if root.description.is_some() {
        Some("description")
    } else if root.default.is_some() {
        Some("default")
    } else if root.format.is_some() {
        Some("format")
    } else if root.enum_values.is_some() {
        Some("enum")
    } else if root.const_value.is_some() {
        Some("const")
    } else if root.items.is_some() {
        Some("items")
    } else if root.bounds != Bounds::default() {
        Some("bounds")
    } else {
        None
    }
}

fn has_kind_keywords(node: &Node) -> bool {
    node.format.is_some()
        || node.items.is_some()
        || node.object.is_some()
        || node.bounds != Bounds::default()
}

fn with_format(keyword: &str, format: Option<&str>) -> String {
    match format {
        Some(f) => format!("{keyword}<{f}>"),
        None => keyword.to_string(),
    }
}

fn with_annotations(base: String, annotations: &[String]) -> String {
    if annotations.is_empty() {
        base
    } else {
        format!("{base} ({})", annotations.join(", "))
    }
}

fn kinds_label(kinds: &[Kind]) -> String {
    if kinds.is_empty() {
        "none".to_string()
    } else {
        kinds
            .iter()
            .map(|k| k.keyword())
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// Validation keywords as annotations, in a fixed order so rendering is deterministic.
fn bound_annotations(b: &Bounds) -> Vec<String> {
    let mut out = Vec::new();
    let numbers = [
        ("min", &b.minimum),
        ("max", &b.maximum),
        ("xmin", &b.exclusive_minimum),
        ("xmax", &b.exclusive_maximum),
        ("step", &b.multiple_of),
    ];
    for (label, n) in numbers {
        if let Some(n) = n {
            out.push(format!("{label}={n}"));
        }
    }
    let counts = [("minlen", b.min_length), ("maxlen", b.max_length)];
    for (label, n) in counts {
        if let Some(n) = n {
            out.push(format!("{label}={n}"));
        }
    }
    if let Some(p) = &b.pattern {
        out.push(format!("pattern={}", Value::from(p.source.as_str())));
    }
    let counts = [("minitems", b.min_items), ("maxitems", b.max_items)];
    for (label, n) in counts {
        if let Some(n) = n {
            out.push(format!("{label}={n}"));
        }
    }
    match b.unique_items {
        Some(true) => out.push("unique".into()),
        Some(false) => out.push("unique=false".into()),
        None => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn defs(params: Value) -> String {
        let tool = ToolDef::new("t", Some("Do it.".into()), Some(params));
        encode_tools(&[tool]).unwrap().definitions().to_string()
    }

    #[test]
    fn required_parameters_come_first_in_required_order() {
        let out = defs(json!({
            "type": "object",
            "properties": {
                "a": {"type": "string"},
                "b": {"type": "string"},
                "z": {"type": "string"},
                "m": {"type": "string"}
            },
            "required": ["z", "b"]
        }));
        assert_eq!(
            out,
            "t: Do it.\n z: string\n b: string\n a?: string\n m?: string"
        );
    }

    #[test]
    fn renders_optional_marker_enum_array_and_nested_object() {
        let out = defs(json!({
            "type": "object",
            "properties": {
                "tags": {"type": "array", "items": {"type": "string"}, "description": "Labels"},
                "level": {"type": "string", "enum": ["low", "high"]},
                "who": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}, "email": {"type": "string"}},
                    "required": ["name"]
                },
                "rows": {
                    "type": "array",
                    "items": {"type": "object", "properties": {"id": {"type": "integer"}}}
                },
                "picks": {"type": "array", "items": {"type": "string", "enum": ["x", "y"]}}
            }
        }));
        assert_eq!(
            out,
            "t: Do it.\n level?: low|high\n picks?: (x|y)[]\n rows?: object[]\n  id?: integer\n tags?: string[] # Labels\n who?: object\n  name: string\n  email?: string"
        );
    }

    #[test]
    fn renders_bounds_formats_and_object_annotations() {
        let out = defs(json!({
            "type": "object",
            "properties": {
                "n": {"type": "integer", "minimum": 1, "maximum": 100},
                "code": {"type": "string", "pattern": "^[A-Z]{3}$", "maxLength": 3},
                "when": {"type": "string", "format": "date-time"},
                "mail": {"type": "string", "format": "email"},
                "ids": {"type": "array", "items": {"type": "integer"}, "uniqueItems": true, "maxItems": 5},
                "meta": {"type": "object"},
                "opts": {"type": "object", "properties": {"x": {"type": "boolean"}}, "additionalProperties": false}
            },
            "additionalProperties": false
        }));
        assert_eq!(
            out,
            "t (closed): Do it.\n code?: string (maxlen=3, pattern=\"^[A-Z]{3}$\")\n ids?: integer[] (maxitems=5, unique)\n mail?: string<email>\n meta?: object (any)\n n?: integer (min=1, max=100)\n opts?: object (closed)\n  x?: boolean\n when?: datetime"
        );
    }

    #[test]
    fn enum_values_are_quoted_only_when_they_must_be() {
        let out = defs(json!({
            "type": "object",
            "properties": {
                "city": {"type": "string", "enum": ["New York", "LA", "string"]},
                "n": {"type": "number", "enum": [1, 2]},
                "mixed": {"enum": ["a", 1, null]}
            }
        }));
        assert_eq!(
            out,
            "t: Do it.\n city?: \"New York\"|LA|\"string\"\n mixed?: a|1|null (type=none)\n n?: 1|2 (type=number)"
        );
    }

    #[test]
    fn descriptions_escape_control_characters_and_keep_empty_distinct() {
        let out = defs(json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "description": "line one\nline two # not a comment"},
                "b": {"type": "string", "description": ""}
            }
        }));
        assert_eq!(
            out,
            "t: Do it.\n a?: string # line one\\nline two # not a comment\n b?: string #"
        );
    }

    #[test]
    fn the_common_path_contains_no_double_quotes() {
        // Every `"` costs an escape in the JSON request body the token count is taken on.
        let out = defs(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "count": {"type": "integer"},
                "emails": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        }));
        assert!(!out.contains('"'), "{out}");
    }

    #[test]
    fn defaults_and_nullable_types_render() {
        let out = defs(json!({
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "default": 10},
                "note": {"type": ["string", "null"], "maxLength": 5}
            }
        }));
        assert_eq!(
            out,
            "t: Do it.\n limit?: integer = 10\n note?: string (maxlen=5)|null"
        );
    }

    #[test]
    fn duplicate_tool_names_are_unsupported() {
        let t = ToolDef::new("same", None, None);
        let err = encode_tools(&[t.clone(), t]).unwrap_err();
        assert!(matches!(
            err,
            CompactError::Unsupported {
                feature: UnsupportedFeature::DuplicateTool,
                ..
            }
        ));
    }

    #[test]
    fn inconsistent_required_is_unsupported() {
        let tool = ToolDef::new(
            "t",
            None,
            Some(json!({"type": "object", "properties": {}, "required": ["ghost"]})),
        );
        let err = encode_tools(&[tool]).unwrap_err();
        assert!(matches!(
            err,
            CompactError::Unsupported {
                feature: UnsupportedFeature::InconsistentRequired,
                ..
            }
        ));
    }

    #[test]
    fn a_description_on_array_items_has_no_place_and_is_unsupported() {
        let tool = ToolDef::new(
            "t",
            None,
            Some(json!({"type": "object", "properties": {
                "a": {"type": "array", "items": {"type": "string", "description": "x"}}
            }})),
        );
        let err = encode_tools(&[tool]).unwrap_err();
        assert!(
            matches!(&err, CompactError::Unsupported { path, .. } if path == "/parameters/properties/a/items/description"),
            "{err:?}"
        );
    }

    #[test]
    fn encoding_is_deterministic() {
        let tools = vec![
            ToolDef::new("a", Some("A".into()), None),
            ToolDef::new(
                "b",
                None,
                Some(json!({"type": "object", "properties": {"x": {"type": "string"}}})),
            ),
        ];
        assert_eq!(encode_tools(&tools), encode_tools(&tools));
        assert_eq!(
            encode_tools(&tools).unwrap().definitions(),
            "a (noparams): A\nb:\n x?: string"
        );
    }
}

//! Parsing compact definitions back into JSON Schema.
//!
//! This reads the text only — the same text a model is shown — so a successful round trip is
//! evidence that the schema survived compaction, not an artefact of carrying the original along.

use serde_json::{Map, Number, Value};

use crate::error::CompactError;
use crate::grammar::{format_for_alias, is_word_char, unescape_description};
use crate::models::{CompactTools, ToolDef};
use crate::schema::{
    Additional, Kind, Node, ObjectShape, Pattern, Prop, empty_object, implied_kinds, to_json,
};

/// Parse compact definitions back into tool definitions.
///
/// The result is the canonical form of the original schemas: identical except that
/// `required: []` is omitted and a tool without `parameters` reads back as
/// `{"type":"object","properties":{}}`.
///
/// ```
/// use nasiko_tool_compact::{decode_tools, encode_tools, ToolDef};
/// use serde_json::json;
///
/// let params = json!({
///     "type": "object",
///     "properties": {"q": {"type": "string", "description": "Query"}},
///     "required": ["q"]
/// });
/// let tool = ToolDef::new("search", Some("Search the web.".into()), Some(params.clone()));
/// let back = decode_tools(&encode_tools(&[tool]).unwrap()).unwrap();
/// assert_eq!(back[0].name, "search");
/// assert_eq!(back[0].parameters.as_ref(), Some(&params));
/// ```
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let text = compact.definitions();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut tools = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line_no = index + 1;
        let header = parse_header(lines[index]).map_err(|r| invalid(line_no, r))?;
        index += 1;
        let mut root = empty_object();
        let shape = root.object.get_or_insert_with(Default::default);
        let mut strict = None;
        let mut parameters_absent = false;
        for annotation in header.annotations {
            match annotation {
                Annotation::Any => shape.properties = None,
                Annotation::NoRequired => shape.required = Some(Vec::new()),
                Annotation::NoParams => parameters_absent = true,
                Annotation::Closed => shape.additional = Additional::Allowed(false),
                Annotation::Open => shape.additional = Additional::Allowed(true),
                Annotation::Strict(s) => strict = Some(s),
                other => return Err(invalid(line_no, format!("`{other:?}` on a tool header"))),
            }
        }
        let (props, required) = parse_properties(&lines, &mut index, 1)?;
        if shape.properties.is_some() {
            shape.properties = Some(props);
        } else if !props.is_empty() {
            return Err(invalid(
                line_no,
                "a free-form object cannot list properties",
            ));
        }
        if !required.is_empty() {
            shape.required = Some(required);
        }
        if parameters_absent && root != empty_object() {
            return Err(invalid(line_no, "`noparams` on a tool with parameters"));
        }
        tools.push(ToolDef {
            name: header.name,
            description: header.description,
            parameters: (!parameters_absent).then(|| to_json(&root)),
            strict,
            extra: Map::new(),
        });
    }
    Ok(tools)
}

fn invalid(line: usize, reason: impl Into<String>) -> CompactError {
    CompactError::InvalidDefinitions {
        line,
        reason: reason.into(),
    }
}

struct Header {
    name: String,
    annotations: Vec<Annotation>,
    description: Option<String>,
}

fn parse_header(line: &str) -> Result<Header, String> {
    let mut cur = Cursor::new(line);
    let name = cur.take_while(|c| c != ' ' && c != ':');
    if name.is_empty() {
        return Err("expected a tool name".into());
    }
    let annotations = if cur.eat(" (") {
        cur.annotations()?
    } else {
        Vec::new()
    };
    if !cur.eat(":") {
        return Err("expected `:` after the tool name".into());
    }
    let description = if cur.at_end() {
        None
    } else if cur.eat(" ") {
        Some(unescape_description(cur.rest())?)
    } else {
        return Err("expected a space before the description".into());
    };
    Ok(Header {
        name: name.to_string(),
        annotations,
        description,
    })
}

/// Parse the property lines at `depth`, recursing into deeper lines for object children.
/// Returns the properties in text order and the names of the required ones, in text order.
fn parse_properties(
    lines: &[&str],
    index: &mut usize,
    depth: usize,
) -> Result<(Vec<Prop>, Vec<String>), CompactError> {
    let mut props = Vec::new();
    let mut required = Vec::new();
    while let Some(line) = lines.get(*index) {
        let indent = indent_of(line);
        if indent < depth {
            break;
        }
        let line_no = *index + 1;
        if indent > depth {
            return Err(invalid(line_no, "unexpected indentation"));
        }
        let (name, optional, mut node) =
            parse_property(line.trim_start_matches(' ')).map_err(|r| invalid(line_no, r))?;
        *index += 1;
        if lines.get(*index).is_some_and(|l| indent_of(l) == depth + 1) {
            let shape = object_spine(&mut node)
                .ok_or_else(|| invalid(line_no, "child lines under a non-object type"))?;
            if shape.properties.is_none() {
                return Err(invalid(
                    line_no,
                    "a free-form object cannot list properties",
                ));
            }
            let (children, child_required) = parse_properties(lines, index, depth + 1)?;
            shape.properties = Some(children);
            if !child_required.is_empty() {
                shape.required = Some(child_required);
            }
        }
        if !optional {
            required.push(name.clone());
        }
        if props.iter().any(|p: &Prop| p.name == name) {
            return Err(invalid(
                line_no,
                format!("property `{name}` declared twice"),
            ));
        }
        props.push(Prop { name, node });
    }
    Ok((props, required))
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// The object whose properties a property's child lines describe: the property's own object,
/// or the innermost object of an array of objects.
fn object_spine(node: &mut Node) -> Option<&mut ObjectShape> {
    if node.types.contains(&Kind::Object) {
        return node.object.as_mut();
    }
    match node.items.as_deref_mut() {
        Some(item) => object_spine(item),
        None => None,
    }
}

fn parse_property(text: &str) -> Result<(String, bool, Node), String> {
    let mut cur = Cursor::new(text);
    let name = cur.take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if name.is_empty() {
        return Err("expected a property name".into());
    }
    let optional = cur.eat("?");
    if !cur.eat(": ") {
        return Err("expected `: ` after the property name".into());
    }
    let terms = cur.union()?;
    let mut node = union_to_node(terms)?;
    if cur.eat(" = ") {
        node.default = Some(cur.json_value()?);
    }
    if cur.eat(" #") {
        let description = cur.rest();
        let description = description.strip_prefix(' ').unwrap_or(description);
        node.description = Some(unescape_description(description)?);
        cur.finish();
    }
    if !cur.at_end() {
        return Err(format!("unexpected text `{}`", cur.rest()));
    }
    Ok((name.to_string(), optional, node))
}

#[derive(Debug)]
enum Atom {
    Word(String),
    Format(String, String),
    Literal(Value),
    Group(Vec<Term>),
}

#[derive(Debug)]
struct Term {
    atom: Atom,
    annotations: Vec<Annotation>,
    /// One entry per `[]`, holding that array's annotations.
    arrays: Vec<Vec<Annotation>>,
}

impl Term {
    /// A term that can only be an enum value: a quoted or numeric literal, or a bare word that
    /// is not a type keyword. `null` is ambiguous and decided by its neighbours.
    fn is_value(&self) -> bool {
        self.arrays.is_empty()
            && match &self.atom {
                Atom::Literal(_) => true,
                Atom::Word(w) => {
                    w == "true"
                        || w == "false"
                        || (Kind::from_keyword(w).is_none() && !is_type_word(w))
                }
                _ => false,
            }
    }
}

fn is_type_word(word: &str) -> bool {
    word == "any" || format_for_alias(word).is_some()
}

#[derive(Debug, Clone, PartialEq)]
enum Annotation {
    Number(&'static str, Number),
    Count(&'static str, u64),
    Pattern(String),
    Unique(bool),
    Any,
    Closed,
    Open,
    NoRequired,
    NoParams,
    Const,
    Type(Vec<Kind>),
    Strict(bool),
}

fn union_to_node(terms: Vec<Term>) -> Result<Node, String> {
    let literal_mode = terms.iter().any(|t| {
        t.is_value()
            || t.annotations
                .iter()
                .any(|a| matches!(a, Annotation::Const | Annotation::Type(_)))
    });
    if literal_mode {
        return literals_to_node(terms);
    }
    let mut nodes = terms
        .into_iter()
        .map(term_to_node)
        .collect::<Result<Vec<_>, _>>()?;
    if nodes.len() == 1 {
        return nodes.pop().ok_or_else(|| "empty type".to_string());
    }
    // A type union: each term contributes one kind; the keywords come from the one term that
    // carries any.
    let mut kinds = Vec::with_capacity(nodes.len());
    let mut carrier: Option<Node> = None;
    for node in nodes {
        let [kind] = node.types.as_slice() else {
            return Err("each union member must be a single type".into());
        };
        kinds.push(*kind);
        if node != bare(*kind) {
            if carrier.is_some() {
                return Err("keywords on more than one union member".into());
            }
            carrier = Some(node);
        }
    }
    let mut merged = carrier.unwrap_or_default();
    if kinds.contains(&Kind::Object) && merged.object.is_none() {
        // A bare `object` member: properties may follow as child lines.
        merged.object = empty_object().object;
    }
    merged.types = kinds;
    Ok(merged)
}

/// The node a keyword alone produces, with no other keywords.
fn bare(kind: Kind) -> Node {
    match kind {
        Kind::Object => empty_object(),
        other => Node {
            types: vec![other],
            ..Default::default()
        },
    }
}

fn literals_to_node(terms: Vec<Term>) -> Result<Node, String> {
    let mut node = Node::default();
    let mut values = Vec::with_capacity(terms.len());
    let mut is_const = false;
    let mut declared = None;
    for term in terms {
        if !term.arrays.is_empty() {
            return Err("`[]` on an enum value".into());
        }
        let value = match term.atom {
            Atom::Literal(v) => v,
            Atom::Word(w) if w == "true" => Value::Bool(true),
            Atom::Word(w) if w == "false" => Value::Bool(false),
            Atom::Word(w) if w == "null" => Value::Null,
            Atom::Word(w) if Kind::from_keyword(&w).is_none() && !is_type_word(&w) => {
                Value::String(w)
            }
            other => return Err(format!("`{other:?}` cannot be an enum value")),
        };
        values.push(value);
        for annotation in term.annotations {
            match annotation {
                Annotation::Const => is_const = true,
                Annotation::Type(kinds) => declared = Some(kinds),
                other => apply(&mut node, other)?,
            }
        }
    }
    node.types = declared.unwrap_or_else(|| implied_kinds(&values));
    if is_const {
        let [value] = <[Value; 1]>::try_from(values).map_err(|_| "`const` with several values")?;
        node.const_value = Some(value);
    } else {
        node.enum_values = Some(values);
    }
    Ok(node)
}

fn term_to_node(term: Term) -> Result<Node, String> {
    let mut node = match term.atom {
        Atom::Group(inner) => union_to_node(inner)?,
        Atom::Word(word) => word_to_node(&word, None)?,
        Atom::Format(word, format) => word_to_node(&word, Some(format))?,
        Atom::Literal(v) => return Err(format!("unexpected literal `{v}`")),
    };
    for annotation in term.annotations {
        apply(&mut node, annotation)?;
    }
    for annotations in term.arrays {
        node = Node {
            types: vec![Kind::Array],
            items: Some(Box::new(node)),
            ..Default::default()
        };
        for annotation in annotations {
            apply(&mut node, annotation)?;
        }
    }
    Ok(node)
}

fn word_to_node(word: &str, format: Option<String>) -> Result<Node, String> {
    if let Some(aliased) = format_for_alias(word) {
        if format.is_some() {
            return Err(format!("`{word}` cannot take a format"));
        }
        return Ok(Node {
            types: vec![Kind::String],
            format: Some(aliased.to_string()),
            ..Default::default()
        });
    }
    if word == "any" {
        return match format {
            None => Ok(Node::default()),
            Some(_) => Err("`any` cannot take a format".into()),
        };
    }
    let kind = Kind::from_keyword(word).ok_or_else(|| format!("unknown type `{word}`"))?;
    let mut node = bare(kind);
    node.format = format;
    Ok(node)
}

fn apply(node: &mut Node, annotation: Annotation) -> Result<(), String> {
    let b = &mut node.bounds;
    match annotation {
        Annotation::Number(label, n) => {
            let slot = match label {
                "min" => &mut b.minimum,
                "max" => &mut b.maximum,
                "xmin" => &mut b.exclusive_minimum,
                "xmax" => &mut b.exclusive_maximum,
                _ => &mut b.multiple_of,
            };
            *slot = Some(n);
        }
        Annotation::Count(label, n) => {
            let slot = match label {
                "minlen" => &mut b.min_length,
                "maxlen" => &mut b.max_length,
                "minitems" => &mut b.min_items,
                _ => &mut b.max_items,
            };
            *slot = Some(n);
        }
        Annotation::Pattern(source) => {
            b.pattern =
                Some(Pattern::compile(&source).ok_or_else(|| format!("bad pattern `{source}`"))?);
        }
        Annotation::Unique(u) => b.unique_items = Some(u),
        Annotation::Any | Annotation::Closed | Annotation::Open | Annotation::NoRequired => {
            let shape = node
                .object
                .as_mut()
                .ok_or("object annotation on a non-object")?;
            match annotation {
                Annotation::Any => shape.properties = None,
                Annotation::Closed => shape.additional = Additional::Allowed(false),
                Annotation::NoRequired => shape.required = Some(Vec::new()),
                _ => shape.additional = Additional::Allowed(true),
            }
        }
        Annotation::Const | Annotation::Type(_) | Annotation::Strict(_) | Annotation::NoParams => {
            return Err(format!("`{annotation:?}` is not valid here"));
        }
    }
    Ok(())
}

/// A cursor over one line. Positions only ever advance by whole characters, so every slice it
/// takes is on a character boundary.
struct Cursor<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or_default()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.text.len()
    }

    fn finish(&mut self) {
        self.pos = self.text.len();
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn eat(&mut self, token: &str) -> bool {
        if self.rest().starts_with(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    fn take_while(&mut self, keep: impl Fn(char) -> bool) -> &'a str {
        let rest = self.rest();
        let len: usize = rest
            .chars()
            .take_while(|c| keep(*c))
            .map(char::len_utf8)
            .sum();
        self.pos += len;
        rest.get(..len).unwrap_or_default()
    }

    fn union(&mut self) -> Result<Vec<Term>, String> {
        let mut terms = vec![self.term()?];
        while self.eat("|") {
            terms.push(self.term()?);
        }
        Ok(terms)
    }

    fn term(&mut self) -> Result<Term, String> {
        let atom = self.atom()?;
        let annotations = if self.eat(" (") {
            self.annotations()?
        } else {
            Vec::new()
        };
        let mut arrays = Vec::new();
        while self.eat("[]") {
            arrays.push(if self.eat(" (") {
                self.annotations()?
            } else {
                Vec::new()
            });
        }
        Ok(Term {
            atom,
            annotations,
            arrays,
        })
    }

    fn atom(&mut self) -> Result<Atom, String> {
        match self.peek() {
            Some('(') => {
                self.eat("(");
                let inner = self.union()?;
                if !self.eat(")") {
                    return Err("unclosed `(`".into());
                }
                Ok(Atom::Group(inner))
            }
            Some('"') => Ok(Atom::Literal(Value::String(self.json_string()?))),
            Some(c) if c == '-' || c.is_ascii_digit() => {
                Ok(Atom::Literal(Value::Number(self.json_number()?)))
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let word = self.take_while(is_word_char).to_string();
                if self.eat("<") {
                    let format = self.take_while(|c| c != '>').to_string();
                    if !self.eat(">") {
                        return Err("unclosed `<`".into());
                    }
                    Ok(Atom::Format(word, format))
                } else {
                    Ok(Atom::Word(word))
                }
            }
            other => Err(format!("unexpected `{}` in a type", other.unwrap_or(' '))),
        }
    }

    /// Annotations after ` (`: `name` or `name=value`, comma separated, closed by `)`.
    fn annotations(&mut self) -> Result<Vec<Annotation>, String> {
        let mut out = Vec::new();
        loop {
            let name = self.take_while(|c| c.is_ascii_lowercase());
            let annotation = if self.eat("=") {
                self.annotation_value(name)?
            } else {
                match name {
                    "any" => Annotation::Any,
                    "closed" => Annotation::Closed,
                    "open" => Annotation::Open,
                    "noreq" => Annotation::NoRequired,
                    "noparams" => Annotation::NoParams,
                    "const" => Annotation::Const,
                    "unique" => Annotation::Unique(true),
                    "strict" => Annotation::Strict(true),
                    "nonstrict" => Annotation::Strict(false),
                    other => return Err(format!("unknown annotation `{other}`")),
                }
            };
            out.push(annotation);
            if self.eat(")") {
                return Ok(out);
            }
            if !self.eat(", ") {
                return Err("expected `, ` or `)` in annotations".into());
            }
        }
    }

    fn annotation_value(&mut self, name: &str) -> Result<Annotation, String> {
        let number_label = ["min", "max", "xmin", "xmax", "step"]
            .into_iter()
            .find(|l| *l == name);
        if let Some(label) = number_label {
            return Ok(Annotation::Number(label, self.json_number()?));
        }
        let count_label = ["minlen", "maxlen", "minitems", "maxitems"]
            .into_iter()
            .find(|l| *l == name);
        if let Some(label) = count_label {
            let n = self.json_number()?;
            let n = n
                .as_u64()
                .ok_or_else(|| format!("`{name}` needs a count"))?;
            return Ok(Annotation::Count(label, n));
        }
        match name {
            "pattern" => Ok(Annotation::Pattern(self.json_string()?)),
            "unique" if self.eat("false") => Ok(Annotation::Unique(false)),
            "type" => {
                let label = self.take_while(|c| c != ',' && c != ')');
                if label == "none" {
                    return Ok(Annotation::Type(Vec::new()));
                }
                label
                    .split('|')
                    .map(|w| Kind::from_keyword(w).ok_or_else(|| format!("unknown type `{w}`")))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Annotation::Type)
            }
            other => Err(format!("unknown annotation `{other}=`")),
        }
    }

    /// A JSON string starting at the cursor. The scan finds the closing quote (honouring
    /// escapes); `serde_json` does the decoding.
    fn json_string(&mut self) -> Result<String, String> {
        let rest = self.rest();
        let mut escaped = false;
        let mut end = None;
        for (i, c) in rest.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                end = Some(i + 1);
                break;
            }
        }
        let end = end.ok_or("unterminated string")?;
        let raw = rest.get(..end).unwrap_or_default();
        self.pos += end;
        serde_json::from_str(raw).map_err(|e| e.to_string())
    }

    fn json_number(&mut self) -> Result<Number, String> {
        let raw =
            self.take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'));
        serde_json::from_str(raw).map_err(|_| format!("bad number `{raw}`"))
    }

    /// Any JSON value (a default), ended by the end of the line or ` #`.
    fn json_value(&mut self) -> Result<Value, String> {
        let rest = self.rest();
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                self.pos += stream.byte_offset();
                Ok(value)
            }
            _ => Err("bad default value".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode_tools;
    use crate::schema::canonical_parameters;
    use serde_json::json;

    fn roundtrip(params: Value) {
        let tool = ToolDef::new("t", Some("Do it.".into()), Some(params));
        let compact = encode_tools(std::slice::from_ref(&tool)).unwrap();
        let back = decode_tools(&compact).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, tool.name);
        assert_eq!(back[0].description, tool.description);
        assert_eq!(
            back[0].parameters.as_ref(),
            Some(&canonical_parameters(&tool)),
            "definitions were:\n{}",
            compact.definitions()
        );
    }

    #[test]
    fn roundtrip_equals_canonical_for_the_fixture_suite() {
        let fixtures = [
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}),
            json!({"type": "object", "properties": {
                "who": {"type": "object", "properties": {
                    "name": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}}
                }, "required": ["name"], "additionalProperties": false}
            }}),
            json!({"type": "object", "properties": {
                "rows": {"type": "array", "items": {"type": "object", "properties": {
                    "id": {"type": "integer"}, "v": {"type": ["number", "null"]}
                }, "required": ["id"]}}
            }}),
            json!({"type": "object", "properties": {
                "grid": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}}
            }}),
            json!({"type": "object", "properties": {
                "mixed": {"enum": ["a", 1, true, null, "two words"]},
                "num": {"type": "number", "enum": [1, 2]},
                "untyped": {"enum": ["x", "y"]},
                "one": {"const": "fixed"},
                "typed_one": {"type": "string", "const": "fixed"}
            }}),
            json!({"type": "object", "properties": {
                "n": {"type": "integer", "minimum": -5, "exclusiveMaximum": 9.5, "multipleOf": 0.5},
                "s": {"type": "string", "minLength": 1, "maxLength": 64, "pattern": "^a(b|c)\\d+$"},
                "l": {"type": "array", "items": {"type": "string", "maxLength": 3}, "minItems": 1, "uniqueItems": false}
            }}),
            json!({"type": "object", "properties": {
                "meta": {"type": "object"},
                "empty": {"type": "object", "properties": {}},
                "open": {"type": "object", "properties": {"x": {"type": "string"}}, "additionalProperties": true}
            }, "additionalProperties": false}),
            json!({"type": "object"}),
            json!({"type": "object", "properties": {
                "limit": {"type": "integer", "default": 10, "description": "Max rows"},
                "mode": {"type": "string", "enum": ["a", "b"], "default": "a"},
                "when": {"type": "string", "format": "date-time"},
                "mail": {"type": "string", "format": "email"},
                "big": {"type": "integer", "format": "int64"},
                "anything": {},
                "pick": {"type": "array", "items": {"enum": ["x y", "z"]}}
            }, "required": ["when", "limit"]}),
            // Found by review: these forms must read back exactly.
            json!({"type": "object", "properties": {
                "n": {"const": null},
                "tn": {"type": "null", "const": null},
                "obj": {"type": "object", "properties": {"a": {"type": "string"}}, "required": []}
            }, "required": []}),
            // Found by property testing: a one-value enum as array items.
            json!({"type": "object", "properties": {
                "one": {"type": "array", "items": {"type": "string", "enum": ["a"]}},
                "fixed": {"type": "array", "items": {"const": 3}}
            }}),
            json!({"type": "object", "properties": {
                "maybe_list": {"type": ["array", "null"], "items": {"type": "string"}},
                "maybe_obj": {"type": ["object", "null"], "properties": {"k": {"type": "string"}}}
            }}),
        ];
        for params in fixtures {
            roundtrip(params);
        }
    }

    #[test]
    fn description_with_hash_backslash_crlf_emoji_roundtrips() {
        roundtrip(json!({"type": "object", "properties": {
            "a": {"type": "string", "description": "a # b \\ c\r\nd 😀 = x"},
            "b": {"type": "string", "description": ""},
            "c": {"type": "string", "description": " leading space"}
        }}));
        let tool = ToolDef::new("t", Some("Multi\nline # desc".into()), None);
        let back = decode_tools(&encode_tools(&[tool]).unwrap()).unwrap();
        assert_eq!(back[0].description.as_deref(), Some("Multi\nline # desc"));
    }

    #[test]
    fn quoted_enum_values_with_grammar_characters_roundtrip() {
        roundtrip(json!({"type": "object", "properties": {
            "a": {"type": "string", "enum": ["x # y", "p|q", "(r)", "s, t", "string", "1"]}
        }}));
    }

    #[test]
    fn tool_annotations_and_empty_descriptions_roundtrip() {
        let mut strict = ToolDef::new(
            "s",
            Some(String::new()),
            Some(
                json!({"type": "object", "properties": {"x": {"type": "string"}},
                        "required": ["x"], "additionalProperties": false}),
            ),
        );
        strict.strict = Some(true);
        let back = decode_tools(&encode_tools(&[strict.clone()]).unwrap()).unwrap();
        assert_eq!(back[0].strict, Some(true));
        assert_eq!(back[0].description.as_deref(), Some(""));
        assert_eq!(back[0].parameters, strict.parameters);
    }

    #[test]
    fn several_tools_roundtrip_in_order() {
        let tools = vec![
            ToolDef::new("a", None, None),
            ToolDef::new(
                "b",
                Some("B".into()),
                Some(json!({"type": "object", "properties": {"x": {"type": "string"}}})),
            ),
        ];
        let back = decode_tools(&encode_tools(&tools).unwrap()).unwrap();
        let names: Vec<_> = back.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(back[0].parameters, None, "absent parameters stay absent");
    }

    #[test]
    fn malformed_definitions_report_the_line() {
        for (text, line) in [
            ("t:\n x: (string", 2),
            ("t:\n x: string<email", 2),
            ("t:\n x: string|1", 2),
            ("t:\n x: string\n   y: string", 3),
            ("t:\n x string", 2),
            (":nope", 1),
            ("t:\n x: string (bogus=1)", 2),
            ("t:\n x: \"unterminated", 2),
        ] {
            let err = decode_tools(&CompactTools::from_definitions(text)).unwrap_err();
            assert!(
                matches!(err, CompactError::InvalidDefinitions { line: l, .. } if l == line),
                "{text:?} gave {err:?}"
            );
        }
    }
}

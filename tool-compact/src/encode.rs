//! Native tool schemas -> compact signatures (and back, for `decode_tools`).
//!
//! ```text
//! Tools (? = optional):
//! create_event(title:str, start:datetime, duration_min?:int, visibility?:"public"|"private")
//! |Create an event. start: Start time, ISO 8601
//! To call tools, write <<call name {"arg":value}>> for each call needed (several allowed).
//! ```

use serde_json::{Map, Value, json};

use crate::{Error, Result, ToolDef, is_name_char};

const HEADER: &str = "Tools (? = optional):";
const FOOTER: &str =
    "To call tools, write <<call name {\"arg\":value}>> for each call needed (several allowed).";
const MAX_DEPTH: usize = 6;
/// An argument description is kept when it has at least this many words not already implied
/// by the argument's name (so `"Start time, ISO 8601"` survives but `"The title"` does not).
const MIN_EXTRA_WORDS: usize = 2;
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "to", "for", "is", "and", "or", "this", "that", "with", "be",
];
const ALLOWED_KEYS: &[&str] = &[
    "type",
    "description",
    "enum",
    "format",
    "items",
    "properties",
    "required",
    "title",
    "additionalProperties",
    "$schema",
];

/// The compact tool block to inject into the prompt instead of native `tools`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    pub prompt: String,
}

/// Renders `tools` compactly. `Err(Error::Bypass)` means "send the native tools instead":
/// an unsupported schema feature, or a result that would not be smaller.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    if tools.is_empty() {
        return Err(Error::Bypass("no tools".into()));
    }
    let mut seen = std::collections::HashSet::new();
    let mut prompt = format!("{HEADER}\n");
    for tool in tools {
        check_tool(tool)?;
        if !seen.insert(tool.name.as_str()) {
            return Err(Error::Bypass(format!("duplicate tool `{}`", tool.name)));
        }
        let root = tool.parameters.clone().unwrap_or(Value::Null);
        prompt.push_str(&format!("{}({})\n", tool.name, render_props(&root)));
        if let Some(line) = description_line(tool, &root) {
            prompt.push_str(&format!("|{line}\n"));
        }
    }
    prompt.push_str(FOOTER);
    let native: usize = tools.iter().map(native_len).sum();
    if prompt.len() >= native {
        return Err(Error::Bypass("compact form is not smaller".into()));
    }
    Ok(CompactTools { prompt })
}

/// Confirms a tool only uses schema features this format preserves.
pub(crate) fn check_tool(tool: &ToolDef) -> Result<()> {
    let bypass = |why: String| Err(Error::Bypass(format!("tool `{}`: {why}", tool.name)));
    if tool.name.is_empty() || !tool.name.chars().all(is_name_char) {
        return bypass("name has characters outside [A-Za-z0-9_.-]".into());
    }
    match &tool.parameters {
        None => Ok(()),
        Some(schema) => check_schema(schema, 0, true).or_else(bypass),
    }
}

fn check_schema(s: &Value, depth: usize, root: bool) -> std::result::Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("schema nested too deeply".into());
    }
    let o = s.as_object().ok_or("schema is not an object")?;
    if let Some(k) = o.keys().find(|k| !ALLOWED_KEYS.contains(&k.as_str())) {
        return Err(format!("unsupported keyword `{k}`"));
    }
    if o.get("additionalProperties")
        .is_some_and(|v| v != &Value::Bool(false))
    {
        return Err("additionalProperties must be false when present".into());
    }
    let has_enum = match o.get("enum") {
        None => false,
        Some(Value::Array(a))
            if !a.is_empty()
                && a.iter()
                    .all(|v| v.is_string() || v.is_number() || v.is_boolean()) =>
        {
            true
        }
        Some(_) => return Err("enum must be a non-empty list of scalars".into()),
    };
    let ty = o.get("type").and_then(Value::as_str);
    match ty {
        Some("string" | "integer" | "number" | "boolean" | "array" | "object") => {}
        Some(t) => return Err(format!("unsupported type `{t}`")),
        None if o.contains_key("type") => return Err("`type` must be a single string".into()),
        None if has_enum => {}
        None => return Err("missing `type`".into()),
    }
    if root && ty != Some("object") {
        return Err("root schema must be type object".into());
    }
    if let Some(f) = o.get("format")
        && (ty != Some("string") || !matches!(f.as_str(), Some("date-time" | "date" | "email")))
    {
        return Err(format!("unsupported format {f}"));
    }
    if o.contains_key("items") && ty != Some("array") {
        return Err("`items` on a non-array".into());
    }
    if (o.contains_key("properties") || o.contains_key("required")) && ty != Some("object") {
        return Err("`properties`/`required` on a non-object".into());
    }
    if ty == Some("array") {
        check_schema(
            o.get("items").ok_or("array without `items`")?,
            depth + 1,
            false,
        )?;
    }
    if ty == Some("object") {
        let empty = Map::new();
        let props = match o.get("properties") {
            Some(p) => p.as_object().ok_or("`properties` must be an object")?,
            None => &empty,
        };
        if props.is_empty() && !root {
            return Err("free-form object (no properties)".into());
        }
        for (name, sub) in props {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!("property name `{name}` is not [A-Za-z0-9_]+"));
            }
            check_schema(sub, depth + 1, false)?;
        }
        let none = vec![];
        let required = match o.get("required") {
            Some(r) => r.as_array().ok_or("`required` must be a list")?,
            None => &none,
        };
        for r in required {
            match r.as_str() {
                Some(r) if props.contains_key(r) => {}
                _ => return Err(format!("`required` names an unknown property {r}")),
            }
        }
    }
    Ok(())
}

/// Properties with required ones first (in `required` order), then the rest, so output is
/// deterministic regardless of map ordering.
fn ordered_props(s: &Value) -> Vec<(&str, &Value, bool)> {
    let Some(props) = s.get("properties").and_then(Value::as_object) else {
        return vec![];
    };
    let required: Vec<&str> = s
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out: Vec<(&str, &Value, bool)> = vec![];
    for r in &required {
        if let Some(v) = props.get(*r)
            && !out.iter().any(|(n, ..)| n == r)
        {
            out.push((r, v, true));
        }
    }
    out.extend(
        props
            .iter()
            .filter(|(k, _)| !required.contains(&k.as_str()))
            .map(|(k, v)| (k.as_str(), v, false)),
    );
    out
}

fn render_props(s: &Value) -> String {
    ordered_props(s)
        .iter()
        .map(|(name, v, req)| format!("{name}{}:{}", if *req { "" } else { "?" }, render_type(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_type(s: &Value) -> String {
    if let Some(Value::Array(values)) = s.get("enum") {
        return values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("|");
    }
    match s.get("type").and_then(Value::as_str) {
        Some("string") => match s.get("format").and_then(Value::as_str) {
            Some("date-time") => "datetime",
            Some("date") => "date",
            Some("email") => "email",
            _ => "str",
        }
        .into(),
        Some("integer") => "int".into(),
        Some("number") => "num".into(),
        Some("boolean") => "bool".into(),
        Some("array") => format!("[{}]", render_type(&s["items"])),
        Some("object") => format!("{{{}}}", render_props(s)),
        _ => "any".into(),
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stem(w: &str) -> String {
    let w = w.to_lowercase();
    if w.len() > 3 {
        w.strip_suffix('s').unwrap_or(&w).to_string()
    } else {
        w
    }
}

/// True when `desc` says something the argument name does not already say.
fn informative(name: &str, desc: &str) -> bool {
    let words = |t: &str| -> Vec<String> {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(stem)
            .collect()
    };
    let known = words(name);
    words(desc)
        .iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()) && !known.contains(w))
        .count()
        >= MIN_EXTRA_WORDS
}

fn collect_notes(s: &Value, path: &str, out: &mut Vec<String>) {
    for (name, v, _) in ordered_props(s) {
        let here = if path.is_empty() {
            name.to_string()
        } else {
            format!("{path}.{name}")
        };
        if let Some(d) = v.get("description").and_then(Value::as_str)
            && informative(name, d)
        {
            out.push(format!("{here}: {}", collapse(d)));
        }
        match v.get("type").and_then(Value::as_str) {
            Some("object") => collect_notes(v, &here, out),
            Some("array") if v["items"].get("type").and_then(Value::as_str) == Some("object") => {
                collect_notes(&v["items"], &format!("{here}[]"), out)
            }
            _ => {}
        }
    }
}

fn description_line(tool: &ToolDef, root: &Value) -> Option<String> {
    let mut notes = vec![];
    collect_notes(root, "", &mut notes);
    let desc = tool
        .description
        .as_deref()
        .map(collapse)
        .filter(|d| !d.is_empty());
    match (desc, notes.is_empty()) {
        (None, true) => None,
        (Some(d), true) => Some(d),
        (None, false) => Some(notes.join("; ")),
        (Some(d), false) => Some(format!("{d} {}", notes.join("; "))),
    }
}

fn native_len(tool: &ToolDef) -> usize {
    let mut function = json!({ "name": tool.name });
    if let Some(d) = &tool.description {
        function["description"] = json!(d);
    }
    if let Some(p) = &tool.parameters {
        function["parameters"] = p.clone();
    }
    json!({ "type": "function", "function": function })
        .to_string()
        .len()
}

// ─── decode_tools: compact prompt -> tool definitions ───────────────────────────────────────

/// Parses a [`CompactTools`] prompt back into definitions, so schema information (names,
/// required vs optional, types, enums, nesting) can be checked as surviving the round trip.
/// Descriptions come back as the raw `|` line (argument notes included).
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let bad = |line: &str| Error::Malformed(format!("cannot parse tool line `{line}`"));
    let mut tools: Vec<ToolDef> = vec![];
    for line in compact
        .prompt
        .lines()
        .filter(|l| !l.is_empty() && *l != HEADER && *l != FOOTER)
    {
        if let Some(desc) = line.strip_prefix('|') {
            tools.last_mut().ok_or_else(|| bad(line))?.description = Some(desc.to_string());
            continue;
        }
        let mut p = Parser { s: line };
        let name = p.ident().ok_or_else(|| bad(line))?.to_string();
        let params = p
            .eat('(')
            .then(|| p.props(')'))
            .flatten()
            .filter(|_| p.s.is_empty());
        tools.push(ToolDef {
            name,
            description: None,
            parameters: Some(params.ok_or_else(|| bad(line))?),
        });
    }
    Ok(tools)
}

struct Parser<'a> {
    s: &'a str,
}

enum Atom {
    Lit(Value),
    Schema(Value),
}

impl<'a> Parser<'a> {
    fn eat(&mut self, c: char) -> bool {
        match self.s.strip_prefix(c) {
            Some(rest) => {
                self.s = rest;
                true
            }
            None => false,
        }
    }

    fn ident(&mut self) -> Option<&'a str> {
        let n = self
            .s
            .find(|c: char| !is_name_char(c))
            .unwrap_or(self.s.len());
        let (name, rest) = self.s.split_at(n);
        self.s = rest;
        (!name.is_empty()).then_some(name)
    }

    /// `name:T, other?:T` up to and including `close`.
    fn props(&mut self, close: char) -> Option<Value> {
        let mut props = Map::new();
        let mut required = vec![];
        if !self.eat(close) {
            loop {
                let name = self.ident()?;
                let optional = self.eat('?');
                self.eat(':').then_some(())?;
                props.insert(name.to_string(), self.ty()?);
                if !optional {
                    required.push(json!(name));
                }
                if self.eat(',') {
                    self.s = self.s.trim_start_matches(' ');
                    continue;
                }
                self.eat(close).then_some(())?;
                break;
            }
        }
        let mut obj = json!({ "type": "object", "properties": props });
        if !required.is_empty() {
            obj["required"] = Value::Array(required);
        }
        Some(obj)
    }

    /// One type, or a `|`-separated list of literals (an enum).
    fn ty(&mut self) -> Option<Value> {
        let mut atoms = vec![self.atom()?];
        while self.eat('|') {
            atoms.push(self.atom()?);
        }
        if atoms.iter().all(|a| matches!(a, Atom::Lit(_))) {
            let values: Vec<Value> = atoms
                .into_iter()
                .filter_map(|a| if let Atom::Lit(v) = a { Some(v) } else { None })
                .collect();
            let kind = |v: &Value| match v {
                Value::String(_) => "string",
                Value::Bool(_) => "boolean",
                n if n.is_i64() || n.is_u64() => "integer",
                _ => "number",
            };
            let mut schema = json!({ "enum": values });
            if values.iter().all(|v| kind(v) == kind(&values[0])) {
                schema["type"] = json!(kind(&values[0]));
            }
            return Some(schema);
        }
        match (atoms.pop()?, atoms.is_empty()) {
            (Atom::Schema(s), true) => Some(s),
            _ => None,
        }
    }

    fn atom(&mut self) -> Option<Atom> {
        if self.eat('[') {
            let items = self.ty()?;
            self.eat(']').then_some(())?;
            return Some(Atom::Schema(json!({ "type": "array", "items": items })));
        }
        if self.eat('{') {
            return self.props('}').map(Atom::Schema);
        }
        let first = self.s.chars().next()?;
        if first == '"'
            || first == '-'
            || first.is_ascii_digit()
            || self.s.starts_with("true")
            || self.s.starts_with("false")
        {
            let mut it = serde_json::Deserializer::from_str(self.s).into_iter::<Value>();
            let v = it.next()?.ok()?;
            self.s = &self.s[it.byte_offset()..];
            return Some(Atom::Lit(v));
        }
        let schema = match self.ident()? {
            "str" => json!({ "type": "string" }),
            "int" => json!({ "type": "integer" }),
            "num" => json!({ "type": "number" }),
            "bool" => json!({ "type": "boolean" }),
            "datetime" => json!({ "type": "string", "format": "date-time" }),
            "date" => json!({ "type": "string", "format": "date" }),
            "email" => json!({ "type": "string", "format": "email" }),
            _ => return None,
        };
        Some(Atom::Schema(schema))
    }
}

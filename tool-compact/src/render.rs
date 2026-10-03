//! Schema model → compact definition text (grammar in the crate docs).

use serde_json::Value;

use crate::schema::{Field, Kind, Object, Range, Ty};

/// Type keywords; a string literal equal to one of these is quoted so it reads as a value.
pub(crate) const KEYWORDS: &[&str] = &[
    "str", "int", "num", "bool", "any", "obj", "null", "true", "false", "datetime", "date", "time",
    "email", "uri", "uuid",
];

/// Words that never carry meaning on their own when judging if a description is redundant.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "at", "to", "for", "and", "or", "is", "be", "by", "as",
    "with", "this", "that", "it", "its", "e", "g", "eg", "ie",
];

/// Shortest word that may match another by prefix ("min" ~ "minutes").
const MIN_PREFIX_LEN: usize = 3;

/// What happens to property descriptions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DescriptionPolicy {
    /// Drop a property description only when every word in it is already said by the tool
    /// name, the property name or its type ("Event title" on `title`). Anything that adds a
    /// word ("Attendee emails" on `attendees`) is kept, so disambiguating text survives.
    #[default]
    Auto,
    /// Keep every description (lossless; `decode_tools` returns the schema verbatim).
    Keep,
}

pub(crate) struct Renderer<'a> {
    pub(crate) policy: DescriptionPolicy,
    pub(crate) tool_words: &'a [String],
}

impl Renderer<'_> {
    /// `field, field, ...` (used for both the top-level argument list and nested objects).
    pub(crate) fn fields(&self, obj: &Object, out: &mut String) {
        for (i, f) in obj.fields.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            self.field(f, out);
        }
    }

    fn field(&self, f: &Field, out: &mut String) {
        push_key(&f.name, out);
        if !f.required {
            out.push('?');
        }
        out.push(':');
        self.ty(&f.ty, Some(&f.name), out);
    }

    fn ty(&self, ty: &Ty, field_name: Option<&str>, out: &mut String) {
        match &ty.kind {
            Kind::Str(None) => out.push_str("str"),
            Kind::Str(Some(f)) => out.push_str(f.keyword()),
            Kind::Int(r) => {
                out.push_str("int");
                push_range(r, out);
            }
            Kind::Num(r) => {
                out.push_str("num");
                push_range(r, out);
            }
            Kind::Bool => out.push_str("bool"),
            Kind::Any => out.push_str("any"),
            Kind::AnyObject => out.push_str("obj"),
            Kind::Enum(values) => {
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        out.push('|');
                    }
                    push_literal(v, out);
                }
            }
            Kind::Array(items) => {
                out.push('[');
                self.ty(items, None, out);
                out.push(']');
            }
            Kind::Object(obj) => {
                out.push('{');
                self.fields(obj, out);
                out.push('}');
                if obj.closed {
                    out.push('!');
                }
            }
        }
        if ty.nullable && !matches!(ty.kind, Kind::Enum(_)) {
            out.push_str("|null");
        }
        if let Some(d) = &ty.default {
            out.push('=');
            out.push_str(&d.to_string());
        }
        if let Some(desc) = &ty.description
            && self.keep_description(desc, ty, field_name)
        {
            out.push(' ');
            push_quoted(desc, out);
        }
    }

    fn keep_description(&self, desc: &str, ty: &Ty, field_name: Option<&str>) -> bool {
        match self.policy {
            DescriptionPolicy::Keep => true,
            DescriptionPolicy::Auto => {
                // Tool-name words count only on an exact match: "emails" in `send_email` names
                // the action, while "CC emails" on a field says the items are addresses.
                let mut known: Vec<String> = Vec::new();
                if let Some(name) = field_name {
                    known.extend(words(name));
                }
                known.extend(type_words(ty).iter().map(|w| (*w).to_string()));
                if let Kind::Enum(values) = &ty.kind {
                    for v in values {
                        known.extend(words(&v.to_string()));
                    }
                }
                !words(desc)
                    .iter()
                    .all(|w| self.tool_words.contains(w) || is_known(w, &known))
            }
        }
    }
}

/// Lower-cased alphanumeric words, split on punctuation, `_` and camelCase, minus stopwords.
pub(crate) fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = c.is_lowercase() || c.is_numeric();
            cur.extend(c.to_lowercase());
        } else {
            prev_lower = false;
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|w| !STOPWORDS.contains(&w.as_str()));
    out
}

fn is_known(word: &str, known: &[String]) -> bool {
    known.iter().any(|k| {
        let short = word.len().min(k.len());
        word == k
            || (short >= MIN_PREFIX_LEN && (word.starts_with(k.as_str()) || k.starts_with(word)))
    })
}

/// Words a type keyword already conveys to the model.
fn type_words(ty: &Ty) -> &'static [&'static str] {
    use crate::schema::Format;
    match &ty.kind {
        Kind::Str(None) => &["string", "text"],
        Kind::Str(Some(Format::DateTime)) => &[
            "date",
            "time",
            "datetime",
            "timestamp",
            "iso",
            "8601",
            "rfc",
            "3339",
            "utc",
        ],
        Kind::Str(Some(Format::Date)) => &["date", "day", "iso", "8601"],
        Kind::Str(Some(Format::Time)) => &["time", "iso", "8601"],
        Kind::Str(Some(Format::Email)) => &["email", "address"],
        Kind::Str(Some(Format::Uri)) => &["url", "uri", "link"],
        Kind::Str(Some(Format::Uuid)) => &["uuid", "id", "identifier"],
        Kind::Int(_) => &["integer", "int", "number", "whole"],
        Kind::Num(_) => &["number", "numeric", "float", "decimal"],
        Kind::Bool => &["boolean", "bool", "flag", "whether", "true", "false"],
        Kind::Array(_) => &["list", "array"],
        Kind::Any | Kind::AnyObject | Kind::Enum(_) | Kind::Object(_) => &[],
    }
}

/// A bare word: starts with a letter or `_`, then letters, digits, `_`, `-`, `.`, `/`.
pub(crate) fn is_bare_word(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

fn push_key(name: &str, out: &mut String) {
    let ident = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident {
        out.push_str(name);
    } else {
        push_quoted(name, out);
    }
}

fn push_literal(v: &Value, out: &mut String) {
    match v {
        Value::String(s) if is_bare_word(s) && !KEYWORDS.contains(&s.as_str()) => out.push_str(s),
        Value::String(s) => push_quoted(s, out),
        other => out.push_str(&other.to_string()),
    }
}

fn push_range(r: &Range, out: &mut String) {
    if r.is_empty() {
        return;
    }
    out.push('(');
    if let Some(n) = &r.min {
        out.push_str(&n.to_string());
    }
    out.push_str("..");
    if let Some(n) = &r.max {
        out.push_str(&n.to_string());
    }
    out.push(')');
}

/// `'text'` with `\\`, `\'`, `\n`, `\r`, `\t` escapes.
pub(crate) fn push_quoted(s: &str, out: &mut String) {
    out.push('\'');
    push_escaped(s, &['\''], out);
    out.push('\'');
}

/// Escapes backslash, line breaks, tabs and the given extra characters.
pub(crate) fn push_escaped(s: &str, extra: &[char], out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if extra.contains(&c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
}

//! Lexical rules shared by the encoder and [`crate::decode_tools`], so the two cannot drift.

use serde_json::Value;

/// Opens a call in a model reply.
pub(crate) const MARKER: &str = "<<call";

/// Words with a fixed meaning in a type expression. An enum value spelled like one is written
/// JSON-quoted instead of bare.
const RESERVED_WORDS: &[&str] = &[
    "string", "integer", "number", "boolean", "null", "object", "array", "any", "datetime", "date",
    "time", "true", "false",
];

/// `format` values with a one-word spelling. Any other format is written `string<fmt>`.
const FORMAT_ALIASES: &[(&str, &str)] = &[
    ("date-time", "datetime"),
    ("date", "date"),
    ("time", "time"),
];

pub(crate) fn alias_for_format(format: &str) -> Option<&'static str> {
    FORMAT_ALIASES
        .iter()
        .find(|(f, _)| *f == format)
        .map(|(_, alias)| *alias)
}

pub(crate) fn format_for_alias(word: &str) -> Option<&'static str> {
    FORMAT_ALIASES
        .iter()
        .find(|(_, alias)| *alias == word)
        .map(|(f, _)| *f)
}

pub(crate) fn is_reserved(word: &str) -> bool {
    RESERVED_WORDS.contains(&word)
}

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '+' | '-')
}

/// Whether an enum value can be written without quotes: it reads back as the same string and
/// cannot be mistaken for a keyword, a number or grammar punctuation.
pub(crate) fn is_bare_word(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(is_word_char)
        && !is_reserved(s)
}

/// An enum or const value as it appears in a type expression.
pub(crate) fn literal(value: &Value) -> String {
    match value {
        Value::String(s) if is_bare_word(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Descriptions stay on one line and survive a round trip byte for byte.
pub(crate) fn escape_description(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn unescape_description(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                let code = u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("bad escape `\\u{hex}`"))?;
                out.push(code);
            }
            other => return Err(format!("bad escape `\\{}`", other.unwrap_or(' '))),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bare_words_exclude_keywords_numbers_and_punctuation() {
        for ok in [
            "public",
            "high-priority",
            "en_US",
            "a.b",
            "image/png",
            "x:y",
        ] {
            assert!(is_bare_word(ok), "{ok}");
        }
        for quoted in [
            "string", "null", "true", "2xl", "-a", "New York", "a|b", "a,b", "",
        ] {
            assert!(!is_bare_word(quoted), "{quoted}");
        }
        assert_eq!(literal(&json!("string")), "\"string\"");
        assert_eq!(literal(&json!(3)), "3");
    }

    #[test]
    fn description_escaping_round_trips_every_control_character() {
        let nasty = "a # b \\ c\r\nd\te\u{0}f\u{1f}g 😀 \u{7f}";
        let escaped = escape_description(nasty);
        assert!(!escaped.contains('\n') && !escaped.contains('\r'));
        assert_eq!(unescape_description(&escaped).unwrap(), nasty);
    }
}

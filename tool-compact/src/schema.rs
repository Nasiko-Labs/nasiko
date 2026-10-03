//! JSON Schema type → compact type-string rendering.
//!
//! Converts a JSON Schema property object into a short type token used in the
//! compact signature line, e.g.:
//!   `{"type":"string","format":"date-time"}` → `"datetime"`
//!   `{"type":"array","items":{"type":"string"}}` → `"[str]"`
//!   `{"type":"string","enum":["a","b"]}` → `"a|b"`
//!
//! All rendering is pure and deterministic — no I/O, no allocation beyond String.

use serde_json::Value;

/// Render a JSON Schema property object as a compact type token.
///
/// Falls back to `"any"` for unrecognised shapes rather than failing —
/// the compactor is lossy-to-the-model by design.
pub fn render_type(schema: &Value) -> String {
    // Enum shortcut: always prefer enum rendering regardless of base type
    if let Some(variants) = schema.get("enum").and_then(Value::as_array) {
        let parts: Vec<String> = variants
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        if !parts.is_empty() {
            return parts.join("|");
        }
    }

    let base_type = schema.get("type").and_then(Value::as_str).unwrap_or("any");
    let format = schema.get("format").and_then(Value::as_str).unwrap_or("");

    match (base_type, format) {
        ("string", "date-time") | ("string", "datetime") => "datetime".into(),
        ("string", "date") => "date".into(),
        ("string", "time") => "time".into(),
        ("string", "uri") | ("string", "url") => "url".into(),
        ("string", "email") => "email".into(),
        ("string", "uuid") => "uuid".into(),
        ("string", _) => "str".into(),
        ("integer", _) => "int".into(),
        ("number", _) => "float".into(),
        ("boolean", _) => "bool".into(),
        ("object", _) => "obj".into(),
        ("null", _) => "null".into(),
        ("array", _) => {
            // Render the items type recursively
            let item_type = schema
                .get("items")
                .map(|items| render_type(items))
                .unwrap_or_else(|| "any".into());
            format!("[{item_type}]")
        }
        _ => "any".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_plain() {
        assert_eq!(render_type(&json!({"type":"string"})), "str");
    }
    #[test]
    fn string_datetime() {
        assert_eq!(render_type(&json!({"type":"string","format":"date-time"})), "datetime");
    }
    #[test]
    fn array_of_strings() {
        assert_eq!(render_type(&json!({"type":"array","items":{"type":"string"}})), "[str]");
    }
    #[test]
    fn enum_variants() {
        assert_eq!(render_type(&json!({"type":"string","enum":["public","private"]})), "public|private");
    }
    #[test]
    fn integer() {
        assert_eq!(render_type(&json!({"type":"integer"})), "int");
    }
    #[test]
    fn boolean() {
        assert_eq!(render_type(&json!({"type":"boolean"})), "bool");
    }
    #[test]
    fn unknown_falls_back() {
        assert_eq!(render_type(&json!({"type":"widget"})), "any");
    }
}

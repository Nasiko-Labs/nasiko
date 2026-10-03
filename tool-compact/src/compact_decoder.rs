use crate::error::{Result, ToolCompactError};
use crate::schema::ToolRegistry;
use crate::validator::validate_call;
use serde_json::Value;

pub const CALL_OPEN: &str = "<<call ";
pub const CALL_CLOSE: &str = ">>";

/// Finds every `<<call name {json}>>` in `text`, validates each against `registry`,
/// and returns `(tool_name, arguments)` in order of appearance.
///
/// * Text before, between and after calls is ignored (plain answers return no calls).
/// * Arguments are parsed by `serde_json`, so `>>` inside a JSON string, escapes and
///   unicode are handled correctly.
/// * Fail closed: unknown tool, bad JSON, truncated call, missing `>>`, missing required
///   field, wrong type or enum violation return an error, never a guessed call.
pub fn decode_compact_calls(
    text: &str,
    registry: &ToolRegistry,
) -> Result<Vec<(String, Value)>> {
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(CALL_OPEN) {
        let after = &rest[start + CALL_OPEN.len()..];
        let (name, args, consumed) = parse_one_call(after)?.ok_or_else(|| {
            ToolCompactError::Malformed("truncated tool call: text ends before '>>'".into())
        })?;
        let args = validate_call(&name, &args, registry)?;
        calls.push((name, args));
        rest = &after[consumed..];
    }
    Ok(calls)
}

/// Parses `name {json}>>` (the text right after `<<call `).
///
/// * `Ok(Some((name, args, consumed)))`: one complete call.
/// * `Ok(None)`: input is a valid prefix but incomplete (more text could finish it).
/// * `Err(..)`: input can never become a valid call.
pub(crate) fn parse_one_call(s: &str) -> Result<Option<(String, Value, usize)>> {
    let name_len = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .unwrap_or(s.len());
    if name_len == s.len() {
        return Ok(None); // name may still be growing
    }
    let name = &s[..name_len];
    if name.is_empty() {
        return Err(ToolCompactError::Malformed(
            "missing tool name after '<<call '".into(),
        ));
    }

    let after_name = &s[name_len..];
    let json_part = after_name.trim_start();
    if json_part.len() == after_name.len() {
        return Err(ToolCompactError::Malformed(format!(
            "expected a space and JSON arguments after tool name '{name}'"
        )));
    }
    let json_offset = s.len() - json_part.len();
    if json_part.is_empty() {
        return Ok(None);
    }
    if !json_part.starts_with('{') {
        return Err(ToolCompactError::InvalidArguments {
            tool: name.to_string(),
            reasoning: "arguments must be a JSON object".to_string(),
        });
    }

    let mut stream = serde_json::Deserializer::from_str(json_part).into_iter::<Value>();
    let value = match stream.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) if e.is_eof() => return Ok(None),
        Some(Err(e)) => {
            return Err(ToolCompactError::Malformed(format!(
                "invalid JSON arguments for '{name}': {e}"
            )));
        }
        None => return Ok(None),
    };
    let json_end = stream.byte_offset();

    let tail = &json_part[json_end..];
    let tail_trimmed = tail.trim_start();
    if tail_trimmed.is_empty() || tail_trimmed == ">" {
        return Ok(None); // waiting for '>>'
    }
    if !tail_trimmed.starts_with(CALL_CLOSE) {
        return Err(ToolCompactError::Malformed(format!(
            "expected '>>' after the arguments of '{name}'"
        )));
    }

    let consumed = json_offset + json_end + (tail.len() - tail_trimmed.len()) + CALL_CLOSE.len();
    Ok(Some((name.to_string(), value, consumed)))
}

#[cfg(test)]
mod v1_decoder_tests {
    use super::*;
    use crate::schema::{ParameterSchema, ToolSchema, ValueType};

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(
            ToolSchema::new("create_calendar_event", "Create an event")
                .with_parameter(ParameterSchema::new("title", ValueType::String, true))
                .with_parameter(ParameterSchema::new("start", ValueType::String, true))
                .with_parameter(
                    ParameterSchema::new("visibility", ValueType::String, false)
                        .with_enum(vec!["public".into(), "private".into()]),
                ),
        );
        r
    }

    #[test]
    fn decodes_single_call() {
        let t = r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#;
        let calls = decode_compact_calls(t, &registry()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "create_calendar_event");
        assert_eq!(calls[0].1["title"], "Retro");
    }

    #[test]
    fn marker_inside_string_argument_is_not_the_end() {
        let t = r#"<<call create_calendar_event {"title":"a >> b","start":"x"}>>"#;
        let calls = decode_compact_calls(t, &registry()).unwrap();
        assert_eq!(calls[0].1["title"], "a >> b");
    }

    #[test]
    fn text_around_and_multiple_calls() {
        let t = r#"Sure. <<call create_calendar_event {"title":"A","start":"x"}>> and <<call create_calendar_event {"title":"B","start":"y"}>> done."#;
        let calls = decode_compact_calls(t, &registry()).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].1["title"], "B");
    }

    #[test]
    fn plain_answer_has_no_calls() {
        let calls = decode_compact_calls("It is sunny today.", &registry()).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn unknown_tool_is_an_error() {
        let err = decode_compact_calls(r#"<<call nope {"a":1}>>"#, &registry()).unwrap_err();
        assert!(matches!(err, ToolCompactError::UnknownTool(_)));
    }

    #[test]
    fn enum_violation_is_an_error() {
        let t = r#"<<call create_calendar_event {"title":"A","start":"x","visibility":"secret"}>>"#;
        let err = decode_compact_calls(t, &registry()).unwrap_err();
        assert!(matches!(err, ToolCompactError::InvalidArguments { .. }));
    }

    #[test]
    fn missing_required_is_an_error() {
        let t = r#"<<call create_calendar_event {"start":"x"}>>"#;
        let err = decode_compact_calls(t, &registry()).unwrap_err();
        assert!(matches!(err, ToolCompactError::InvalidArguments { .. }));
    }

    #[test]
    fn truncated_call_is_an_error() {
        let t = r#"<<call create_calendar_event {"title":"A","start":"x"}"#;
        let err = decode_compact_calls(t, &registry()).unwrap_err();
        assert!(matches!(err, ToolCompactError::Malformed(_)));
    }

    #[test]
    fn non_object_arguments_are_an_error() {
        let err = decode_compact_calls("<<call create_calendar_event [1]>>", &registry())
            .unwrap_err();
        assert!(matches!(err, ToolCompactError::InvalidArguments { .. }));
    }
}
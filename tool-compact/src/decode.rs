//! Compact text back to schemas and, in later tasks, tool calls.
//!
//! `decode_tools` reads the signature text. It does not return the originals
//! stored on [`CompactTools`], so a renderer that drops a type fails the test.

use serde_json::{Map, Value};

use crate::encode::INSTRUCTION;
use crate::grammar;
use crate::schema::{self, Field, Scalar, Shape};
use crate::types::{CompactError, ToolCall, ToolDef};

pub(crate) fn schemas_from_text(text: &str) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line == INSTRUCTION {
            continue;
        }
        tools.push(parse_signature(line)?);
    }
    Ok(tools)
}

fn parse_signature(line: &str) -> Result<ToolDef, CompactError> {
    let (name, rest) = split_name(line)?;
    let (params, description) = split_params_and_description(&rest)?;
    let fields = parse_fields(&params)?;
    let description = if description.is_empty() {
        None
    } else {
        Some(description)
    };
    Ok(ToolDef {
        name,
        description,
        parameters: Some(object_schema(&fields)),
    })
}

fn split_name(line: &str) -> Result<(String, String), CompactError> {
    let mut name = String::new();
    let mut chars = line.chars();
    for ch in chars.by_ref() {
        if ch == '(' {
            return Ok((name, chars.collect()));
        }
        name.push(ch);
    }
    Err(malformed())
}

fn split_params_and_description(rest: &str) -> Result<(String, String), CompactError> {
    let mut params = String::new();
    let mut depth = 0i32;
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ')' && depth == 0 {
            let mut tail = String::new();
            tail.extend(chars);
            let description = tail
                .strip_prefix(" - ")
                .ok_or_else(malformed)?
                .to_string();
            return Ok((params, description));
        }
        match ch {
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            _ => {}
        }
        params.push(ch);
    }
    Err(malformed())
}

fn parse_fields(body: &str) -> Result<Vec<Field>, CompactError> {
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top(body, ',')
        .into_iter()
        .map(|part| parse_field(&part))
        .collect()
}

fn parse_field(raw: &str) -> Result<Field, CompactError> {
    let raw = raw.trim();
    let (name, required, ty) = if let Some((name, ty)) = raw.split_once("?:") {
        (name, false, ty)
    } else if let Some((name, ty)) = raw.split_once(':') {
        (name, true, ty)
    } else {
        return Err(malformed());
    };
    Ok(Field {
        name: name.trim().to_string(),
        required,
        shape: parse_shape(ty.trim())?,
    })
}

fn parse_shape(token: &str) -> Result<Shape, CompactError> {
    if let Some(inner) = token.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(Shape::Array(Box::new(parse_shape(inner.trim())?)));
    }
    if let Some(inner) = token.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        return Ok(Shape::Object {
            fields: parse_fields(inner)?,
        });
    }
    if token.contains('|') {
        return Ok(Shape::Enum(
            token.split('|').map(str::to_string).collect(),
        ));
    }
    Ok(match token {
        "str" => Shape::Scalar(Scalar::Str),
        "int" => Shape::Scalar(Scalar::Int),
        "num" => Shape::Scalar(Scalar::Num),
        "bool" => Shape::Scalar(Scalar::Bool),
        "datetime" => Shape::Scalar(Scalar::DateTime),
        _ => return Err(malformed()),
    })
}

fn split_top(input: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in input.chars() {
        if ch == sep && depth == 0 {
            parts.push(std::mem::take(&mut current));
            continue;
        }
        if matches!(ch, '{' | '[') {
            depth += 1;
        } else if matches!(ch, '}' | ']') {
            depth -= 1;
        }
        current.push(ch);
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn object_schema(fields: &[Field]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        if field.required {
            required.push(Value::String(field.name.clone()));
        }
        properties.insert(field.name.clone(), shape_to_value(&field.shape));
    }
    let mut object = Map::new();
    object.insert("type".into(), Value::String("object".into()));
    object.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        object.insert("required".into(), Value::Array(required));
    }
    Value::Object(object)
}

fn shape_to_value(shape: &Shape) -> Value {
    match shape {
        Shape::Scalar(Scalar::Str) => serde_json::json!({"type": "string"}),
        Shape::Scalar(Scalar::Int) => serde_json::json!({"type": "integer"}),
        Shape::Scalar(Scalar::Num) => serde_json::json!({"type": "number"}),
        Shape::Scalar(Scalar::Bool) => serde_json::json!({"type": "boolean"}),
        Shape::Scalar(Scalar::DateTime) => {
            serde_json::json!({"type": "string", "format": "date-time"})
        }
        Shape::Enum(values) => serde_json::json!({"type": "string", "enum": values}),
        Shape::Array(inner) => serde_json::json!({"type": "array", "items": shape_to_value(inner)}),
        Shape::Object { fields } => object_schema(fields),
    }
}

fn malformed() -> CompactError {
    grammar::malformed("")
}

pub(crate) fn calls_from_text(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut rest = text.to_string();
    let mut calls = Vec::new();
    loop {
        let Some((_, after_marker)) = rest.split_once("<<call ") else {
            break;
        };
        let after_marker = after_marker.to_string();
        let (raw, consumed) = match grammar::take_call(&after_marker)? {
            grammar::Taken::Incomplete => return Err(grammar::malformed("")),
            grammar::Taken::Ready(raw, consumed) => (raw, consumed),
        };
        calls.push(accept_call(raw, tools)?);
        rest = after_marker.chars().skip(consumed).collect();
    }
    if rest.contains("<<call") {
        return Err(grammar::malformed(""));
    }
    Ok(calls)
}

pub(crate) fn accept_call(raw: grammar::RawCall, tools: &[ToolDef]) -> Result<ToolCall, CompactError> {
    let Some(tool) = tools.iter().find(|tool| tool.name == raw.name) else {
        return Err(CompactError::UnknownTool { name: raw.name });
    };
    let parsed: Value =
        serde_json::from_str(&raw.arguments).map_err(|_| grammar::malformed(&raw.name))?;
    if !parsed.is_object() {
        return Err(grammar::malformed(&raw.name));
    }
    let shape = schema::classify(tool)?;
    schema::check(&shape, &parsed).map_err(|reason| CompactError::InvalidArguments {
        name: raw.name.clone(),
        reason,
    })?;
    Ok(ToolCall {
        name: raw.name,
        arguments: raw.arguments,
    })
}

#[cfg(test)]
mod tests {
    use crate::fixtures::calendar;
    use crate::types::ToolDef;
    use serde_json::{Value, json};

    #[test]
    fn decoded_calendar_keeps_schema_facts() {
        let compact = crate::encode_tools(&[calendar()]).unwrap();
        let decoded = crate::decode_tools(&compact).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "create_calendar_event");
        assert_eq!(decoded[0].description, calendar().description);
        let params = decoded[0].parameters.as_ref().unwrap();
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&json!("title")) && required.contains(&json!("start")));
        assert_eq!(required.len(), 2);
        assert_eq!(params["properties"]["start"]["format"], "date-time");
        assert_eq!(
            params["properties"]["visibility"]["enum"],
            json!(["public", "private"])
        );
        assert_eq!(params["properties"]["attendees"]["items"]["type"], "string");
        assert_eq!(params["properties"]["duration_min"]["type"], "integer");
    }

    #[test]
    fn design_review_call_decodes_to_one_tool_call() {
        let calls = crate::decode_calls(crate::fixtures::design_review(), &[calendar()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["title"], "Design review");
        assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
        assert_eq!(args["attendees"], json!(["riya@example.com"]));
    }

    #[test]
    fn argument_key_order_does_not_matter() {
        let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","title":"Design review"}>>"#;
        let calls = crate::decode_calls(text, &[calendar()]).unwrap();
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["title"], "Design review");
        assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
    }

    #[test]
    fn prose_around_a_call_is_ignored() {
        let text = format!(
            "Sure.\n{design_review}\nDone.",
            design_review = crate::fixtures::design_review()
        );
        let calls = crate::decode_calls(&text, &[calendar()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
    }

    #[test]
    fn two_calls_come_back_in_source_order() {
        let text = r#"<<call send_email {"to":["sam@example.com"]}>> then <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#;
        let email = ToolDef {
            name: "send_email".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"to": {"type": "array", "items": {"type": "string"}}},
                "required": ["to"]
            })),
        };
        let calls = crate::decode_calls(text, &[calendar(), email]).unwrap();
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }

    #[test]
    fn plain_answer_has_no_calls_and_is_not_an_error() {
        let calls = crate::decode_calls("What's the weather?", &[calendar()]).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn schema_valid_calls_only() {
        let bad = [
            r#"<<call weather {"city":"Pune"}>>"#,
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#,
            r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
            r#"<<call create_calendar_event {"title":"Retro"}"#,
            "not a call <<call",
        ];
        for text in bad {
            assert!(crate::decode_calls(text, &[calendar()]).is_err(), "{text}");
            let tools = [calendar()];
            let mut dec = crate::StreamDecoder::new(&tools);
            let pushed = dec.push(text);
            let rejected = match pushed {
                Err(_) => true,
                Ok(calls) if calls.is_empty() => dec.finish().map(|left| left.is_empty()).unwrap_or(true),
                Ok(_) => false,
            };
            assert!(rejected, "{text}");
        }
        let ok = crate::decode_calls(crate::fixtures::design_review(), &[calendar()]).unwrap();
        let shape = crate::schema::classify(&calendar()).unwrap();
        let args: Value = serde_json::from_str(&ok[0].arguments).unwrap();
        assert!(crate::schema::check(&shape, &args).is_ok());
    }

    #[test]
    fn unknown_tool_is_unknown_tool() {
        let text = r#"<<call weather {"city":"Pune"}>>"#;
        let err = crate::decode_calls(text, &[calendar()]).unwrap_err();
        assert!(matches!(err, crate::CompactError::UnknownTool { name } if name == "weather"));
    }

    #[test]
    fn missing_title_is_invalid_arguments() {
        let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#;
        let err = crate::decode_calls(text, &[calendar()]).unwrap_err();
        assert!(matches!(
            err,
            crate::CompactError::InvalidArguments { reason: crate::ArgumentFault::MissingField(ref f), .. } if f == "title"
        ));
    }

    #[test]
    fn string_duration_is_wrong_type() {
        let text = r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","duration_min":"30"}>>"#;
        let err = crate::decode_calls(text, &[calendar()]).unwrap_err();
        assert!(matches!(
            err,
            crate::CompactError::InvalidArguments { reason: crate::ArgumentFault::WrongType { ref field }, .. } if field == "duration_min"
        ));
    }

    #[test]
    fn secret_visibility_is_a_bad_enum() {
        let text = r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let err = crate::decode_calls(text, &[calendar()]).unwrap_err();
        assert!(matches!(
            err,
            crate::CompactError::InvalidArguments { reason: crate::ArgumentFault::BadEnum { ref field }, .. } if field == "visibility"
        ));
    }

    #[test]
    fn truncated_marker_is_malformed() {
        let text = r#"<<call create_calendar_event {"title":"Retro"}"#;
        let err = crate::decode_calls(text, &[calendar()]).unwrap_err();
        assert!(matches!(
            err,
            crate::CompactError::InvalidArguments { reason: crate::ArgumentFault::Malformed, .. }
        ));
    }

    #[test]
    fn two_tools_keep_both_names() {
        let email = ToolDef {
            name: "send_email".into(),
            description: Some("Send an email.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": { "to": {"type": "array", "items": {"type": "string"}} },
                "required": ["to"]
            })),
        };
        let compact = crate::encode_tools(&[calendar(), email]).unwrap();
        let names: Vec<_> = crate::decode_tools(&compact)
            .unwrap()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, ["create_calendar_event", "send_email"]);
    }
}

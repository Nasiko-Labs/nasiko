use crate::{schema::ToolCall, CompactError};
use serde_json::Value;

const START: &str = "<<call ";
const END: &str = ">>";

pub fn decode_calls(output: &str) -> Result<Vec<ToolCall>, CompactError> {
    let mut calls = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = output[cursor..].find(START) {
        let start = cursor + relative_start;
        let body_start = start + START.len();

        let relative_end = find_call_end(&output[body_start..])
            .ok_or_else(|| {
                CompactError::MalformedCall(
                    "unterminated call marker".into(),
                )
            })?;

        let body_end = body_start + relative_end;
        let body = &output[body_start..body_end];

        calls.push(parse_call_body(body)?);

        cursor = body_end + END.len();
    }

    Ok(calls)
}

fn find_call_end(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();

    let mut in_string = false;
    let mut escaped = false;

    let mut i = 0;

    while i + 1 < bytes.len() {
        let ch = bytes[i];

        if escaped {
            escaped = false;
            i += 1;
            continue;
        }

        if ch == b'\\' && in_string {
            escaped = true;
            i += 1;
            continue;
        }

        if ch == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }

        if !in_string && bytes[i] == b'>' && bytes[i + 1] == b'>' {
            return Some(i);
        }

        i += 1;
    }

    None
}

fn parse_call_body(body: &str) -> Result<ToolCall, CompactError> {
    let body = body.trim();

    let split = body.find(' ').ok_or_else(|| {
        CompactError::MalformedCall(
            "expected '<tool_name> <json_arguments>'".into(),
        )
    })?;

    let name = body[..split].trim();

    if name.is_empty() {
        return Err(CompactError::MalformedCall(
            "missing tool name".into(),
        ));
    }

    let arguments = body[split..].trim();

    if arguments.is_empty() {
        return Err(CompactError::MalformedCall(
            "missing arguments".into(),
        ));
    }

    let arguments: Value = serde_json::from_str(arguments).map_err(|e| {
        CompactError::MalformedCall(format!("invalid JSON arguments: {e}"))
    })?;

    Ok(ToolCall {
        name: name.to_string(),
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_one_call() {
        let output =
            r#"<<call create_calendar_event {"title":"Design review"}>>"#;

        let calls = decode_calls(output).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Design review");
    }

    #[test]
    fn decodes_multiple_calls() {
        let output = r#"
        <<call create_calendar_event {"title":"Review"}>>
        <<call send_email {"to":["a@example.com"]}>>
        "#;

        let calls = decode_calls(output).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[1].name, "send_email");
    }

    #[test]
    fn ignores_text_around_calls() {
        let output =
            r#"Sure, I will do that. <<call send_email {"to":["a@example.com"]}>> Done."#;

        let calls = decode_calls(output).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
    }

    #[test]
    fn plain_answer_has_no_calls() {
        let output = "I don't need any tools for this.";

        let calls = decode_calls(output).unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn supports_end_marker_inside_json_string() {
        let output =
            r#"<<call send_email {"body":"hello >> world"}>>"#;

        let calls = decode_calls(output).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["body"], "hello >> world");
    }

    #[test]
    fn rejects_unterminated_call() {
        let output =
            r#"<<call send_email {"body":"hello"}"#;

        assert!(decode_calls(output).is_err());
    }
}
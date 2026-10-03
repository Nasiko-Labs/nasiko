//! Whole-string decoding of compact tool calls.

use crate::error::Result;
use crate::stream::StreamDecoder;
use crate::types::{ToolCall, ToolDef};

/// Decode every call in a complete model output, in order.
///
/// Text outside calls is ignored; output with no calls yields an empty list. If any
/// call is invalid the whole output is rejected and no calls are returned.
///
/// This runs the same code path as [`StreamDecoder`] fed a single chunk, so decoding
/// a whole string and decoding it chunk by chunk always agree.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools);
    decoder.push(text)?;
    decoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CompactError;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "send_email".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"}
                },
                "required": ["to", "subject", "body"]
            })),
        }]
    }

    fn email(subject: &str) -> String {
        format!(
            r#"<<call send_email {{"to":["sam@example.com"],"subject":"{subject}","body":"x"}}>>"#
        )
    }

    #[test]
    fn plain_answer_has_no_calls() {
        assert_eq!(
            decode_calls("The weather is unavailable.", &tools()).unwrap(),
            vec![]
        );
        assert_eq!(decode_calls("", &tools()).unwrap(), vec![]);
    }

    #[test]
    fn text_before_and_after_calls() {
        let out = format!("Sure.\n{}\nDone.", email("Hi"));
        let calls = decode_calls(&out, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["subject"], json!("Hi"));
    }

    #[test]
    fn multiple_calls_keep_order() {
        let out = format!("{}\n{}", email("first"), email("second"));
        let calls = decode_calls(&out, &tools()).unwrap();
        let subjects: Vec<_> = calls
            .iter()
            .map(|c| c.arguments["subject"].clone())
            .collect();
        assert_eq!(subjects, vec![json!("first"), json!("second")]);
    }

    #[test]
    fn close_marker_inside_string_argument() {
        let calls = decode_calls(&email("a >> b"), &tools()).unwrap();
        assert_eq!(calls[0].arguments["subject"], json!("a >> b"));
    }

    #[test]
    fn one_invalid_call_rejects_the_whole_output() {
        let out = format!("{}\n<<call send_email {{\"to\":[]}}>>", email("ok"));
        assert!(matches!(
            decode_calls(&out, &tools()),
            Err(CompactError::InvalidArguments { .. })
        ));
    }
}

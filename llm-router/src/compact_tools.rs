//! Opt-in non-streaming OpenAI Chat Completions transformation.
//! Unsupported requests pass through unchanged. Invalid compact output fails
//! the entire response with 502; there is no second billed model call.
use crate::{
    GatewayError,
    ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall},
};
use nasiko_tool_compact::{INSTRUCTIONS, ToolDef, decode_calls, encode_tools};
use serde_json::Value;

pub(crate) struct Session {
    tools: Vec<ToolDef>,
}

pub(crate) fn prepare(
    req: &mut ChatRequest,
    enabled: bool,
    provider: &str,
    has_fallbacks: bool,
) -> Option<Session> {
    // Preserve exact default behavior before even inspecting the request.
    if !enabled
        || provider != "openai"
        || has_fallbacks
        || req.is_streaming()
        || req.tool_choice.as_ref().is_some_and(|c| c != "auto")
        || req.extra.contains_key("response_format")
        || req
            .messages
            .iter()
            .any(|m| m.role == "tool" || m.tool_calls.is_some())
    {
        return None;
    }
    let native = req.tools.as_ref().filter(|t| !t.is_empty())?;
    let tools: Vec<ToolDef> = serde_json::from_value(serde_json::to_value(native).ok()?).ok()?;
    let compact = encode_tools(&tools).ok()?;
    // Insert before user text without rewriting author messages. Previous
    // tool-call conversations are excluded above; normal user/assistant history
    // remains unchanged.
    req.messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(Value::String(format!(
                "{INSTRUCTIONS}\n{}",
                compact.definitions
            ))),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    req.tools = None;
    req.tool_choice = None;
    req.extra.remove("parallel_tool_calls");
    Some(Session { tools })
}
impl Session {
    pub(crate) fn restore(&self, response: &mut ChatResponse) -> Result<(), GatewayError> {
        // Validate every choice first. Never return a partly transformed response.
        let mut decoded = Vec::new();
        for choice in &response.choices {
            if choice.message.tool_calls.is_some() {
                return Err(failure("unexpected_native_call"));
            }
            let text = match &choice.message.content {
                None | Some(Value::Null) => "",
                Some(Value::String(s)) => s.as_str(),
                _ => return Err(failure("invalid_content")),
            };
            let calls = decode_calls(text, &self.tools).map_err(|e| failure(e.code()))?;
            decoded.push(calls);
        }
        for (choice, calls) in response.choices.iter_mut().zip(decoded) {
            if calls.is_empty() {
                continue;
            }
            let calls = calls
                .into_iter()
                .map(|call| {
                    Ok(ToolCall {
                        id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: call.name,
                            arguments: serde_json::to_string(&call.arguments)
                                .map_err(|_| failure("invalid_arguments"))?,
                        },
                        extra: Default::default(),
                    })
                })
                .collect::<Result<Vec<_>, GatewayError>>()?;
            // Prose accompanying a tool call is intentionally omitted. Compact
            // markers never appear in the client-facing assistant content.
            choice.message.content = None;
            choice.message.tool_calls = Some(calls);
            choice.finish_reason = Some("tool_calls".into());
        }
        Ok(())
    }
}
fn failure(code: &str) -> GatewayError {
    GatewayError::Upstream(format!("compact-tools: {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn request() -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [
                {
                    "role": "user",
                    "content": "send hello"
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "send",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "text": {
                                    "type": "string"
                                }
                            },
                            "required": [
                                "text"
                            ]
                        }
                    }
                }
            ],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "temperature": 0
        }))
        .unwrap()
    }
    fn response(text: &str) -> ChatResponse {
        serde_json::from_value(json!({"id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})).unwrap()
    }
    #[test]
    fn disabled_is_byte_identical() {
        let mut r = request();
        let bytes = serde_json::to_vec(&r).unwrap();
        assert!(prepare(&mut r, false, "openai", false).is_none());
        assert_eq!(serde_json::to_vec(&r).unwrap(), bytes);
        assert!(!crate::GatewayConfig::default().compact_tools_enabled);
    }
    #[test]
    fn request_and_response_seam() {
        let mut r = request();
        let session = prepare(&mut r, true, "openai", false).unwrap();
        assert!(r.tools.is_none());
        assert!(r.tool_choice.is_none());
        assert_eq!(r.messages[1].content.as_ref().unwrap(), "send hello");
        let mut resp = response("before <<call send {\"text\":\"hello >> 🦀\"}>> after");
        session.restore(&mut resp).unwrap();
        let call = &resp.choices[0].message.tool_calls.as_ref().unwrap()[0];
        assert!(call.id.starts_with("call_"));
        assert_eq!(call.function.name, "send");
        assert_eq!(
            serde_json::from_str::<Value>(&call.function.arguments).unwrap(),
            json!({"text":"hello >> 🦀"})
        );
        assert!(resp.choices[0].message.content.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }
    #[test]
    fn plain_answer_unchanged() {
        let mut r = request();
        let s = prepare(&mut r, true, "openai", false).unwrap();
        let mut resp = response("no tools needed");
        let before = serde_json::to_vec(&resp).unwrap();
        s.restore(&mut resp).unwrap();
        assert_eq!(serde_json::to_vec(&resp).unwrap(), before);
    }
    #[test]
    fn invalid_output_502_and_atomic() {
        let mut r = request();
        let s = prepare(&mut r, true, "openai", false).unwrap();
        let mut resp = response("<<call send {\"text\":\"valid\"}>> <<call delete {}>>");
        let before = serde_json::to_vec(&resp).unwrap();
        assert_eq!(
            s.restore(&mut resp).unwrap_err().status(),
            axum::http::StatusCode::BAD_GATEWAY
        );
        assert_eq!(serde_json::to_vec(&resp).unwrap(), before);
    }
    #[test]
    fn unsupported_requests_unchanged() {
        let mut cases = vec![];
        let mut stream = request();
        stream.stream = Some(true);
        cases.push((stream, "openai", false));
        let mut forced = request();
        forced.tool_choice = Some(json!("required"));
        cases.push((forced, "openai", false));
        let mut none = request();
        none.tool_choice = Some(json!("none"));
        cases.push((none, "openai", false));
        let mut history = request();
        history.messages.push(
            serde_json::from_value(json!({"role":"tool","content":"result","tool_call_id":"x"}))
                .unwrap(),
        );
        cases.push((history, "openai", false));
        let mut schema = request();
        schema.tools.as_mut().unwrap()[0]
            .function
            .parameters
            .as_mut()
            .unwrap()["$ref"] = json!("#/x");
        cases.push((schema, "openai", false));
        let mut structured = request();
        structured
            .extra
            .insert("response_format".into(), json!({"type":"json_object"}));
        cases.push((structured, "openai", false));
        cases.push((request(), "anthropic", false));
        cases.push((request(), "openai", true));
        for (mut r, provider, fallbacks) in cases {
            let before = serde_json::to_vec(&r).unwrap();
            assert!(prepare(&mut r, true, provider, fallbacks).is_none());
            assert_eq!(serde_json::to_vec(&r).unwrap(), before);
        }
    }
}

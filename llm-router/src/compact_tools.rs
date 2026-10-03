//! Optional request/response seam. The codec is shared with compact_tools_eval.
use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};
use nasiko_tool_compact::{
    CALL_INSTRUCTIONS, CompactError, ToolDef, decode_response, encode_tools,
};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bypass {
    Disabled,
    NoTools,
    Streaming,
    ToolChoice,
    History,
    ExtraFields,
    UnsupportedSchema,
    NoReduction,
}

impl Bypass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoTools => "no_tools",
            Self::Streaming => "streaming",
            Self::ToolChoice => "tool_choice",
            Self::History => "history",
            Self::ExtraFields => "extra_fields",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::NoReduction => "no_reduction",
        }
    }
}

/// The permissive IR doesn't retain unknown function-level fields, so inspect
/// them before parsing. A strict or future tool definition is passed natively.
pub fn wire_supported(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_none_or(|tools| {
            tools.iter().all(|tool| {
                tool.as_object()
                    .is_some_and(|m| m.keys().all(|k| matches!(k.as_str(), "type" | "function")))
                    && tool
                        .get("function")
                        .and_then(Value::as_object)
                        .is_some_and(|m| {
                            m.keys().all(|k| {
                                matches!(k.as_str(), "name" | "description" | "parameters")
                            })
                        })
            })
        })
}

/// Owns only request-scoped schemas, never provider credentials.
#[derive(Debug)]
pub struct Session {
    pub tools: Vec<ToolDef>,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

/// On bypass, req is completely untouched. Eligibility is deliberately narrow.
pub fn prepare_request(req: &mut ChatRequest, enabled: bool) -> Result<Session, Bypass> {
    if !enabled {
        return Err(Bypass::Disabled);
    }
    if req.is_streaming() {
        return Err(Bypass::Streaming);
    }
    if !req.extra.is_empty() {
        return Err(Bypass::ExtraFields);
    }
    if req.tool_choice.as_ref().is_some_and(|c| c != "auto") {
        return Err(Bypass::ToolChoice);
    }
    if req.messages.iter().any(|m| {
        m.role == "tool"
            || m.tool_calls.is_some()
            || m.tool_call_id.is_some()
            || !m.extra.is_empty()
            || m.content
                .as_ref()
                .is_some_and(|v| !v.is_string() && !v.is_null())
    }) {
        return Err(Bypass::History);
    }
    let native = req
        .tools
        .as_ref()
        .filter(|t| !t.is_empty())
        .ok_or(Bypass::NoTools)?;
    let mut tools = Vec::new();
    for t in native {
        if t.kind != "function" || !t.extra.is_empty() {
            return Err(Bypass::ExtraFields);
        }
        tools.push(ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t
                .function
                .parameters
                .clone()
                .ok_or(Bypass::UnsupportedSchema)?,
        });
    }
    let compact = encode_tools(&tools).map_err(|_| Bypass::UnsupportedSchema)?;
    let mut outbound = req.clone();
    outbound.tools = None;
    outbound.tool_choice = None;
    let insertion = outbound
        .messages
        .iter()
        .position(|m| m.role != "system" && m.role != "developer")
        .unwrap_or(outbound.messages.len());
    outbound.messages.insert(
        insertion,
        Message {
            role: "system".into(),
            content: Some(Value::String(format!(
                "Tools:\n{}{CALL_INSTRUCTIONS}",
                compact.catalog().map_err(|_| Bypass::UnsupportedSchema)?
            ))),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    let bytes_before = serde_json::to_vec(req).expect("serializable IR").len();
    let bytes_after = serde_json::to_vec(&outbound)
        .expect("serializable IR")
        .len();
    if bytes_after >= bytes_before {
        return Err(Bypass::NoReduction);
    }
    *req = outbound;
    Ok(Session {
        tools,
        bytes_before,
        bytes_after,
    })
}

impl Session {
    /// Validate every choice before committing any response mutation.
    pub fn restore(&self, response: &mut ChatResponse) -> Result<(), CompactError> {
        let mut restored = response.clone();
        for choice in &mut restored.choices {
            if choice.finish_reason.as_deref() != Some("stop") {
                return Err(CompactError::MalformedOutput(
                    "compact response did not finish normally".into(),
                ));
            }
            if choice
                .message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| !calls.is_empty())
            {
                return Err(CompactError::MalformedOutput(
                    "unexpected native tool calls in compact response".into(),
                ));
            }
            let text = match &choice.message.content {
                Some(Value::String(s)) => s.as_str(),
                None | Some(Value::Null) => {
                    return Err(CompactError::MalformedOutput(
                        "missing compact response text".into(),
                    ));
                }
                _ => {
                    return Err(CompactError::MalformedOutput(
                        "non-text compact response".into(),
                    ));
                }
            };
            let decoded = decode_response(text, &self.tools)?;
            if !decoded.calls.is_empty() {
                choice.message.tool_calls = Some(
                    decoded
                        .calls
                        .into_iter()
                        .map(|c| ToolCall {
                            id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                            kind: "function".into(),
                            function: FunctionCall {
                                name: c.name,
                                arguments: serde_json::to_string(&c.arguments).expect("JSON Value"),
                            },
                            extra: Default::default(),
                        })
                        .collect(),
                );
                choice.message.content = if decoded.text.trim().is_empty() {
                    None
                } else {
                    Some(Value::String(decoded.text))
                };
                choice.finish_reason = Some("tool_calls".into());
            }
        }
        *response = restored;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> ChatRequest {
        serde_json::from_value(json!({"messages":[{"role":"user","content":"Create an event"}],"tools":[{
            "type":"function","function":{"name":"event","description":"Create a calendar event.",
            "parameters":{"type":"object","properties":{
                "title":{"type":"string","description":"Event title"},
                "start":{"type":"string","description":"Start time"},
                "attendees":{"type":"array","items":{"type":"string"},"description":"Attendee emails"},
                "visibility":{"type":"string","enum":["public","private"],"description":"Event visibility"},
                "duration":{"type":"integer","description":"Duration in minutes"}
            },"required":["title","start"]}}
        }]})).unwrap()
    }
    fn response(text: &str) -> ChatResponse {
        serde_json::from_value(json!({"id":"completion-test","model":"test","choices":[{
            "index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"
        }],"usage":{"prompt_tokens":50,"completion_tokens":20,"total_tokens":70}}))
        .unwrap()
    }
    #[test]
    fn disabled_is_byte_identical() {
        let mut req = request();
        let before = serde_json::to_vec(&req).unwrap();
        assert_eq!(
            prepare_request(&mut req, false).unwrap_err(),
            Bypass::Disabled
        );
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
        assert!(!crate::GatewayConfig::default().compact_tools_enabled);
    }
    #[test]
    fn supported_request_and_response_restore_client_protocol() {
        let mut req = request();
        let session = prepare_request(&mut req, true).unwrap();
        assert!(req.tools.is_none());
        assert!(session.bytes_after < session.bytes_before);
        let mut resp =
            response(r#"Before <<call event {"title":"Review","start":"Monday"}>> After"#);
        let usage = resp.usage.clone();
        session.restore(&mut resp).unwrap();
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.content, Some(json!("Before  After")));
        let call = &choice.message.tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.function.name, "event");
        assert_eq!(
            serde_json::from_str::<Value>(&call.function.arguments).unwrap(),
            json!({"title":"Review","start":"Monday"})
        );
        assert_eq!(
            serde_json::to_value(resp.usage).unwrap(),
            serde_json::to_value(usage).unwrap()
        );
    }
    #[test]
    fn invalid_response_is_atomic() {
        let mut req = request();
        let session = prepare_request(&mut req, true).unwrap();
        let mut resp = response(
            r#"<<call event {"title":"Review","start":"Monday"}>> <<call event {"title":"Missing start"}>>"#,
        );
        let before = serde_json::to_vec(&resp).unwrap();
        assert_eq!(
            session.restore(&mut resp).unwrap_err().code(),
            "invalid_arguments"
        );
        assert_eq!(serde_json::to_vec(&resp).unwrap(), before);
        let mut truncated = response(r#"<<call event {"title":"Review","start":"Monday"}>>"#);
        truncated.choices[0].finish_reason = Some("length".into());
        assert!(session.restore(&mut truncated).is_err());
    }
    #[test]
    fn missing_text_native_calls_and_truncated_plain_text_are_rejected_atomically() {
        let session = prepare_request(&mut request(), true).unwrap();
        for kind in ["missing", "null", "native", "truncated", "second_choice"] {
            let mut resp = response("Ordinary answer");
            match kind {
                "missing" => resp.choices[0].message.content = None,
                "null" => resp.choices[0].message.content = Some(Value::Null),
                "native" => {
                    resp.choices[0].message.tool_calls = Some(vec![ToolCall {
                        id: "native-call".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "event".into(),
                            arguments: "{}".into(),
                        },
                        extra: Default::default(),
                    }])
                }
                "truncated" => resp.choices[0].finish_reason = Some("length".into()),
                "second_choice" => {
                    resp.choices[0].message.content = Some(json!(
                        "<<call event {\"title\":\"Review\",\"start\":\"Monday\"}>>"
                    ));
                    let mut second = resp.choices[0].clone();
                    second.index = 1;
                    second.message.content = None;
                    resp.choices.push(second);
                }
                _ => unreachable!(),
            }
            let before = serde_json::to_vec(&resp).unwrap();
            assert_eq!(
                session.restore(&mut resp).unwrap_err().code(),
                "malformed_output",
                "{kind}"
            );
            assert_eq!(serde_json::to_vec(&resp).unwrap(), before);
        }
    }
    #[test]
    fn unsupported_requests_are_unchanged() {
        for kind in ["stream", "choice", "history", "schema", "extra"] {
            let mut req = request();
            match kind {
                "stream" => req.stream = Some(true),
                "choice" => req.tool_choice = Some(json!("required")),
                "history" => req.messages[0].role = "tool".into(),
                "schema" => {
                    req.tools.as_mut().unwrap()[0]
                        .function
                        .parameters
                        .as_mut()
                        .unwrap()["$ref"] = json!("remote")
                }
                "extra" => {
                    req.extra
                        .insert("response_format".into(), json!({"type":"json_object"}));
                }
                _ => unreachable!(),
            }
            let before = serde_json::to_vec(&req).unwrap();
            assert!(prepare_request(&mut req, true).is_err());
            assert_eq!(serde_json::to_vec(&req).unwrap(), before);
        }
    }
    #[test]
    fn strict_and_future_tool_fields_bypass_before_ir_parsing() {
        let mut body = serde_json::to_value(request()).unwrap();
        assert!(wire_supported(&body));
        body["tools"][0]["function"]["strict"] = json!(true);
        assert!(!wire_supported(&body));
    }
}

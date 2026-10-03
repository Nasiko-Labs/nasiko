//! Opt-in compact tool schemas between the router and the model.
//!
//! Clients still send and receive OpenAI-shaped tool definitions and tool calls.
//! When [`crate::config::GatewayConfig::compact_tools`] is set, the router swaps
//! the outbound `tools` array for a compact system prompt and decodes
//! `<<call name {json}>>` from the assistant text. The flag defaults off, and
//! this module is not called unless it is on. A schema the compact crate cannot
//! represent leaves the request untouched.

use nasiko_tool_compact::{StreamDecoder, StreamItem, decode_calls, encode_tools};
use serde_json::{Map, Value};

use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, FunctionCallDelta, Message, ToolCall, ToolCallDelta,
    ToolDef,
};

/// Tools the model was shown, kept so the response can be checked against the
/// original schemas. `None` means this request was not compacted.
pub(crate) fn prepare(req: &mut ChatRequest) -> Option<Vec<nasiko_tool_compact::ToolDef>> {
    let tools = req.tools.as_ref()?;
    if tools.is_empty() {
        return None;
    }
    let compact: Vec<_> = tools.iter().map(to_compact).collect();
    let encoded = encode_tools(&compact).ok()?;
    if !encoded.compacted {
        tracing::debug!(
            target: "nasiko::llm_router::compact_tools",
            reason = ?encoded.bypass_reason,
            "compact tools: schema bypassed, request unchanged"
        );
        return None;
    }
    req.tools = None;
    req.tool_choice = None;
    req.messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(Value::String(encoded.prompt)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        },
    );
    Some(compact)
}

/// Replace compact call text in a non-streaming response with OpenAI tool calls.
/// A decode error leaves the assistant message as the model wrote it.
pub(crate) fn rewrite_response(resp: &mut ChatResponse, tools: &[nasiko_tool_compact::ToolDef]) {
    for choice in &mut resp.choices {
        let Some(Value::String(text)) = choice.message.content.clone() else {
            continue;
        };
        let calls = match decode_calls(&text, tools) {
            Ok(calls) if !calls.is_empty() => calls,
            Ok(_) => continue,
            Err(err) => {
                tracing::warn!(
                    target: "nasiko::llm_router::compact_tools",
                    error = %err,
                    "compact tools: response was not a valid compact call"
                );
                continue;
            }
        };
        let prose = prose_around_calls(&text, tools);
        choice.message.tool_calls = Some(calls.into_iter().map(to_ir_call).collect());
        choice.message.content = prose
            .filter(|text| !text.trim().is_empty())
            .map(Value::String);
        if matches!(choice.finish_reason.as_deref(), Some("stop") | None) {
            choice.finish_reason = Some("tool_calls".into());
        }
    }
}

pub(crate) enum Translate {
    Chunks(Vec<ChatChunk>),
    /// Decode failed. The original chunk is restored and compact mode should stop.
    Failed(Box<ChatChunk>),
}

pub(crate) fn translate_chunk(decoder: &mut StreamDecoder, mut chunk: ChatChunk) -> Translate {
    let content = chunk
        .choices
        .first_mut()
        .and_then(|choice| choice.delta.content.take());
    let Some(content) = content else {
        return Translate::Chunks(vec![chunk]);
    };
    let items = match decoder.push(&content) {
        Ok(items) => items,
        Err(err) => {
            tracing::warn!(
                target: "nasiko::llm_router::compact_tools",
                error = %err,
                "compact tools: stream chunk was not valid compact output"
            );
            if let Some(choice) = chunk.choices.first_mut() {
                choice.delta.content = Some(content);
            }
            return Translate::Failed(Box::new(chunk));
        }
    };
    Translate::Chunks(project(chunk, items))
}

pub(crate) fn finish_stream(decoder: &mut StreamDecoder, template: &ChatChunk) -> Vec<ChatChunk> {
    match decoder.finish() {
        Ok(items) if !items.is_empty() => project(template.clone(), items),
        Ok(_) => Vec::new(),
        Err(err) => {
            tracing::warn!(
                target: "nasiko::llm_router::compact_tools",
                error = %err,
                "compact tools: stream ended inside a compact call"
            );
            Vec::new()
        }
    }
}

fn project(template: ChatChunk, items: Vec<StreamItem>) -> Vec<ChatChunk> {
    if items.is_empty() {
        return if chunk_has_tail(&template) {
            vec![template]
        } else {
            Vec::new()
        };
    }
    let last = items.len() - 1;
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let mut chunk = template.clone();
            if index != last {
                chunk.usage = None;
                if let Some(choice) = chunk.choices.first_mut() {
                    choice.finish_reason = None;
                }
            }
            apply_item(&mut chunk, item, index == last);
            chunk
        })
        .collect()
}

fn chunk_has_tail(chunk: &ChatChunk) -> bool {
    if chunk.usage.is_some() {
        return true;
    }
    chunk.choices.iter().any(|choice| {
        choice.finish_reason.is_some()
            || choice.delta.role.is_some()
            || choice.delta.tool_calls.is_some()
    })
}

fn apply_item(chunk: &mut ChatChunk, item: StreamItem, is_last: bool) {
    let Some(choice) = chunk.choices.first_mut() else {
        return;
    };
    match item {
        StreamItem::Text(text) => {
            choice.delta.content = Some(text);
        }
        StreamItem::Call(call) => {
            choice.delta.content = None;
            let index = call
                .id
                .strip_prefix("call_")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let name = call.function.name;
            let arguments = call.function.arguments;
            let id = call.id;
            choice.delta.tool_calls = Some(vec![ToolCallDelta {
                index,
                id: Some(id),
                kind: Some("function".into()),
                function: Some(FunctionCallDelta {
                    name: Some(name),
                    arguments: Some(arguments),
                }),
            }]);
            if is_last && matches!(choice.finish_reason.as_deref(), Some("stop") | None) {
                choice.finish_reason = Some("tool_calls".into());
            }
        }
    }
}

fn prose_around_calls(text: &str, tools: &[nasiko_tool_compact::ToolDef]) -> Option<String> {
    let mut decoder = StreamDecoder::new(tools);
    let mut items = decoder.push(text).ok()?;
    items.extend(decoder.finish().ok()?);
    let mut prose = String::new();
    for item in items {
        if let StreamItem::Text(part) = item {
            prose.push_str(&part);
        }
    }
    Some(prose)
}

fn to_compact(tool: &ToolDef) -> nasiko_tool_compact::ToolDef {
    nasiko_tool_compact::ToolDef {
        kind: tool.kind.clone(),
        function: nasiko_tool_compact::FunctionDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        },
        extra: tool.extra.clone(),
    }
}

fn to_ir_call(call: nasiko_tool_compact::ToolCall) -> ToolCall {
    ToolCall {
        id: call.id,
        kind: call.kind,
        function: crate::ir::FunctionCall {
            name: call.function.name,
            arguments: call.function.arguments,
        },
        extra: call.extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;
    use crate::ir::{Choice, FunctionDef, Message};
    use serde_json::json;

    fn sample_request() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("book it".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(vec![ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "ping".into(),
                    description: Some("Ping".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": { "n": { "type": "integer" } }
                    })),
                },
                extra: Map::new(),
            }]),
            tool_choice: Some(json!("auto")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn flag_defaults_off() {
        assert!(!GatewayConfig::default().compact_tools);
    }

    #[test]
    fn prepare_rewrites_only_the_request_it_is_given() {
        let mut req = sample_request();
        let before = serde_json::to_string(&sample_request()).unwrap();
        let tools = prepare(&mut req).unwrap();
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        assert_eq!(req.messages[0].role, "system");
        assert!(req.messages[0].text().unwrap().contains("ping"));
        assert_ne!(serde_json::to_string(&req).unwrap(), before);

        let mut resp = ChatResponse {
            id: "chatcmpl-1".into(),
            object: "chat.completion".into(),
            created: None,
            model: "gpt-4o-mini".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(Value::String("<<call ping {\"n\":1}>>".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Map::new(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Map::new(),
        };
        rewrite_response(&mut resp, &tools);
        let call = &resp.choices[0].message.tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.function.name, "ping");
        assert_eq!(call.function.arguments, "{\"n\":1}");
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        assert!(resp.choices[0].message.content.is_none());
    }

    #[test]
    fn unsupported_schema_does_not_mutate_the_request() {
        let mut req = sample_request();
        req.tools.as_mut().unwrap()[0].function.parameters =
            Some(json!({"type":"object","properties":{"id":{"$ref":"#/Id"}}}));
        let snapshot = serde_json::to_string(&req).unwrap();
        assert!(prepare(&mut req).is_none());
        assert_eq!(serde_json::to_string(&req).unwrap(), snapshot);
    }
}

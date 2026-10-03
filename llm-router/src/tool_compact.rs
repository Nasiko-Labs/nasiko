//! Compact tool schemas at the egress seam (opt-in, off by default).
//!
//! Replaces a request's native `tools` with one compact system message from
//! [`nasiko_tool_compact`], then decodes the `<<call …>>` markers in the reply back into standard
//! `tool_calls`. The client never sees the compact format in either direction.
//!
//! # Scope
//!
//! Non-streaming chat on every inbound surface (OpenAI, Anthropic, Gemini), because it runs on
//! the normalized [`ChatRequest`] IR. Streaming requests are passed through natively for now —
//! [`nasiko_tool_compact::StreamDecoder`] exists, but a mid-stream decode failure has no safe
//! recovery once text has reached the client.
//!
//! # Bypass, never approximate
//!
//! Compaction is skipped (the request goes out byte-identical) when there are no tools, when
//! `tool_choice` forces or forbids a call (the compact contract cannot guarantee either), when
//! `parallel_tool_calls` is `false`, when any tool is not a plain function, when a schema uses a
//! feature outside the crate's supported subset, or when history holds a tool result the text
//! form cannot carry (non-text content).
//!
//! # Failure policy
//!
//! A reply that does not decode — unknown tool, invalid arguments, malformed marker — is never
//! repaired. The caller re-sends the original native request and returns that answer instead,
//! so a client sees either a valid call or the provider's own native behaviour.

use std::collections::HashMap;

use nasiko_tool_compact as tc;
use serde_json::{Map, Value};

use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef, Usage};

/// Why compaction was skipped, for logs and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    NoTools,
    Streaming,
    ToolChoice,
    SequentialCallsOnly,
    NonFunctionTool,
    UnsupportedSchema(String),
    UnsupportedHistory,
}

impl Skipped {
    pub fn as_label(&self) -> &'static str {
        match self {
            Skipped::NoTools => "no_tools",
            Skipped::Streaming => "streaming",
            Skipped::ToolChoice => "tool_choice",
            Skipped::SequentialCallsOnly => "parallel_tool_calls_false",
            Skipped::NonFunctionTool => "non_function_tool",
            Skipped::UnsupportedSchema(_) => "unsupported_schema",
            Skipped::UnsupportedHistory => "unsupported_history",
        }
    }
}

/// What a compacted request needs to decode its reply.
#[derive(Debug, Clone)]
pub struct Compacted {
    pub tools: Vec<tc::ToolDef>,
}

/// Convert router tool definitions to the crate's (the seam the crate's docs describe).
pub fn to_compact_tools(tools: &[ToolDef]) -> Vec<tc::ToolDef> {
    tools
        .iter()
        .map(|t| tc::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect()
}

/// Compact `req` in place, or leave it untouched and say why not.
pub fn apply(req: &mut ChatRequest) -> Result<Compacted, Skipped> {
    let tools = match &req.tools {
        Some(t) if !t.is_empty() => t,
        _ => return Err(Skipped::NoTools),
    };
    if req.is_streaming() {
        return Err(Skipped::Streaming);
    }
    match &req.tool_choice {
        None => {}
        Some(Value::String(s)) if s == "auto" => {}
        Some(_) => return Err(Skipped::ToolChoice),
    }
    if req.extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Err(Skipped::SequentialCallsOnly);
    }
    if tools
        .iter()
        .any(|t| t.kind != "function" || !t.extra.is_empty())
    {
        return Err(Skipped::NonFunctionTool);
    }

    let compact_tools = to_compact_tools(tools);
    let compact =
        tc::encode_tools(&compact_tools).map_err(|e| Skipped::UnsupportedSchema(e.to_string()))?;
    let messages = history_to_text(&req.messages).ok_or(Skipped::UnsupportedHistory)?;

    // Everything validated; mutate only now so a skip leaves `req` exactly as it arrived.
    req.messages = with_tools_prompt(messages, compact.prompt());
    req.tools = None;
    req.tool_choice = None;
    Ok(Compacted {
        tools: compact_tools,
    })
}

/// Append the tools block to a leading plain-text system message, or insert one. Appending keeps
/// the author's text as an unchanged prefix (and the tools block is constant per tool set, so the
/// cacheable prefix stays stable); reusing the message saves the per-message framing tokens.
fn with_tools_prompt(mut messages: Vec<Message>, prompt: String) -> Vec<Message> {
    if let Some(first) = messages.first_mut()
        && first.role == "system"
        && let Some(Value::String(text)) = &mut first.content
    {
        text.push('\n');
        text.push_str(&prompt);
        return messages;
    }
    messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(Value::String(prompt)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        },
    );
    messages
}

/// Earlier native calls and results, rewritten into the compact text form the model now speaks.
/// `None` if a message cannot be carried faithfully.
fn history_to_text(messages: &[Message]) -> Option<Vec<Message>> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        if let Some(calls) = &m.tool_calls {
            let rendered = tc::render_calls(
                &calls
                    .iter()
                    .map(|c| tc::ToolCall {
                        name: c.function.name.clone(),
                        arguments: c.function.arguments.clone(),
                    })
                    .collect::<Vec<_>>(),
            );
            for c in calls {
                names.insert(c.id.as_str(), c.function.name.as_str());
            }
            let text = match m.text() {
                Some(t) if !t.trim().is_empty() => format!("{t}\n{rendered}"),
                _ => rendered,
            };
            out.push(Message {
                content: Some(Value::String(text)),
                tool_calls: None,
                ..m.clone()
            });
        } else if m.role == "tool" {
            let result = match &m.content {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(_)) => m.text()?,
                None | Some(Value::Null) => String::new(),
                Some(_) => return None,
            };
            let id = m.tool_call_id.as_deref().unwrap_or("");
            let name = names.get(id).copied().unwrap_or("tool");
            out.push(Message {
                role: "user".into(),
                content: Some(Value::String(format!("[result of {name}]\n{result}"))),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: m.extra.clone(),
            });
        } else {
            out.push(m.clone());
        }
    }
    Some(out)
}

/// Decode every choice's text into `tool_calls`, in place. Returns the number of calls.
///
/// On error `resp` is left untouched and the caller falls back to the native request.
pub fn decode_response(resp: &mut ChatResponse, compacted: &Compacted) -> Result<usize, tc::Error> {
    let mut decoded = Vec::with_capacity(resp.choices.len());
    for choice in &resp.choices {
        decoded.push(match choice.message.text() {
            Some(text) => Some(tc::decode(&text, &compacted.tools)?),
            None => None,
        });
    }
    let mut total = 0;
    for (choice, d) in resp.choices.iter_mut().zip(decoded) {
        let Some(d) = d.filter(|d| !d.calls.is_empty()) else {
            continue;
        };
        total += d.calls.len();
        choice.message.content = d.content().map(Value::String);
        choice.message.tool_calls = Some(
            d.calls
                .into_iter()
                .map(|c| ToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: c.name,
                        arguments: c.arguments,
                    },
                    extra: Map::new(),
                })
                .collect(),
        );
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(total)
}

/// Token usage of a failed compact attempt plus its native retry, so the fallback is billed in
/// full. Cache fields come from the retry alone: they describe the request that was answered.
pub fn sum_usage(first: Option<Usage>, retry: Option<Usage>) -> Option<Usage> {
    let (Some(first), Some(mut retry)) = (first.as_ref(), retry.clone()) else {
        return retry.or(first);
    };
    let add = |a: Option<i64>, b: Option<i64>| match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
    retry.prompt_tokens = add(first.prompt_tokens, retry.prompt_tokens);
    retry.completion_tokens = add(first.completion_tokens, retry.completion_tokens);
    retry.total_tokens = add(first.total_tokens, retry.total_tokens);
    Some(retry)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(extra: Value) -> ChatRequest {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Email sam that the build is green"}],
            "tools": [{"type": "function", "function": {
                "name": "send_email",
                "description": "Send an email.",
                "parameters": {"type": "object", "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "body": {"type": "string"}
                }, "required": ["to", "body"]}
            }}]
        });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        serde_json::from_value(body).unwrap()
    }

    fn reply(text: &str) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "x", "model": "m",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": text}}]
        }))
        .unwrap()
    }

    #[test]
    fn compacts_tools_into_a_leading_system_message() {
        let mut req = request(json!({}));
        apply(&mut req).unwrap();
        assert!(req.tools.is_none());
        assert_eq!(req.messages[0].role, "system");
        let prompt = req.messages[0].text().unwrap();
        assert!(
            prompt.contains("send_email(to:[str], body:str) - Send an email."),
            "{prompt}"
        );
        assert_eq!(
            req.messages[1].text().unwrap(),
            "Email sam that the build is green"
        );
    }

    #[test]
    fn an_existing_system_prompt_is_kept_as_an_unchanged_prefix() {
        let mut req = request(json!({"messages": [
            {"role": "system", "content": "You are terse."},
            {"role": "user", "content": "hi"},
        ]}));
        apply(&mut req).unwrap();
        assert_eq!(req.messages.len(), 2);
        let sys = req.messages[0].text().unwrap();
        assert!(
            sys.starts_with("You are terse.\nTools:\nsend_email("),
            "{sys}"
        );
    }

    #[test]
    fn every_skip_leaves_the_request_byte_identical() {
        for (extra, why) in [
            (json!({"tools": []}), Skipped::NoTools),
            (json!({"stream": true}), Skipped::Streaming),
            (json!({"tool_choice": "required"}), Skipped::ToolChoice),
            (json!({"tool_choice": "none"}), Skipped::ToolChoice),
            (
                json!({"tool_choice": {"type": "function", "function": {"name": "send_email"}}}),
                Skipped::ToolChoice,
            ),
            (
                json!({"parallel_tool_calls": false}),
                Skipped::SequentialCallsOnly,
            ),
            (
                json!({"tools": [{"type": "function", "function": {"name": "f",
                "parameters": {"type": "object", "properties": {"a": {"oneOf": []}}}}}]}),
                Skipped::UnsupportedSchema(String::new()),
            ),
        ] {
            let mut req = request(extra);
            let before = serde_json::to_string(&req).unwrap();
            let err = apply(&mut req).unwrap_err();
            assert_eq!(err.as_label(), why.as_label());
            assert_eq!(serde_json::to_string(&req).unwrap(), before);
        }
    }

    #[test]
    fn history_calls_and_results_become_compact_text() {
        let mut req = request(json!({"messages": [
            {"role": "user", "content": "email sam"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "call_9", "type": "function",
                "function": {"name": "send_email", "arguments": "{\"to\":[\"sam\"],\"body\":\"hi\"}"}}]},
            {"role": "tool", "tool_call_id": "call_9", "content": "sent"},
        ]}));
        apply(&mut req).unwrap();
        assert_eq!(
            req.messages[2].text().unwrap(),
            "<<call send_email {\"to\":[\"sam\"],\"body\":\"hi\"}>>"
        );
        assert!(req.messages[2].tool_calls.is_none());
        assert_eq!(req.messages[3].role, "user");
        assert_eq!(
            req.messages[3].text().unwrap(),
            "[result of send_email]\nsent"
        );
    }

    #[test]
    fn decodes_a_reply_into_tool_calls() {
        let mut req = request(json!({}));
        let compacted = apply(&mut req).unwrap();
        let mut resp = reply(
            "On it.\n<<call send_email {\"to\":[\"sam@example.com\"],\"body\":\"Build is green\"}>>",
        );
        assert_eq!(decode_response(&mut resp, &compacted).unwrap(), 1);
        let msg = &resp.choices[0].message;
        let call = &msg.tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.function.name, "send_email");
        assert!(call.id.starts_with("call_"));
        assert_eq!(msg.text().as_deref(), Some("On it."));
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn plain_replies_are_untouched() {
        let mut req = request(json!({}));
        let compacted = apply(&mut req).unwrap();
        let mut resp = reply("  Nothing to do.  ");
        let before = serde_json::to_string(&resp).unwrap();
        assert_eq!(decode_response(&mut resp, &compacted).unwrap(), 0);
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn a_bad_reply_is_an_error_and_leaves_the_response_untouched() {
        let mut req = request(json!({}));
        let compacted = apply(&mut req).unwrap();
        for text in [
            "<<call delete_everything {}>>",
            "<<call send_email {\"to\":[\"sam\"]}>>",
            "<<call send_email {\"to\":\"sam\",\"body\":\"x\"}>>",
        ] {
            let mut resp = reply(text);
            let before = serde_json::to_string(&resp).unwrap();
            assert!(decode_response(&mut resp, &compacted).is_err(), "{text}");
            assert_eq!(serde_json::to_string(&resp).unwrap(), before);
        }
    }
}

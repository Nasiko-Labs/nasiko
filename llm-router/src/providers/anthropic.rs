//! Anthropic provider — OpenAI ⇄ Anthropic Messages API translation.
//!
//! Request (C1/C3): pull `system` out to the top level; `tools[].function.parameters`
//! → `tools[].input_schema` (no `type:function` wrapper); assistant `tool_calls[]` →
//! `tool_use` blocks; `{role:"tool"}` results → a following user turn with
//! `tool_result` blocks (consecutive results merged into one turn). `max_tokens` is
//! required by Anthropic, so we always send one.
//!
//! Response (C2): `content[].tool_use` → OpenAI `tool_calls[]` (arguments as a JSON
//! string); text blocks → `message.content`; `stop_reason` → `finish_reason`;
//! `usage.{input,output}_tokens` → `{prompt,completion,total}_tokens`.

use std::collections::HashMap;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Map, Value, json};

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError, delta_chunk, finish_chunk, now_unix, usage_chunk};
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, Choice, Delta, EmbeddingsRequest, EmbeddingsResponse,
    FunctionCall, FunctionCallDelta, Message, ToolCall, ToolCallDelta, ToolDef, Usage,
};
use crate::resolver::ResolvedConfig;

/// Anthropic requires `max_tokens`; used when neither config nor request sets it.
const DEFAULT_MAX_TOKENS: i64 = 4096;
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://api.anthropic.com/v1` (overridable for tests).
    base: String,
}

impl AnthropicProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self { http, base }
    }
}

#[async_trait]
impl ProviderClient for AnthropicProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let body = to_anthropic_request(req, cfg);

        let resp = self
            .http
            .post(format!("{}/messages", self.base))
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Status {
                status: status.as_u16(),
                message: text,
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }

        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        Ok(from_anthropic_response(&value, &cfg.model))
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let mut body = to_anthropic_request(req, cfg);
        body["stream"] = json!(true);

        let resp = self
            .http
            .post(format!("{}/messages", self.base))
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Status {
                status: status.as_u16(),
                message: text,
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }

        let model = cfg.model.clone();
        let data = sse_data_stream(resp.bytes_stream());
        let stream = async_stream::stream! {
            futures::pin_mut!(data);
            let mut id = String::new();
            // Anthropic content-block index → OpenAI tool_call index (text blocks
            // don't get a tool index, so the two can differ).
            let mut block_to_tool: HashMap<i64, i64> = HashMap::new();
            let mut next_tool_index: i64 = 0;
            let mut input_tokens: Option<i64> = None;
            let mut output_tokens: Option<i64> = None;
            let mut finish: Option<String> = None;

            while let Some(item) = data.next().await {
                let payload = match item {
                    Ok(p) => p,
                    Err(e) => { yield Err(e); return; }
                };
                let event: Value = match serde_json::from_str(&payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match event["type"].as_str() {
                    Some("message_start") => {
                        id = event["message"]["id"].as_str().unwrap_or_default().to_string();
                        input_tokens = event["message"]["usage"]["input_tokens"].as_i64();
                        yield Ok(delta_chunk(&id, &model, Delta {
                            role: Some("assistant".to_string()),
                            ..Delta::default()
                        }));
                    }
                    Some("content_block_start") => {
                        let block = &event["content_block"];
                        if block["type"].as_str() == Some("tool_use") {
                            let oa_index = next_tool_index;
                            next_tool_index += 1;
                            block_to_tool.insert(event["index"].as_i64().unwrap_or(0), oa_index);
                            yield Ok(delta_chunk(&id, &model, Delta {
                                tool_calls: Some(vec![ToolCallDelta {
                                    index: oa_index,
                                    id: block["id"].as_str().map(str::to_string),
                                    kind: Some("function".to_string()),
                                    function: Some(FunctionCallDelta {
                                        name: block["name"].as_str().map(str::to_string),
                                        arguments: Some(String::new()),
                                    }),
                                }]),
                                ..Delta::default()
                            }));
                        }
                    }
                    Some("content_block_delta") => {
                        let delta = &event["delta"];
                        match delta["type"].as_str() {
                            Some("text_delta") => {
                                if let Some(text) = delta["text"].as_str() {
                                    yield Ok(delta_chunk(&id, &model, Delta {
                                        content: Some(text.to_string()),
                                        ..Delta::default()
                                    }));
                                }
                            }
                            Some("input_json_delta") => {
                                let block_index = event["index"].as_i64().unwrap_or(0);
                                if let (Some(partial), Some(&oa_index)) =
                                    (delta["partial_json"].as_str(), block_to_tool.get(&block_index))
                                {
                                    yield Ok(delta_chunk(&id, &model, Delta {
                                        tool_calls: Some(vec![ToolCallDelta {
                                            index: oa_index,
                                            id: None,
                                            kind: None,
                                            function: Some(FunctionCallDelta {
                                                name: None,
                                                arguments: Some(partial.to_string()),
                                            }),
                                        }]),
                                        ..Delta::default()
                                    }));
                                }
                            }
                            _ => {}
                        }
                    }
                    Some("message_delta") => {
                        if let Some(sr) = event["delta"]["stop_reason"].as_str() {
                            finish = Some(map_stop_reason(sr).to_string());
                        }
                        if let Some(ot) = event["usage"]["output_tokens"].as_i64() {
                            output_tokens = Some(ot);
                        }
                    }
                    Some("message_stop") => break,
                    _ => {}
                }
            }

            // Terminal: finish chunk, then an OpenAI-style usage chunk (empty choices).
            yield Ok(finish_chunk(&id, &model, finish.unwrap_or_else(|| "stop".to_string())));
            yield Ok(usage_chunk(&id, &model, Usage {
                prompt_tokens: input_tokens,
                completion_tokens: output_tokens,
                total_tokens: match (input_tokens, output_tokens) {
                    (Some(i), Some(o)) => Some(i + o),
                    _ => None,
                },
            }));
        };
        Ok(Box::pin(stream))
    }

    async fn embeddings(
        &self,
        _req: &EmbeddingsRequest,
        _cfg: &ResolvedConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        Err(ProviderError::Status {
            status: 501,
            message: "Anthropic has no embeddings API".to_string(),
            retryable: false,
        })
    }
}

// ── OpenAI → Anthropic (request) ─────────────────────────────────────────────

fn to_anthropic_request(req: &ChatRequest, cfg: &ResolvedConfig) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    for m in &req.messages {
        match m.role.as_str() {
            "system" => {
                if let Some(t) = m.text() {
                    system_parts.push(t);
                }
            }
            "tool" => {
                // Accumulate consecutive tool results into one following user turn.
                pending_tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.text().unwrap_or_default(),
                }));
            }
            "assistant" => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                messages.push(assistant_to_anthropic(m));
            }
            _ => {
                // "user" (and any unexpected role) → a user text turn.
                flush_tool_results(&mut pending_tool_results, &mut messages);
                messages.push(json!({ "role": "user", "content": m.text().unwrap_or_default() }));
            }
        }
    }
    flush_tool_results(&mut pending_tool_results, &mut messages);

    let max_tokens = cfg.max_tokens.or(req.max_tokens).unwrap_or(DEFAULT_MAX_TOKENS);
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    if let Some(t) = cfg.temperature.or(req.temperature) {
        body["temperature"] = json!(t);
    }
    if !system_parts.is_empty() {
        body["system"] = json!(system_parts.join("\n"));
    }
    if let Some(tools) = &req.tools {
        let translated: Vec<Value> = tools.iter().map(tool_to_anthropic).collect();
        if !translated.is_empty() {
            body["tools"] = json!(translated);
        }
    }
    if let Some(choice) = req.tool_choice.as_ref().and_then(tool_choice_to_anthropic) {
        body["tool_choice"] = choice;
    }
    body
}

fn flush_tool_results(pending: &mut Vec<Value>, messages: &mut Vec<Value>) {
    if !pending.is_empty() {
        messages.push(json!({ "role": "user", "content": std::mem::take(pending) }));
    }
}

fn assistant_to_anthropic(m: &Message) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(t) = m.text()
        && !t.is_empty()
    {
        blocks.push(json!({ "type": "text", "text": t }));
    }
    if let Some(tool_calls) = &m.tool_calls {
        for tc in tool_calls {
            // arguments is a JSON string → Anthropic `input` must be an object.
            let input: Value = serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
            blocks.push(json!({
                "type": "tool_use",
                "id": tc.id,
                "name": tc.function.name,
                "input": input,
            }));
        }
    }
    match blocks.as_slice() {
        [] => json!({ "role": "assistant", "content": "" }),
        // Single text block → string content (both forms are valid; string is tidier).
        [only] if only["type"].as_str() == Some("text") => {
            json!({ "role": "assistant", "content": only["text"].clone() })
        }
        _ => json!({ "role": "assistant", "content": blocks }),
    }
}

fn tool_to_anthropic(t: &ToolDef) -> Value {
    let schema = t
        .function
        .parameters
        .clone()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let mut out = json!({ "name": t.function.name, "input_schema": schema });
    if let Some(desc) = &t.function.description {
        out["description"] = json!(desc);
    }
    out
}

fn tool_choice_to_anthropic(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(s) => match s.as_str() {
            "required" => Some(json!({ "type": "any" })),
            "none" => None, // Anthropic has no "none"; omit
            _ => Some(json!({ "type": "auto" })),
        },
        Value::Object(o) => o
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .map(|name| json!({ "type": "tool", "name": name })),
        _ => None,
    }
}

// ── Anthropic → OpenAI (response) ────────────────────────────────────────────

fn from_anthropic_response(body: &Value, model: &str) -> ChatResponse {
    let id = body["id"].as_str().unwrap_or_default();

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    if let Some(blocks) = body["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
                Some("tool_use") => tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_string(),
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        // input (object) → arguments (JSON string)
                        arguments: block
                            .get("input")
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                    extra: Map::new(),
                }),
                _ => {} // ignore other block types (e.g. thinking)
            }
        }
    }

    // Match OpenAI: content is null when the turn is purely tool calls.
    let content = if tool_calls.is_empty() {
        Some(Value::String(text))
    } else if text.is_empty() {
        Some(Value::Null)
    } else {
        Some(Value::String(text))
    };

    let finish_reason = map_stop_reason(body["stop_reason"].as_str().unwrap_or("end_turn"));

    let usage = body.get("usage").map(|u| {
        let input = u["input_tokens"].as_i64();
        let output = u["output_tokens"].as_i64();
        Usage {
            prompt_tokens: input,
            completion_tokens: output,
            total_tokens: match (input, output) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            },
        }
    });

    ChatResponse {
        id: format!("chatcmpl-{id}"),
        object: "chat.completion".to_string(),
        created: Some(now_unix()),
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".to_string(),
                content,
                name: None,
                tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
                tool_call_id: None,
                extra: Map::new(),
            },
            finish_reason: Some(finish_reason.to_string()),
        }],
        usage,
        extra: Map::new(),
    }
}

fn map_stop_reason(reason: &str) -> &'static str {
    match reason {
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        _ => "stop", // end_turn / stop_sequence / unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved() -> ResolvedConfig {
        ResolvedConfig {
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet-20241022".into(),
            litellm_model: "anthropic/claude-3-5-sonnet-20241022".into(),
            api_key: "sk-ant-test".into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
        }
    }

    #[test]
    fn request_extracts_system_and_translates_tools() {
        // Mirrors REQUEST_JOURNEY steps 2 → 5.
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "temperature": 0.1,
            "max_tokens": 4000,
            "tool_choice": "auto",
            "messages": [
                { "role": "system", "content": "You are a Translation agent." },
                { "role": "user", "content": "translate to hindi: my name is csg" }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "translate_text",
                    "description": "Translate plain text",
                    "parameters": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
                }
            }]
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved());

        assert_eq!(body["model"], "claude-3-5-sonnet-20241022");
        assert_eq!(body["max_tokens"], 4000); // from request (config didn't set it)
        assert_eq!(body["temperature"], 0.1);
        assert_eq!(body["system"], "You are a Translation agent.");
        // system removed from messages; only the user turn remains
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
        // tool: no "type":"function" wrapper; parameters → input_schema
        let tool = &body["tools"][0];
        assert_eq!(tool["name"], "translate_text");
        assert!(tool.get("type").is_none());
        assert_eq!(tool["input_schema"]["properties"]["text"]["type"], "string");
        assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
    }

    #[test]
    fn drops_unsupported_openai_params() {
        // drop_params: OpenAI-only params (top_p, frequency_penalty, …) live in `extra`
        // and must NOT be forwarded to Anthropic — the translator only emits mapped fields.
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "hi" }],
            "top_p": 0.9,
            "frequency_penalty": 0.5,
            "logit_bias": { "50256": -100 }
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved());
        assert!(body.get("top_p").is_none());
        assert!(body.get("frequency_penalty").is_none());
        assert!(body.get("logit_bias").is_none());
    }

    #[test]
    fn max_tokens_defaults_when_unset() {
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let body = to_anthropic_request(&req, &resolved());
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn multi_turn_tool_history_becomes_tool_use_and_tool_result() {
        // Mirrors REQUEST_JOURNEY step 9 (C3).
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "user", "content": "translate to hindi: my name is csg" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "toolu_01", "type": "function",
                    "function": { "name": "translate_text", "arguments": "{\"text\":\"my name is csg\",\"target_language\":\"hi\"}" }
                }]},
                { "role": "tool", "tool_call_id": "toolu_01", "content": "मेरा नाम csg है" }
            ]
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        // assistant turn carries a tool_use block with the parsed input object
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["id"], "toolu_01");
        assert_eq!(msgs[1]["content"][0]["input"]["target_language"], "hi");
        // tool result becomes a user turn with a tool_result block + matching id
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_01");
        assert_eq!(msgs[2]["content"][0]["content"], "मेरा नाम csg है");
    }

    #[test]
    fn response_tool_use_becomes_openai_tool_calls() {
        // Mirrors REQUEST_JOURNEY steps 6 → 7 (C2).
        let anthropic = json!({
            "id": "msg_01", "type": "message", "role": "assistant",
            "model": "claude-3-5-sonnet-20241022",
            "content": [{
                "type": "tool_use", "id": "toolu_01", "name": "translate_text",
                "input": { "text": "my name is csg", "target_language": "hi" }
            }],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 463, "output_tokens": 58 }
        });
        let resp = from_anthropic_response(&anthropic, "claude-3-5-sonnet-20241022");
        assert_eq!(resp.model, "claude-3-5-sonnet-20241022");
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let msg = &resp.choices[0].message;
        assert_eq!(msg.content, Some(Value::Null)); // pure tool call → null content
        let tc = &msg.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.id, "toolu_01");
        assert_eq!(tc.function.name, "translate_text");
        // arguments is a JSON string round-tripping the input object
        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["target_language"], "hi");
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(463));
        assert_eq!(usage.completion_tokens, Some(58));
        assert_eq!(usage.total_tokens, Some(521));
    }

    #[test]
    fn response_text_becomes_content_and_stop() {
        // Mirrors REQUEST_JOURNEY steps 10 → 11.
        let anthropic = json!({
            "id": "msg_02", "type": "message", "role": "assistant",
            "content": [{ "type": "text", "text": "Here is the translation." }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 531, "output_tokens": 42 }
        });
        let resp = from_anthropic_response(&anthropic, "claude-3-5-sonnet-20241022");
        assert_eq!(resp.choices[0].message.content, Some(Value::String("Here is the translation.".into())));
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn stream_reassembles_tool_call_deltas() {
        // Anthropic streams a tool call as content_block_start + input_json_delta
        // fragments; we must re-emit OpenAI delta.tool_calls[] chunks (C2 streaming).
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":50}}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"translate_text\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"text\\\":\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"hi\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":12}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        server
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({ "stream": true })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o", "stream": true,
            "messages": [{ "role": "user", "content": "translate" }]
        }))
        .unwrap();
        let stream = provider.chat_stream(&req, &resolved()).await.unwrap();
        let chunks: Vec<ChatChunk> = stream.filter_map(|r| async { r.ok() }).collect().await;

        // Reassemble the streamed tool-call arguments across deltas.
        let mut name = String::new();
        let mut args = String::new();
        let mut finish = None;
        let mut usage = None;
        for c in &chunks {
            if let Some(choice) = c.choices.first() {
                if let Some(tcs) = &choice.delta.tool_calls {
                    for tc in tcs {
                        if let Some(f) = &tc.function {
                            if let Some(n) = &f.name {
                                name = n.clone();
                            }
                            if let Some(a) = &f.arguments {
                                args.push_str(a);
                            }
                        }
                    }
                }
                if choice.finish_reason.is_some() {
                    finish = choice.finish_reason.clone();
                }
            }
            if c.usage.is_some() {
                usage = c.usage.clone();
            }
        }
        assert_eq!(name, "translate_text");
        assert_eq!(serde_json::from_str::<Value>(&args).unwrap()["text"], "hi");
        assert_eq!(finish.as_deref(), Some("tool_calls"));
        let usage = usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(50));
        assert_eq!(usage.completion_tokens, Some(12));
        assert_eq!(usage.total_tokens, Some(62));
    }

    #[tokio::test]
    async fn stream_text_deltas_become_content() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_2\",\"usage\":{\"input_tokens\":5}}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        server
            .mock("POST", "/messages")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;
        let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "stream": true, "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let chunks: Vec<ChatChunk> = provider
            .chat_stream(&req, &resolved())
            .await
            .unwrap()
            .filter_map(|r| async { r.ok() })
            .collect()
            .await;
        let text: String = chunks
            .iter()
            .filter_map(|c| c.choices.first().and_then(|ch| ch.delta.content.clone()))
            .collect();
        assert_eq!(text, "Hello");
        let finish = chunks.iter().find_map(|c| c.choices.first().and_then(|ch| ch.finish_reason.clone()));
        assert_eq!(finish.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn chat_calls_messages_endpoint_and_translates() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/messages")
            .match_header("x-api-key", "sk-ant-test")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "claude-3-5-sonnet-20241022", "system": "You are helpful."
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_9", "type": "message", "role": "assistant",
                    "content": [{ "type": "text", "text": "नमस्ते" }],
                    "stop_reason": "end_turn",
                    "usage": { "input_tokens": 10, "output_tokens": 3 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "hi" }
            ]
        }))
        .unwrap();
        let resp = provider.chat(&req, &resolved()).await.unwrap();
        m.assert_async().await;
        assert_eq!(resp.model, "claude-3-5-sonnet-20241022");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("नमस्ते"));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(13));
    }
}

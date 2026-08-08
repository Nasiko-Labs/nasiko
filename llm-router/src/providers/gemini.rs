//! Gemini provider — OpenAI ⇄ Gemini `generateContent` translation.
//!
//! Request (C1/C3): `system` → `systemInstruction`; user → `contents[role:user]`;
//! assistant text/`tool_calls` → `contents[role:model]` with `text`/`functionCall`
//! parts; `{role:"tool"}` results → a `contents[role:user]` with `functionResponse`
//! parts. Gemini keys `functionResponse` by function **name**, not the OpenAI
//! `tool_call_id`, so we track id→name across the history. `tools[].function` →
//! `tools[0].functionDeclarations[]`.
//!
//! Response (C2): `candidates[0].content.parts[].functionCall` → OpenAI `tool_calls[]`
//! (Gemini supplies no call id, so we synthesize one; arguments as a JSON string);
//! text parts → `content`; `finishReason`/presence-of-calls → `finish_reason`;
//! `usageMetadata` → `usage`.

use std::collections::HashMap;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Map, Value, json};

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError, delta_chunk, finish_chunk, now_unix, usage_chunk};
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, Choice, Delta, Embedding, EmbeddingsRequest,
    EmbeddingsResponse, FunctionCall, FunctionCallDelta, Message, ToolCall, ToolCallDelta, Usage,
};
use crate::resolver::ResolvedConfig;

pub struct GeminiProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://generativelanguage.googleapis.com/v1beta`.
    base: String,
}

impl GeminiProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self { http, base }
    }
}

#[async_trait]
impl ProviderClient for GeminiProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let body = to_gemini_request(req, cfg);

        let resp = self
            .http
            .post(format!(
                "{}/models/{}:generateContent",
                self.base, cfg.model
            ))
            .header("x-goog-api-key", &cfg.api_key)
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
        Ok(from_gemini_response(&value, &cfg.model))
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let body = to_gemini_request(req, cfg);

        let resp = self
            .http
            .post(format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                self.base, cfg.model
            ))
            .header("x-goog-api-key", &cfg.api_key)
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
            let id = format!("gemini-{}", now_unix());
            let mut role_sent = false;
            let mut tool_index: i64 = 0;
            let mut saw_tool_call = false;
            let mut finish: Option<String> = None;
            let mut usage: Option<Usage> = None;

            while let Some(item) = data.next().await {
                let payload = match item {
                    Ok(p) => p,
                    Err(e) => { yield Err(e); return; }
                };
                if payload.trim() == "[DONE]" {
                    break;
                }
                let response: Value = match serde_json::from_str(&payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let candidate = &response["candidates"][0];

                if !role_sent {
                    yield Ok(delta_chunk(&id, &model, Delta {
                        role: Some("assistant".to_string()),
                        ..Delta::default()
                    }));
                    role_sent = true;
                }

                if let Some(parts) = candidate["content"]["parts"].as_array() {
                    for part in parts {
                        if let Some(text) = part["text"].as_str() {
                            if !text.is_empty() {
                                yield Ok(delta_chunk(&id, &model, Delta {
                                    content: Some(text.to_string()),
                                    ..Delta::default()
                                }));
                            }
                        } else if let Some(fc) = part.get("functionCall") {
                            saw_tool_call = true;
                            let name = fc["name"].as_str().unwrap_or_default().to_string();
                            let arguments = fc
                                .get("args")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "{}".to_string());
                            let index = tool_index;
                            tool_index += 1;
                            yield Ok(delta_chunk(&id, &model, Delta {
                                tool_calls: Some(vec![ToolCallDelta {
                                    index,
                                    id: Some(format!("call_{name}_{index}")),
                                    kind: Some("function".to_string()),
                                    function: Some(FunctionCallDelta {
                                        name: Some(name),
                                        // Gemini sends complete args in one chunk.
                                        arguments: Some(arguments),
                                    }),
                                }]),
                                ..Delta::default()
                            }));
                        }
                    }
                }

                if let Some(fr) = candidate["finishReason"].as_str() {
                    finish = Some(if fr == "MAX_TOKENS" { "length" } else { "stop" }.to_string());
                }
                if let Some(um) = response.get("usageMetadata") {
                    usage = Some(Usage {
                        prompt_tokens: um["promptTokenCount"].as_i64(),
                        completion_tokens: um["candidatesTokenCount"].as_i64(),
                        total_tokens: um["totalTokenCount"].as_i64(),
                    });
                }
            }

            let finish_reason = if saw_tool_call {
                "tool_calls".to_string()
            } else {
                finish.unwrap_or_else(|| "stop".to_string())
            };
            yield Ok(finish_chunk(&id, &model, finish_reason));
            if let Some(u) = usage {
                yield Ok(usage_chunk(&id, &model, u));
            }
        };
        Ok(Box::pin(stream))
    }

    async fn embeddings(
        &self,
        req: &EmbeddingsRequest,
        cfg: &ResolvedConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        // OpenAI `input` (string | array) → one Gemini embed request per text.
        let inputs: Vec<String> = match &req.input {
            Value::String(s) => vec![s.clone()],
            Value::Array(arr) => arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect(),
            other => vec![other.to_string()],
        };
        let model_path = format!("models/{}", cfg.model);
        let requests: Vec<Value> = inputs
            .iter()
            .map(|text| json!({ "model": model_path, "content": { "parts": [{ "text": text }] } }))
            .collect();

        let resp = self
            .http
            .post(format!(
                "{}/models/{}:batchEmbedContents",
                self.base, cfg.model
            ))
            .header("x-goog-api-key", &cfg.api_key)
            .json(&json!({ "requests": requests }))
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
        let data = value["embeddings"]
            .as_array()
            .map(|embs| {
                embs.iter()
                    .enumerate()
                    .map(|(i, e)| Embedding {
                        object: "embedding".to_string(),
                        embedding: e["values"].clone(),
                        index: i as i64,
                        extra: Map::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Gemini's batch embed API returns no token counts.
        Ok(EmbeddingsResponse {
            object: "list".to_string(),
            data,
            model: cfg.model.clone(),
            usage: None,
            extra: Map::new(),
        })
    }
}

// ── OpenAI → Gemini (request) ────────────────────────────────────────────────

fn to_gemini_request(req: &ChatRequest, cfg: &ResolvedConfig) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    let mut id_to_name: HashMap<String, String> = HashMap::new();
    let mut pending_fn_responses: Vec<Value> = Vec::new();

    for m in &req.messages {
        match m.role.as_str() {
            "system" => {
                if let Some(t) = m.text() {
                    system_parts.push(t);
                }
            }
            "tool" => {
                let id = m.tool_call_id.clone().unwrap_or_default();
                // Gemini wants the function name; recover it from the call this answers.
                let name = id_to_name.get(&id).cloned().unwrap_or(id);
                pending_fn_responses.push(json!({
                    "functionResponse": {
                        "name": name,
                        "response": { "result": m.text().unwrap_or_default() }
                    }
                }));
            }
            "assistant" => {
                flush_fn_responses(&mut pending_fn_responses, &mut contents);
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        id_to_name.insert(tc.id.clone(), tc.function.name.clone());
                    }
                }
                contents.push(assistant_to_gemini(m));
            }
            _ => {
                flush_fn_responses(&mut pending_fn_responses, &mut contents);
                contents.push(json!({
                    "role": "user",
                    "parts": [{ "text": m.text().unwrap_or_default() }]
                }));
            }
        }
    }
    flush_fn_responses(&mut pending_fn_responses, &mut contents);

    let mut body = json!({ "contents": contents });
    if !system_parts.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": system_parts.join("\n") }] });
    }
    if let Some(tools) = &req.tools {
        let decls: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut d = json!({ "name": t.function.name });
                if let Some(desc) = &t.function.description {
                    d["description"] = json!(desc);
                }
                if let Some(params) = &t.function.parameters {
                    d["parameters"] = params.clone();
                }
                d
            })
            .collect();
        if !decls.is_empty() {
            body["tools"] = json!([{ "functionDeclarations": decls }]);
        }
    }
    if let Some(tool_config) = req.tool_choice.as_ref().and_then(tool_choice_to_gemini) {
        body["toolConfig"] = tool_config;
    }

    let mut gen_cfg = Map::new();
    if let Some(t) = cfg.temperature.or(req.temperature) {
        gen_cfg.insert("temperature".into(), json!(t));
    }
    if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
        gen_cfg.insert("maxOutputTokens".into(), json!(mt));
    }
    if !gen_cfg.is_empty() {
        body["generationConfig"] = Value::Object(gen_cfg);
    }
    body
}

fn flush_fn_responses(pending: &mut Vec<Value>, contents: &mut Vec<Value>) {
    if !pending.is_empty() {
        contents.push(json!({ "role": "user", "parts": std::mem::take(pending) }));
    }
}

fn assistant_to_gemini(m: &Message) -> Value {
    let mut parts: Vec<Value> = Vec::new();
    if let Some(t) = m.text()
        && !t.is_empty()
    {
        parts.push(json!({ "text": t }));
    }
    if let Some(tool_calls) = &m.tool_calls {
        for tc in tool_calls {
            let args: Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
            parts.push(json!({ "functionCall": { "name": tc.function.name, "args": args } }));
        }
    }
    if parts.is_empty() {
        parts.push(json!({ "text": "" }));
    }
    json!({ "role": "model", "parts": parts })
}

fn tool_choice_to_gemini(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(s) => {
            let mode = match s.as_str() {
                "required" => "ANY",
                "none" => "NONE",
                _ => "AUTO",
            };
            Some(json!({ "functionCallingConfig": { "mode": mode } }))
        }
        Value::Object(o) => o
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .map(|name| {
                json!({ "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": [name] } })
            }),
        _ => None,
    }
}

// ── Gemini → OpenAI (response) ───────────────────────────────────────────────

fn from_gemini_response(body: &Value, model: &str) -> ChatResponse {
    let candidate = &body["candidates"][0];

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut call_index = 0;
    if let Some(parts) = candidate["content"]["parts"].as_array() {
        for part in parts {
            if let Some(t) = part["text"].as_str() {
                text.push_str(t);
            } else if let Some(fc) = part.get("functionCall") {
                let name = fc["name"].as_str().unwrap_or_default().to_string();
                let arguments = fc
                    .get("args")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                // Gemini supplies no call id; synthesize a stable-within-response one.
                tool_calls.push(ToolCall {
                    id: format!("call_{name}_{call_index}"),
                    kind: "function".to_string(),
                    function: FunctionCall { name, arguments },
                    extra: Map::new(),
                });
                call_index += 1;
            }
        }
    }

    let content = if tool_calls.is_empty() {
        Some(Value::String(text))
    } else if text.is_empty() {
        Some(Value::Null)
    } else {
        Some(Value::String(text))
    };

    let finish_reason = if !tool_calls.is_empty() {
        "tool_calls"
    } else {
        match candidate["finishReason"].as_str() {
            Some("MAX_TOKENS") => "length",
            _ => "stop", // STOP / unknown
        }
    };

    let usage = body.get("usageMetadata").map(|u| Usage {
        prompt_tokens: u["promptTokenCount"].as_i64(),
        completion_tokens: u["candidatesTokenCount"].as_i64(),
        total_tokens: u["totalTokenCount"].as_i64(),
    });

    let id = body["responseId"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("gemini-{}", now_unix()));

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

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved() -> ResolvedConfig {
        ResolvedConfig {
            provider: "gemini".into(),
            model: "gemini-1.5-pro".into(),
            litellm_model: "gemini/gemini-1.5-pro".into(),
            api_key: "AIza-test".into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
        }
    }

    #[test]
    fn request_maps_system_tools_and_generation_config() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "temperature": 0.1,
            "max_tokens": 4000,
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "hi" }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "translate_text", "description": "Translate",
                    "parameters": { "type": "object", "properties": { "text": { "type": "string" } } }
                }
            }]
        }))
        .unwrap();
        let body = to_gemini_request(&req, &resolved());

        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are helpful."
        );
        assert_eq!(body["contents"].as_array().unwrap().len(), 1); // system not in contents
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "translate_text");
        assert_eq!(decl["parameters"]["properties"]["text"]["type"], "string");
        assert_eq!(body["generationConfig"]["temperature"], 0.1);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 4000);
    }

    #[test]
    fn multi_turn_maps_function_call_and_response_by_name() {
        // tool result carries tool_call_id "call_x"; Gemini needs the function name,
        // recovered from the preceding assistant tool_calls.
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "user", "content": "translate" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_x", "type": "function",
                    "function": { "name": "translate_text", "arguments": "{\"text\":\"hi\"}" }
                }]},
                { "role": "tool", "tool_call_id": "call_x", "content": "नमस्ते" }
            ]
        }))
        .unwrap();
        let body = to_gemini_request(&req, &resolved());
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        // assistant → model functionCall
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(
            contents[1]["parts"][0]["functionCall"]["name"],
            "translate_text"
        );
        assert_eq!(
            contents[1]["parts"][0]["functionCall"]["args"]["text"],
            "hi"
        );
        // tool → user functionResponse keyed by the recovered NAME, not the id
        assert_eq!(contents[2]["role"], "user");
        let fr = &contents[2]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "translate_text");
        assert_eq!(fr["response"]["result"], "नमस्ते");
    }

    #[test]
    fn response_function_call_becomes_tool_calls() {
        let gemini = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "functionCall": { "name": "translate_text", "args": { "text": "hi", "target_language": "hi" } } }
                ]},
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 20, "candidatesTokenCount": 8, "totalTokenCount": 28 }
        });
        let resp = from_gemini_response(&gemini, "gemini-1.5-pro");
        assert_eq!(resp.model, "gemini-1.5-pro");
        // STOP finishReason overridden to tool_calls because a call is present
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let msg = &resp.choices[0].message;
        assert_eq!(msg.content, Some(Value::Null));
        let tc = &msg.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.function.name, "translate_text");
        assert!(!tc.id.is_empty()); // synthesized id
        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["target_language"], "hi");
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(20));
        assert_eq!(usage.completion_tokens, Some(8));
        assert_eq!(usage.total_tokens, Some(28));
    }

    #[test]
    fn response_text_becomes_content_and_stop() {
        let gemini = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "Hello there" }] },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 2, "totalTokenCount": 7 }
        });
        let resp = from_gemini_response(&gemini, "gemini-1.5-pro");
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("Hello there".into()))
        );
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn stream_text_then_finish_and_usage() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hel\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lo\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":2,\"totalTokenCount\":6}}\n\n",
        );
        server
            .mock(
                "POST",
                "/models/gemini-1.5-pro:streamGenerateContent?alt=sse",
            )
            .match_header("x-goog-api-key", "AIza-test")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = GeminiProvider::new(reqwest::Client::new(), server.url());
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
        let finish = chunks
            .iter()
            .find_map(|c| c.choices.first().and_then(|ch| ch.finish_reason.clone()));
        assert_eq!(finish.as_deref(), Some("stop"));
        let usage = chunks.iter().find_map(|c| c.usage.clone()).unwrap();
        assert_eq!(usage.total_tokens, Some(6));
    }

    #[tokio::test]
    async fn stream_function_call_becomes_tool_call_delta() {
        let mut server = mockito::Server::new_async().await;
        let sse = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"translate_text\",\"args\":{\"text\":\"hi\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5,\"totalTokenCount\":15}}\n\n";
        server
            .mock(
                "POST",
                "/models/gemini-1.5-pro:streamGenerateContent?alt=sse",
            )
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = GeminiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(
            json!({ "stream": true, "messages": [{ "role": "user", "content": "translate" }] }),
        )
        .unwrap();
        let chunks: Vec<ChatChunk> = provider
            .chat_stream(&req, &resolved())
            .await
            .unwrap()
            .filter_map(|r| async { r.ok() })
            .collect()
            .await;

        let mut name = String::new();
        let mut args = String::new();
        for c in &chunks {
            if let Some(tcs) = c
                .choices
                .first()
                .and_then(|ch| ch.delta.tool_calls.as_ref())
            {
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
        }
        assert_eq!(name, "translate_text");
        assert_eq!(serde_json::from_str::<Value>(&args).unwrap()["text"], "hi");
        // STOP overridden to tool_calls because a call was emitted
        let finish = chunks
            .iter()
            .find_map(|c| c.choices.first().and_then(|ch| ch.finish_reason.clone()));
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }

    #[tokio::test]
    async fn embeddings_via_batch_embed_contents() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/models/gemini-1.5-pro:batchEmbedContents")
            .match_header("x-goog-api-key", "AIza-test")
            .match_body(mockito::Matcher::PartialJson(json!({
                "requests": [
                    { "model": "models/gemini-1.5-pro", "content": { "parts": [{ "text": "a" }] } },
                    { "model": "models/gemini-1.5-pro", "content": { "parts": [{ "text": "b" }] } }
                ]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({ "embeddings": [ { "values": [0.1, 0.2] }, { "values": [0.3, 0.4] } ] })
                    .to_string(),
            )
            .create_async()
            .await;

        let provider = GeminiProvider::new(reqwest::Client::new(), server.url());
        let req: EmbeddingsRequest =
            serde_json::from_value(json!({ "model": "x", "input": ["a", "b"] })).unwrap();
        let resp = provider.embeddings(&req, &resolved()).await.unwrap();
        assert_eq!(resp.object, "list");
        assert_eq!(resp.model, "gemini-1.5-pro");
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].embedding[0], 0.1);
        assert_eq!(resp.data[1].index, 1);
    }

    #[tokio::test]
    async fn chat_calls_generate_content_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/models/gemini-1.5-pro:generateContent")
            .match_header("x-goog-api-key", "AIza-test")
            .match_body(mockito::Matcher::PartialJson(json!({
                "systemInstruction": { "parts": [{ "text": "sys" }] }
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "candidates": [{ "content": { "role": "model", "parts": [{ "text": "ok" }] }, "finishReason": "STOP" }],
                    "usageMetadata": { "promptTokenCount": 3, "candidatesTokenCount": 1, "totalTokenCount": 4 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = GeminiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "sys" },
                { "role": "user", "content": "hi" }
            ]
        }))
        .unwrap();
        let resp = provider.chat(&req, &resolved()).await.unwrap();
        m.assert_async().await;
        assert_eq!(resp.model, "gemini-1.5-pro");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(4));
    }
}

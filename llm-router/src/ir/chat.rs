//! Canonical chat IR — OpenAI Chat Completions shape.
//!
//! This is the hub all providers normalize to/from. Types are **permissive**: every
//! struct keeps an `extra` map (`#[serde(flatten)]`) so unknown OpenAI fields pass
//! through untouched (the Python models' `extra="allow"`). The fields we act on are
//! named; everything else round-trips.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn function_kind() -> String {
    "function".to_string()
}
fn object_chat_completion() -> String {
    "chat.completion".to_string()
}
fn object_chat_chunk() -> String {
    "chat.completion.chunk".to_string()
}

/// Inbound chat request (OpenAI shape). `model` is accepted but **discarded** by the
/// resolver (C4); it is kept named so the OpenAI passthrough path can overwrite it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Passthrough for OpenAI params we don't enumerate (top_p, frequency_penalty, …).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ChatRequest {
    /// Whether the caller requested a streaming response (`stream: true`).
    pub fn is_streaming(&self) -> bool {
        self.stream.unwrap_or(false)
    }
}

/// An OpenAI chat message (`system` | `user` | `assistant` | `tool`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub role: String,
    /// String, array of content parts, or null — kept as raw JSON for permissiveness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Present on assistant messages that called tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Present on `tool`-role messages — the id of the call this result answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Message {
    /// Best-effort plain text of the content: a string as-is, or the concatenation of
    /// `text` parts in an array; `None` for null/other shapes.
    pub fn text(&self) -> Option<String> {
        match &self.content {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(parts)) => {
                let joined: String = parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect();
                (!joined.is_empty()).then_some(joined)
            }
            _ => None,
        }
    }
}

/// An OpenAI function-tool definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// An assistant tool call (OpenAI shape). `function.arguments` is a JSON **string**.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments serialized as a JSON string (OpenAI's contract).
    pub arguments: String,
}

/// Non-streaming chat completion response (OpenAI shape).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatResponse {
    pub id: String,
    #[serde(default = "object_chat_completion")]
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    /// Resolved bare provider-native model id (no provider prefix).
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Choice {
    pub index: i64,
    pub message: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    /// Prompt tokens served from the provider's cache. Anthropic reports
    /// `cache_read_input_tokens`; OpenAI reports `prompt_tokens_details.cached_tokens`
    /// (lifted into this flat field by [`Usage::normalize_openai_details`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i64>,
    /// Prompt tokens written into the provider's cache (Anthropic
    /// `cache_creation_input_tokens`; OpenAI doesn't expose a write count).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i64>,
    /// Cache writes subdivided by reported TTL; never additional prompt tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation: Option<CacheCreationUsage>,
    /// OpenAI nests cache counts under `usage.prompt_tokens_details`; kept so the
    /// flat `cache_read_input_tokens` can be filled from it after deserialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CacheCreationUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_5m_input_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_1h_input_tokens: Option<i64>,
}

/// OpenAI's `usage.prompt_tokens_details` — the only place OpenAI reports cache reads.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PromptTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<i64>,
}

impl Usage {
    /// Lift OpenAI's nested `prompt_tokens_details.cached_tokens` into the flat
    /// `cache_read_input_tokens` (no-op for Anthropic, which sets the flat field
    /// directly), and make `prompt_tokens` count only *uncached* input.
    ///
    /// The second half matters for billing. `calculate_token_cost` charges
    /// `input_tokens` at the full input rate AND `cache_read_input_tokens` at
    /// the cache rate, then sums them — so a prompt count that still includes
    /// cache reads is billed twice. Providers disagree on this:
    ///
    /// - OpenAI: `prompt_tokens` **includes** the cached tokens → subtract here.
    /// - Anthropic: `input_tokens` already **excludes** them → nothing to do
    ///   (it sets the flat field and never sends the nested block).
    /// - Gemini: `promptTokenCount` **includes** them, but reports the count in
    ///   `cachedContentTokenCount` rather than the nested block, so its adapter
    ///   subtracts at its own mapping site (`providers/gemini.rs`).
    ///
    /// Subtraction is keyed on the nested block being the *source* of the
    /// cache-read count, so an explicit flat value still wins and is never
    /// double-counted. The block is consumed, making repeated calls a no-op.
    pub fn normalize_openai_details(&mut self) {
        let Some(nested) = self.prompt_tokens_details.take() else {
            return;
        };
        if self.cache_read_input_tokens.is_some() {
            return;
        }
        self.cache_read_input_tokens = nested.cached_tokens;
        if let Some(cached) = nested.cached_tokens
            && let Some(prompt) = self.prompt_tokens
        {
            self.prompt_tokens = Some((prompt - cached).max(0));
        }
    }
}

/// A streaming chunk (`chat.completion.chunk`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatChunk {
    pub id: String,
    #[serde(default = "object_chat_chunk")]
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    /// Some providers emit a final usage object in a terminal chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkChoice {
    pub index: i64,
    pub delta: Delta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Delta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// A streamed tool-call fragment. `index` ties fragments to the same call; `id`/`name`
/// usually arrive once, `arguments` accumulates as partial JSON.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolCallDelta {
    pub index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FunctionCallDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_usage_lifts_nested_cached_tokens_into_flat_cache_read() {
        // OpenAI reports cache reads nested under prompt_tokens_details.
        let mut usage: Usage = serde_json::from_value(json!({
            "prompt_tokens": 100,
            "completion_tokens": 10,
            "total_tokens": 110,
            "prompt_tokens_details": { "cached_tokens": 640 }
        }))
        .unwrap();
        assert_eq!(
            usage.cache_read_input_tokens, None,
            "nested until normalized"
        );
        usage.normalize_openai_details();
        assert_eq!(usage.cache_read_input_tokens, Some(640));
        assert_eq!(usage.cache_creation_input_tokens, None);
    }

    #[test]
    fn openai_prompt_tokens_become_uncached_only() {
        // The regression this guards: `calculate_token_cost` charges
        // input_tokens at the full rate AND cache_read at the cache rate, so a
        // cache-inclusive prompt count is billed twice. gpt-4o at $2.50/$1.25
        // per 1M with 10k prompt / 8k cached was billed 10k+8k instead of
        // 2k+8k — a ~40% overstatement.
        let mut usage: Usage = serde_json::from_value(json!({
            "prompt_tokens": 10000,
            "completion_tokens": 500,
            "total_tokens": 10500,
            "prompt_tokens_details": { "cached_tokens": 8000 }
        }))
        .unwrap();
        usage.normalize_openai_details();
        assert_eq!(usage.cache_read_input_tokens, Some(8000));
        assert_eq!(
            usage.prompt_tokens,
            Some(2000),
            "prompt_tokens must exclude the cached portion"
        );
    }

    #[test]
    fn normalize_is_idempotent_and_cannot_subtract_twice() {
        let mut usage: Usage = serde_json::from_value(json!({
            "prompt_tokens": 10000,
            "prompt_tokens_details": { "cached_tokens": 8000 }
        }))
        .unwrap();
        usage.normalize_openai_details();
        usage.normalize_openai_details();
        assert_eq!(usage.prompt_tokens, Some(2000));
        assert_eq!(usage.cache_read_input_tokens, Some(8000));
    }

    #[test]
    fn anthropic_shape_prompt_tokens_are_left_alone() {
        // Anthropic's `input_tokens` already excludes cache reads, so touching
        // it would under-bill. It never sends the nested block.
        let mut usage = Usage {
            prompt_tokens: Some(2000),
            cache_read_input_tokens: Some(8000),
            ..Default::default()
        };
        usage.normalize_openai_details();
        assert_eq!(usage.prompt_tokens, Some(2000));
        assert_eq!(usage.cache_read_input_tokens, Some(8000));
    }

    #[test]
    fn cached_larger_than_prompt_clamps_to_zero() {
        // Never emit a negative token count — the cost trigger would read it
        // as a credit against the bill.
        let mut usage: Usage = serde_json::from_value(json!({
            "prompt_tokens": 100,
            "prompt_tokens_details": { "cached_tokens": 640 }
        }))
        .unwrap();
        usage.normalize_openai_details();
        assert_eq!(usage.prompt_tokens, Some(0));
    }

    #[test]
    fn normalize_keeps_explicit_flat_cache_read_over_nested() {
        // Anthropic sets the flat field directly; normalization must not clobber it.
        let mut usage = Usage {
            cache_read_input_tokens: Some(42),
            prompt_tokens_details: Some(PromptTokensDetails {
                cached_tokens: Some(999),
            }),
            ..Default::default()
        };
        usage.normalize_openai_details();
        assert_eq!(usage.cache_read_input_tokens, Some(42));
    }

    #[test]
    fn parses_openai_request_with_tools_and_preserves_unknown_fields() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "hi" }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "translate_text",
                    "description": "Translate text",
                    "parameters": { "type": "object", "properties": { "text": { "type": "string" } } }
                }
            }],
            "tool_choice": "auto",
            "temperature": 0.1,
            "max_tokens": 4000,
            "top_p": 0.9,
            "frequency_penalty": 0.5
        });
        let req: ChatRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.model.as_deref(), Some("gpt-4o"));
        assert_eq!(req.messages.len(), 2);
        assert_eq!(
            req.tools.as_ref().unwrap()[0].function.name,
            "translate_text"
        );
        assert_eq!(req.temperature, Some(0.1));
        assert!(!req.is_streaming());
        // Unknown params land in `extra` and survive a round-trip.
        assert!(req.extra.contains_key("top_p"));
        let back = serde_json::to_value(&req).unwrap();
        assert_eq!(back["top_p"], json!(0.9));
        assert_eq!(back["frequency_penalty"], json!(0.5));
    }

    #[test]
    fn parses_tool_result_and_assistant_tool_calls() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "user", "content": "translate" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_abc", "type": "function",
                    "function": { "name": "translate_text", "arguments": "{\"text\":\"hi\"}" }
                }]},
                { "role": "tool", "tool_call_id": "call_abc", "content": "नमस्ते" }
            ]
        });
        let req: ChatRequest = serde_json::from_value(body).unwrap();
        let assistant = &req.messages[1];
        let tc = &assistant.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.id, "call_abc");
        assert_eq!(tc.function.name, "translate_text");
        assert_eq!(tc.function.arguments, "{\"text\":\"hi\"}");
        let tool_msg = &req.messages[2];
        assert_eq!(tool_msg.role, "tool");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_abc"));
        assert_eq!(tool_msg.text().as_deref(), Some("नमस्ते"));
    }

    #[test]
    fn message_text_handles_string_array_and_null() {
        let s: Message = serde_json::from_value(json!({"role":"user","content":"hello"})).unwrap();
        assert_eq!(s.text().as_deref(), Some("hello"));
        let a: Message = serde_json::from_value(json!({
            "role":"user",
            "content":[{"type":"text","text":"foo"},{"type":"text","text":"bar"}]
        }))
        .unwrap();
        assert_eq!(a.text().as_deref(), Some("foobar"));
        let n: Message =
            serde_json::from_value(json!({"role":"assistant","content":null})).unwrap();
        assert_eq!(n.text(), None);
    }

    #[test]
    fn chat_response_serializes_to_openai_shape() {
        let resp = ChatResponse {
            id: "chatcmpl-1".into(),
            object: "chat.completion".into(),
            created: Some(1718500000),
            model: "claude-3-5-sonnet-20241022".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(Value::String("hi".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Map::new(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(Usage {
                prompt_tokens: Some(10),
                completion_tokens: Some(2),
                total_tokens: Some(12),
                ..Default::default()
            }),
            extra: Map::new(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["model"], "claude-3-5-sonnet-20241022");
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["total_tokens"], 12);
    }

    #[test]
    fn chat_chunk_serializes_tool_call_delta() {
        let chunk = ChatChunk {
            id: "chatcmpl-1".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "gpt-4o".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ToolCallDelta {
                        index: 0,
                        id: Some("call_1".into()),
                        kind: Some("function".into()),
                        function: Some(FunctionCallDelta {
                            name: Some("f".into()),
                            arguments: Some("{\"a\":".into()),
                        }),
                    }]),
                },
                finish_reason: None,
            }],
            usage: None,
            extra: Map::new(),
        };
        let v = serde_json::to_value(&chunk).unwrap();
        assert_eq!(v["object"], "chat.completion.chunk");
        let d = &v["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(d["index"], 0);
        assert_eq!(d["function"]["arguments"], "{\"a\":");
    }

    #[test]
    fn compact_tool_conversions() {
        use nasiko_tool_compact::{ToolCall as CompactToolCall, ToolDef as CompactToolDef};

        let router_tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "test_tool".to_string(),
                description: Some("a test tool".to_string()),
                parameters: Some(json!({"type": "object"})),
            },
            extra: Map::new(),
        };

        // Convert router ToolDef -> compact ToolDef
        let compact_tool: CompactToolDef = (&router_tool).into();
        assert_eq!(compact_tool.name, "test_tool");
        assert_eq!(compact_tool.description.as_deref(), Some("a test tool"));

        // Convert compact ToolCall -> router ToolCall
        let compact_call = CompactToolCall {
            name: "test_tool".to_string(),
            arguments: json!({"key": "val"}),
        };
        let router_call = ToolCall::from_compact(&compact_call, "call_123");
        assert_eq!(router_call.id, "call_123");
        assert_eq!(router_call.function.name, "test_tool");
        assert_eq!(router_call.function.arguments, "{\"key\":\"val\"}");
    }
}

// ─── nasiko-tool-compact Seam Conversions ───────────────────────────────────

impl From<&ToolDef> for nasiko_tool_compact::ToolDef {
    fn from(t: &ToolDef) -> Self {
        Self {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        }
    }
}

impl From<ToolDef> for nasiko_tool_compact::ToolDef {
    fn from(t: ToolDef) -> Self {
        Self {
            name: t.function.name,
            description: t.function.description,
            parameters: t.function.parameters,
        }
    }
}

impl ToolCall {
    /// Convert a compact tool call into router ToolCall, assigning a router ID.
    pub fn from_compact(call: &nasiko_tool_compact::ToolCall, id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: function_kind(),
            function: FunctionCall {
                name: call.name.clone(),
                arguments: serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string()),
            },
            extra: Map::new(),
        }
    }
}


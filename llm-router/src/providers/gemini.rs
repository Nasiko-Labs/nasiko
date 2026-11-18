//! Gemini provider — OpenAI ⇄ Gemini `generateContent` translation.
//!
//! Request (C1/C3): `system` → `systemInstruction`; user → `contents[role:user]`;
//! assistant text/`tool_calls` → `contents[role:model]` with `text`/`functionCall`
//! parts; `{role:"tool"}` results → a `contents[role:user]` with `functionResponse`
//! parts. Gemini keys `functionResponse` by function **name**, not the OpenAI
//! `tool_call_id`, so we track id→name across the history. `tools[].function` →
//! `tools[0].functionDeclarations[]`. Brevity's directive ends the last user turn rather than
//! joining `systemInstruction` ([`crate::brevity`]). The agent's `cache_control` markers
//! become an explicit `cachedContents` cache of the marked prefix (`providers/gemini_cache.rs`).
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

use super::gemini_cache::{self, CachePlan};
use super::media::{self, DocumentKind, document_kind};
use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError, delta_chunk, finish_chunk, now_unix, usage_chunk};
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, Choice, ContentPart, ContentView, DataSource, Delta,
    Embedding, EmbeddingsRequest, EmbeddingsResponse, FunctionCall, FunctionCallDelta, Message,
    ToolCall, ToolCallDelta, UnsupportedPart, Usage,
};
use crate::resolver::{ResolvedConfig, ResolvedEmbeddingConfig};

pub struct GeminiProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://generativelanguage.googleapis.com/v1beta`.
    base: String,
    /// Cache the agent's marked prefix as a `cachedContents` resource (`gemini_cache`).
    explicit_cache: bool,
    /// Vertex mode: bearer token from ADC instead of the `x-goog-api-key` header.
    adc: Option<std::sync::Arc<super::vertex_auth::TokenSource>>,
}

impl GeminiProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self {
            http,
            base,
            explicit_cache: false,
            adc: None,
        }
    }

    /// Vertex AI: authenticate with ADC bearer tokens. Explicit caching stays off
    /// (`cachedContents` is API-key only here).
    pub fn with_adc(mut self, tokens: std::sync::Arc<super::vertex_auth::TokenSource>) -> Self {
        self.adc = Some(tokens);
        self.explicit_cache = false;
        self
    }

    async fn authed(
        &self,
        builder: reqwest::RequestBuilder,
        api_key: &str,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        match &self.adc {
            Some(tokens) => Ok(builder.bearer_auth(tokens.token().await?)),
            None => Ok(builder.header("x-goog-api-key", api_key)),
        }
    }

    /// Turn explicit caching of the agent's marked prefix on or off (`TOKEN_CACHE_MARKERS`).
    pub fn with_explicit_cache(mut self, enabled: bool) -> Self {
        self.explicit_cache = enabled;
        self
    }

    /// POST the request to `models/{model}:{method}`, naming a cache of its marked prefix when
    /// one applies. A cache Gemini will not take back is refused and the request resent whole,
    /// so caching never fails a call.
    async fn generate(
        &self,
        method: &str,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<reqwest::Response, ProviderError> {
        let request = translate(req, cfg)?;
        let url = format!("{}/models/{}:{}", self.base, cfg.model, method);
        if let Some(cached) = self.cached(&request, cfg).await {
            let resp = self.post(&url, cfg, &cached.body).await?;
            if !refuses_cached_content(resp.status()) {
                return require_success(resp).await;
            }
            tracing::warn!(
                target: "nasiko::llm_router::gemini",
                model = %cfg.model, status = resp.status().as_u16(),
                "gemini: request naming an explicit cache was refused; resending it uncached"
            );
            gemini_cache::refuse(&cached);
        }
        require_success(self.post(&url, cfg, &request.body).await?).await
    }

    async fn cached(
        &self,
        request: &GeminiRequest,
        cfg: &ResolvedConfig,
    ) -> Option<gemini_cache::CachedRequest> {
        let plan = request
            .cache_plan
            .as_ref()
            .filter(|_| self.explicit_cache)?;
        gemini_cache::use_cached_prefix(&self.http, &self.base, cfg, &request.body, plan).await
    }

    async fn post(
        &self,
        url: &str,
        cfg: &ResolvedConfig,
        body: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        self.authed(self.http.post(url), &cfg.api_key)
            .await?
            .json(body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))
    }
}

/// The statuses Gemini answers a stale or foreign `cachedContent` with.
fn refuses_cached_content(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 400 | 403 | 404)
}

async fn require_success(resp: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let text = resp.text().await.unwrap_or_default();
    Err(ProviderError::Status {
        status: status.as_u16(),
        message: text,
        retryable: status.as_u16() == 429 || status.is_server_error(),
    })
}

#[async_trait]
impl ProviderClient for GeminiProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let resp = self.generate("generateContent", req, cfg).await?;
        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        from_gemini_response(&value, &cfg.model)
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let resp = self
            .generate("streamGenerateContent?alt=sse", req, cfg)
            .await?;
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
                    Err(error) => {
                        yield Err(ProviderError::Parse(format!("invalid Gemini stream event: {error}")));
                        return;
                    }
                };
                if let Some(error) = response.get("error") {
                    let status = error["code"].as_u64().and_then(|code| u16::try_from(code).ok()).unwrap_or(502);
                    let message = error["message"].as_str().unwrap_or("Gemini stream error");
                    yield Err(ProviderError::Status {
                        status,
                        message: message.to_string(),
                        retryable: status == 429 || status >= 500,
                    });
                    return;
                }
                if let Some(um) = response.get("usageMetadata") {
                    let cached = um["cachedContentTokenCount"].as_i64();
                    usage = Some(Usage {
                        prompt_tokens: uncached_prompt(um["promptTokenCount"].as_i64(), cached),
                        completion_tokens: output_tokens(um),
                        total_tokens: um["totalTokenCount"].as_i64(),
                        cache_read_input_tokens: cached,
                        cache_creation_input_tokens: None,
                        cache_creation: None,
                        prompt_tokens_details: None,
                    });
                }
                let Some(candidate) = response["candidates"].as_array().and_then(|c| c.first()) else {
                    if response.get("usageMetadata").is_some() {
                        continue;
                    }
                    yield Err(ProviderError::Parse("Gemini stream event has no candidate".into()));
                    return;
                };

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
                            let mut extra = Map::new();
                            if let Some(sig) = part.get("thoughtSignature").and_then(|v| v.as_str()) {
                                extra.insert(
                                    "thought_signature".to_string(),
                                    Value::String(sig.to_string()),
                                );
                            }
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
                                    extra,
                                }]),
                                ..Delta::default()
                            }));
                        }
                    }
                }

                if let Some(fr) = candidate["finishReason"].as_str() {
                    match map_finish_reason(candidate, fr) {
                        Ok(mapped) => finish = Some(mapped.to_string()),
                        Err(error) => { yield Err(error); return; }
                    }
                }
                if let Some(um) = response.get("usageMetadata") {
                    let cached = um["cachedContentTokenCount"].as_i64();
                    usage = Some(Usage {
                        // promptTokenCount INCLUDES cachedContentTokenCount; the
                        // cost trigger charges input and cache-read separately,
                        // so report only the uncached remainder (see
                        // `Usage::normalize_openai_details`).
                        prompt_tokens: uncached_prompt(um["promptTokenCount"].as_i64(), cached),
                        completion_tokens: output_tokens(um),
                        total_tokens: um["totalTokenCount"].as_i64(),
                        cache_read_input_tokens: cached,
                        cache_creation_input_tokens: None,
            cache_creation: None,
                        prompt_tokens_details: None,
                    });
                }
            }

            let Some(finish) = finish else {
                yield Err(ProviderError::Parse("Gemini stream ended without finishReason".into()));
                return;
            };
            let finish_reason = if saw_tool_call {
                match finish.as_str() {
                    "stop" => "tool_calls".to_string(),
                    _ => finish,
                }
            } else {
                finish
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
        cfg: &ResolvedEmbeddingConfig,
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
        if self.adc.is_some() {
            return self.vertex_predict_embeddings(&inputs, cfg).await;
        }
        let model_path = format!("models/{}", cfg.model);
        let requests: Vec<Value> = inputs
            .iter()
            .map(|text| json!({ "model": model_path, "content": { "parts": [{ "text": text }] } }))
            .collect();

        let url = format!("{}/models/{}:batchEmbedContents", self.base, cfg.model);
        let mut data: Vec<Embedding> = Vec::with_capacity(inputs.len());
        let mut offset = 0usize;
        for chunk in requests.chunks(100) {
            let resp = self
                .authed(self.http.post(&url), &cfg.api_key)
                .await?
                .json(&json!({ "requests": chunk }))
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
            if let Some(embs) = value["embeddings"].as_array() {
                for (i, e) in embs.iter().enumerate() {
                    data.push(Embedding {
                        object: "embedding".to_string(),
                        embedding: e["values"].clone(),
                        index: (offset + i) as i64,
                        extra: Map::new(),
                    });
                }
            }
            offset += chunk.len();
        }

        // Gemini's batch embed API returns no token counts; estimate so usage
        // rows are billed instead of recording zero input tokens.
        let estimated: i64 = estimate_embedding_tokens(&inputs);
        Ok(EmbeddingsResponse {
            object: "list".to_string(),
            data,
            model: cfg.model.clone(),
            usage: Some(Usage {
                prompt_tokens: Some(estimated),
                total_tokens: Some(estimated),
                ..Usage::default()
            }),
            extra: Map::new(),
        })
    }
}

/// Approximate token count for embedding inputs (~4 chars per token, at least
/// one token per non-empty text). Used when the provider reports no usage.
fn estimate_embedding_tokens(inputs: &[String]) -> i64 {
    inputs
        .iter()
        .map(|text| {
            let chars: i64 = text.chars().count() as i64;
            if chars == 0 { 0 } else { (chars + 3) / 4 }
        })
        .sum()
}

impl GeminiProvider {
    /// Vertex AI serves embeddings via `models/{model}:predict`, not `:batchEmbedContents`.
    async fn vertex_predict_embeddings(
        &self,
        inputs: &[String],
        cfg: &ResolvedEmbeddingConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        let url = format!("{}/models/{}:predict", self.base, cfg.model);
        let resp = self
            .authed(self.http.post(url), &cfg.api_key)
            .await?
            .json(&vertex_predict_body(inputs))
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
        parse_vertex_predict(&value, &cfg.model)
    }
}

/// Vertex `:predict` request body: one instance per input text.
fn vertex_predict_body(inputs: &[String]) -> Value {
    let instances: Vec<Value> = inputs.iter().map(|t| json!({ "content": t })).collect();
    json!({ "instances": instances })
}

/// Vertex `:predict` response (`predictions[].embeddings.values`) → OpenAI shape.
fn parse_vertex_predict(value: &Value, model: &str) -> Result<EmbeddingsResponse, ProviderError> {
    let predictions: &Vec<Value> = value["predictions"]
        .as_array()
        .ok_or_else(|| ProviderError::Parse("Vertex predict response has no predictions".into()))?;
    let mut data: Vec<Embedding> = Vec::with_capacity(predictions.len());
    // Billable input tokens: sum of `embeddings.statistics.token_count` per prediction.
    let mut token_count: Option<i64> = None;
    for (i, p) in predictions.iter().enumerate() {
        if let Some(n) = p["embeddings"]["statistics"]["token_count"].as_f64() {
            token_count = Some(token_count.unwrap_or(0) + n as i64);
        }
        let values: &Value = &p["embeddings"]["values"];
        if !values.is_array() {
            return Err(ProviderError::Parse(format!(
                "Vertex prediction {i} has no embeddings.values"
            )));
        }
        data.push(Embedding {
            object: "embedding".to_string(),
            embedding: values.clone(),
            index: i as i64,
            extra: Map::new(),
        });
    }
    Ok(EmbeddingsResponse {
        object: "list".to_string(),
        data,
        model: model.to_string(),
        usage: token_count.map(|n| Usage {
            prompt_tokens: Some(n),
            total_tokens: Some(n),
            ..Usage::default()
        }),
        extra: Map::new(),
    })
}

#[cfg(test)]
mod vertex_predict_tests {
    use super::*;

    #[test]
    fn body_has_one_instance_per_text() {
        let body: Value = vertex_predict_body(&["a".into(), "b".into()]);
        assert_eq!(body, json!({"instances":[{"content":"a"},{"content":"b"}]}));
    }

    #[test]
    fn parses_predictions_in_order() {
        let v: Value = json!({"predictions":[
            {"embeddings":{"values":[0.1,0.2],"statistics":{"token_count":1}}},
            {"embeddings":{"values":[0.3,0.4],"statistics":{"token_count":1}}}
        ]});
        let r: EmbeddingsResponse = parse_vertex_predict(&v, "text-embedding-005").unwrap();
        assert_eq!(r.data.len(), 2);
        assert_eq!(r.data[1].index, 1);
        assert_eq!(r.data[1].embedding, json!([0.3, 0.4]));
        assert_eq!(r.model, "text-embedding-005");
    }

    /// Live: `VERTEX_LIVE_PROJECT=<p> cargo test ... vertex_live -- --ignored --nocapture`.
    /// Reads `~/.config/gcloud/application_default_credentials.json`.
    #[tokio::test]
    #[ignore]
    async fn vertex_live() {
        let loc: String = std::env::var("VERTEX_LIVE_LOCATION").unwrap_or("global".into());
        let home: String = std::env::var("HOME").unwrap();
        let path: String = format!("{home}/.config/gcloud/application_default_credentials.json");
        let json_text: String = std::fs::read_to_string(&path).expect("ADC file");
        let adc: Value = serde_json::from_str(&json_text).unwrap();
        let project: String = std::env::var("VERTEX_LIVE_PROJECT")
            .ok()
            .or_else(|| adc["quota_project_id"].as_str().map(str::to_string))
            .unwrap_or_else(|| "nasiko-engineer".into());
        println!("project {project}, location {loc}");
        let http = reqwest::Client::new();
        let tokens = std::sync::Arc::new(
            crate::providers::vertex_auth::TokenSource::from_credentials_json(
                http.clone(),
                &json_text,
            )
            .unwrap(),
        );
        let token: String = tokens.token().await.expect("ADC token");
        println!("token ok ({} chars)", token.len());
        let host: String = if loc == "global" {
            "aiplatform.googleapis.com".into()
        } else {
            format!("{loc}-aiplatform.googleapis.com")
        };
        let base: String =
            format!("https://{host}/v1/projects/{project}/locations/{loc}/publishers/google");

        let chat: reqwest::Response = http
            .post(format!("{base}/models/gemini-2.5-flash:generateContent"))
            .bearer_auth(&token)
            .json(&json!({"contents":[{"role":"user","parts":[{"text":"Say ok"}]}]}))
            .send()
            .await
            .unwrap();
        let chat_status = chat.status();
        let chat_body: String = chat.text().await.unwrap();
        println!(
            "gemini chat {chat_status}: {}",
            &chat_body[..chat_body.len().min(300)]
        );
        let _ = chat_status;

        let provider = GeminiProvider::new(http.clone(), base.clone()).with_adc(tokens.clone());
        let emb: EmbeddingsResponse = provider
            .vertex_predict_embeddings(&["hello".into(), "world".into()], &live_emb_cfg())
            .await
            .expect("embeddings");
        let dims: usize = emb.data[0]
            .embedding
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        println!("embeddings: {} vectors, {dims} dims", emb.data.len());
        assert_eq!(emb.data.len(), 2);
        assert!(dims > 0);

        let ds_loc: String =
            std::env::var("VERTEX_LIVE_DEEPSEEK_LOCATION").unwrap_or("us-central1".into());
        let ds: reqwest::Response = http
            .post(format!(
                "https://{ds_loc}-aiplatform.googleapis.com/v1/projects/{project}/locations/{ds_loc}/endpoints/openapi/chat/completions"
            ))
            .bearer_auth(&token)
            .json(&json!({"model":"deepseek-ai/deepseek-v3.1-maas","messages":[{"role":"user","content":"Say ok"}]}))
            .send().await.unwrap();
        let ds_status = ds.status();
        let ds_body: String = ds.text().await.unwrap();
        println!(
            "deepseek (openapi) {ds_status}: {}",
            &ds_body[..ds_body.len().min(300)]
        );
    }

    fn live_emb_cfg() -> ResolvedEmbeddingConfig {
        ResolvedEmbeddingConfig::for_test("vertex", "text-embedding-005", "adc")
    }

    #[test]
    fn missing_values_is_parse_error() {
        let v: Value = json!({"predictions":[{"embeddings":{}}]});
        assert!(matches!(
            parse_vertex_predict(&v, "m"),
            Err(ProviderError::Parse(_))
        ));
        assert!(parse_vertex_predict(&json!({}), "m").is_err());
    }
}

// ── OpenAI → Gemini (request) ────────────────────────────────────────────────

/// Gemini's `promptTokenCount` is **cache-inclusive**: it counts the tokens
/// served from `cachedContentTokenCount` as well as the fresh ones. The billing
/// trigger (`calculate_token_cost`) charges `input_tokens` at the full input
/// rate and adds `cache_read_input_tokens` at the cache rate, so passing the
/// gross count through bills the cached portion twice. Report the remainder.
///
/// Clamped at zero: the subtraction should never go negative, but a provider
/// reporting a larger cached count than prompt count must not produce a
/// negative token count that the cost trigger would read as a credit.
/// Billable output: `candidatesTokenCount` plus `thoughtsTokenCount`. Gemini 3/4
/// thinking tokens are billed as output but excluded from the candidates count.
fn output_tokens(usage: &Value) -> Option<i64> {
    let candidates: Option<i64> = usage["candidatesTokenCount"].as_i64();
    let thoughts: Option<i64> = usage["thoughtsTokenCount"].as_i64();
    match (candidates, thoughts) {
        (None, None) => None,
        (c, t) => Some(c.unwrap_or(0) + t.unwrap_or(0)),
    }
}

fn uncached_prompt(prompt: Option<i64>, cached: Option<i64>) -> Option<i64> {
    match (prompt, cached) {
        (Some(prompt), Some(cached)) => Some((prompt - cached).max(0)),
        (prompt, _) => prompt,
    }
}

const GEMINI_IMAGE_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/heic",
    "image/heif",
];
const GEMINI_INLINE_REQUEST_MAX_BYTES: usize = 20_000_000;

fn inline_data_bytes(contents: &[Value]) -> usize {
    contents
        .iter()
        .filter_map(|turn| turn["parts"].as_array())
        .flatten()
        .filter_map(|part| part["inlineData"]["data"].as_str())
        .map(str::len)
        .sum()
}

/// A part the hub carries but Gemini cannot, rendered onto this edge's error type.
fn unsupported(err: UnsupportedPart) -> ProviderError {
    ProviderError::Rejected(format!(
        "Gemini does not accept this input: {}",
        err.into_message()
    ))
}

fn empty_user_content() -> ProviderError {
    ProviderError::Rejected("user message content cannot be empty".to_string())
}

/// Canonical part → Gemini part.
///
/// `generateContent` carries media only as `inlineData` bytes — it will not fetch a URL
/// on the caller's behalf — so every media arm goes through [`DataSource::inline`] and a
/// URL source is refused here rather than sent as an empty part. That rule belongs to
/// this spoke: the same URL is perfectly valid on the Anthropic edge.
fn gemini_part(part: &ContentPart) -> Result<Value, UnsupportedPart> {
    let inline = |source: &DataSource, message: &str| -> Result<Value, UnsupportedPart> {
        let (mime, data) = source
            .inline()
            .ok_or_else(|| UnsupportedPart::new(message))?;
        Ok(json!({ "inlineData": { "mimeType": mime, "data": data } }))
    };
    match part {
        ContentPart::Text(text) => Ok(json!({ "text": text })),
        ContentPart::Image { source, .. } => {
            if let Some(media_type) = source.media_type()
                && !GEMINI_IMAGE_TYPES.contains(&media_type)
            {
                return Err(UnsupportedPart::new(format!(
                    "'{media_type}' images (it reads png, jpeg, webp, heic and heif)"
                )));
            }
            inline(
                source,
                "unsupported content part type: 'image_url' with web URL (Gemini requires base64 data URL)",
            )
        }
        ContentPart::File { source, filename } => match source.media_type().map(document_kind) {
            Some(DocumentKind::TextLike) => Ok(json!({
                "text": media::text_document(source, filename.as_deref())?
            })),
            Some(DocumentKind::Other) => Err(UnsupportedPart::new(format!(
                "'{}' documents (it reads PDF and text documents)",
                source.media_type().unwrap_or_default()
            ))),
            _ => inline(
                source,
                "unsupported file_data format (must be base64 data URL)",
            ),
        },
        ContentPart::Audio { data, format } => Ok(json!({
            "inlineData": { "mimeType": format!("audio/{format}"), "data": data },
        })),
        ContentPart::Video { source } => inline(
            source,
            "unsupported content part type: 'video_url' with web URL (Gemini requires base64 data URL)",
        ),
    }
}

/// Splits a tool result into the text that goes *inside* `functionResponse.response`
/// and the media parts that must travel *beside* it.
///
/// This is not a straight translation of the Anthropic path. Anthropic's `tool_result`
/// takes a content-block array, so an image simply goes in it. Gemini's
/// `functionResponse.response` is plain JSON with nowhere to put bytes, so the text goes
/// in `response.result` and the media becomes sibling parts of the same turn — which is
/// where Gemini actually reads them from.
fn split_tool_result(m: &Message) -> Result<(String, Vec<Value>), ProviderError> {
    match m.content_view().map_err(unsupported)? {
        ContentView::Text(text) => Ok((text, Vec::new())),
        ContentView::Empty => Ok((String::new(), Vec::new())),
        ContentView::Parts(parts) => {
            // Text parts concatenate with no separator, matching Message::text().
            let text = parts
                .iter()
                .filter_map(ContentPart::non_empty_text)
                .collect::<String>();
            let media = parts
                .iter()
                .filter(|p| !matches!(p, ContentPart::Text(_)))
                .map(|p| gemini_part(p).map_err(unsupported))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((text, media))
        }
    }
}

fn gemini_parts(parts: &[ContentPart]) -> Result<Vec<Value>, ProviderError> {
    parts
        .iter()
        .map(|p| gemini_part(p).map_err(unsupported))
        .collect()
}

/// A translated `generateContent` body, and how much of it the agent marked for caching.
struct GeminiRequest {
    body: Value,
    /// `None` when the request carries no cache marker.
    cache_plan: Option<CachePlan>,
}

#[cfg(test)]
fn to_gemini_request(req: &ChatRequest, cfg: &ResolvedConfig) -> Result<Value, ProviderError> {
    translate(req, cfg).map(|request| request.body)
}

fn translate(req: &ChatRequest, cfg: &ResolvedConfig) -> Result<GeminiRequest, ProviderError> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    let mut id_to_name: HashMap<String, String> = HashMap::new();
    let mut pending_fn_responses: Vec<Value> = Vec::new();
    // How many leading `contents` the agent's last marker covers. The system instruction and
    // the tools are always part of a cached prefix, so a marker on them covers none.
    let mut marked_turns: Option<usize> = req
        .tools
        .iter()
        .flatten()
        .any(|tool| tool.extra.contains_key(crate::ir::CACHE_CONTROL))
        .then_some(0);

    for m in &req.messages {
        if m.carries_cache_marker() {
            let through = turns_through(m, contents.len(), !pending_fn_responses.is_empty());
            marked_turns = Some(marked_turns.unwrap_or(0).max(through));
        }
        match m.role.as_str() {
            "system" => {
                // Brevity's directive goes after the conversation, so a turn that skips it still
                // shares this turn's prefix for implicit caching; `systemInstruction` leads it.
                let placed = crate::brevity::is_directive(m)
                    && pending_fn_responses.is_empty()
                    && end_user_turn_with(contents.last_mut(), crate::brevity::DIRECTIVE);
                if placed {
                    continue;
                }
                if let Some(t) = m.text() {
                    system_parts.push(t);
                }
            }
            "tool" => {
                let id = m.tool_call_id.clone().unwrap_or_default();
                // Gemini wants the function name; recover it from the call this answers.
                let name = id_to_name.get(&id).cloned().unwrap_or(id);
                let (result, media) = split_tool_result(m)?;
                pending_fn_responses.push(json!({
                    "functionResponse": {
                        "name": name,
                        "response": { "result": result }
                    }
                }));
                // Media rides beside the functionResponse, not inside it — see
                // split_tool_result. flush_fn_responses collects the whole pending run
                // into one user turn, so these land as siblings of the response.
                pending_fn_responses.extend(media);
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
                let parts = match m.content_view().map_err(unsupported)? {
                    ContentView::Text(s) if !s.is_empty() => vec![json!({ "text": s })],
                    ContentView::Parts(parts) if !parts.is_empty() => gemini_parts(&parts)?,
                    _ => return Err(empty_user_content()),
                };
                contents.push(json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
        }
    }
    flush_fn_responses(&mut pending_fn_responses, &mut contents);
    if inline_data_bytes(&contents) > GEMINI_INLINE_REQUEST_MAX_BYTES {
        return Err(unsupported(UnsupportedPart::new(
            "more than 20 MB of inline data in one request",
        )));
    }

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
                // `parametersJsonSchema` takes standard JSON Schema as is. `parameters` takes only
                // an OpenAPI subset and 400s on keys Pydantic, Zod and MCP emit ($schema, $defs,
                // additionalProperties, exclusiveMinimum, …). The two are mutually exclusive.
                if let Some(params) = &t.function.parameters {
                    d["parametersJsonSchema"] = params.clone();
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
    // OpenAI `stop` is a string or an array of strings, and not a named IR field.
    let stop: Vec<&str> = match req.extra.get("stop") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !stop.is_empty() {
        gen_cfg.insert("stopSequences".into(), json!(stop));
    }
    // OpenAI `response_format` → Gemini JSON mode. `json_schema` carries a standard JSON
    // Schema, which `responseJsonSchema` accepts as is (unlike the OpenAPI-subset
    // `responseSchema`).
    if let Some(rf) = req.extra.get("response_format") {
        match rf.get("type").and_then(Value::as_str) {
            Some("json_object") => {
                gen_cfg.insert("responseMimeType".into(), json!("application/json"));
            }
            Some("json_schema") => {
                gen_cfg.insert("responseMimeType".into(), json!("application/json"));
                if let Some(schema) = rf.pointer("/json_schema/schema") {
                    gen_cfg.insert("responseJsonSchema".into(), schema.clone());
                }
            }
            _ => {}
        }
    }
    if !gen_cfg.is_empty() {
        body["generationConfig"] = Value::Object(gen_cfg);
    }
    Ok(GeminiRequest {
        body,
        cache_plan: marked_turns.map(|turns| CachePlan::new(turns, req.cache_markers())),
    })
}

/// How many `contents` exist once `m` is translated: a system message joins
/// `systemInstruction`, a tool result joins the pending turn of function responses, and a user
/// or assistant message first flushes that turn, then adds its own.
fn turns_through(m: &Message, emitted: usize, pending: bool) -> usize {
    match m.role.as_str() {
        "system" => emitted,
        "tool" => emitted + 1,
        _ => emitted + usize::from(pending) + 1,
    }
}

fn flush_fn_responses(pending: &mut Vec<Value>, contents: &mut Vec<Value>) {
    if !pending.is_empty() {
        contents.push(json!({ "role": "user", "parts": std::mem::take(pending) }));
    }
}

/// Add `text` as a part at the end of `turn` when it is a user turn; `false` when it is not, or
/// there is none.
fn end_user_turn_with(turn: Option<&mut Value>, text: &str) -> bool {
    match turn
        .filter(|t| t["role"] == "user")
        .and_then(|t| t["parts"].as_array_mut())
    {
        Some(parts) => {
            parts.push(json!({ "text": text }));
            true
        }
        None => false,
    }
}

/// Dummy signature Gemini accepts when the real one was lost (ai.google.dev thought-signatures docs).
const SKIP_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

fn assistant_to_gemini(m: &Message) -> Value {
    let mut parts: Vec<Value> = Vec::new();
    if let Some(t) = m.text()
        && !t.is_empty()
    {
        parts.push(json!({ "text": t }));
    }
    if let Some(tool_calls) = &m.tool_calls {
        // Gemini 3+/thinking models reject a model turn whose first functionCall lacks a
        // thoughtSignature. Clients that rebuild history (Responses API, compacted tools,
        // stored chats) drop it, so send Google's documented bypass value instead.
        let has_sig: bool = tool_calls.iter().any(|tc| {
            tc.extra
                .get("thought_signature")
                .and_then(|v| v.as_str())
                .is_some()
        });
        for (i, tc) in tool_calls.iter().enumerate() {
            let args: Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
            let mut fc_part = json!({ "functionCall": { "name": tc.function.name, "args": args } });
            if let Some(sig) = tc.extra.get("thought_signature").and_then(|v| v.as_str()) {
                fc_part["thoughtSignature"] = Value::String(sig.to_string());
            } else if i == 0 && !has_sig {
                fc_part["thoughtSignature"] = Value::String(SKIP_THOUGHT_SIGNATURE.to_string());
            }
            parts.push(fc_part);
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

fn from_gemini_response(body: &Value, model: &str) -> Result<ChatResponse, ProviderError> {
    let candidate = body["candidates"]
        .as_array()
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| ProviderError::Parse("Gemini response has no candidates".into()))?;

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
                let mut extra = Map::new();
                if let Some(sig) = part.get("thoughtSignature").and_then(|v| v.as_str()) {
                    extra.insert(
                        "thought_signature".to_string(),
                        Value::String(sig.to_string()),
                    );
                }
                // Gemini supplies no call id; synthesize a stable-within-response one.
                tool_calls.push(ToolCall {
                    id: format!("call_{name}_{call_index}"),
                    kind: "function".to_string(),
                    function: FunctionCall { name, arguments },
                    extra,
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

    let provider_finish = candidate["finishReason"]
        .as_str()
        .ok_or_else(|| ProviderError::Parse("Gemini response has no finishReason".into()))?;
    let mapped_finish = map_finish_reason(candidate, provider_finish)?;
    let finish_reason = if !tool_calls.is_empty() && mapped_finish == "stop" {
        "tool_calls"
    } else {
        mapped_finish
    };

    let usage = body.get("usageMetadata").map(|u| {
        let cached = u["cachedContentTokenCount"].as_i64();
        Usage {
            // See `uncached_prompt` — promptTokenCount is cache-inclusive.
            prompt_tokens: uncached_prompt(u["promptTokenCount"].as_i64(), cached),
            completion_tokens: output_tokens(u),
            total_tokens: u["totalTokenCount"].as_i64(),
            cache_read_input_tokens: cached,
            cache_creation_input_tokens: None,
            cache_creation: None,
            prompt_tokens_details: None,
        }
    });

    let id = body["responseId"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("gemini-{}", now_unix()));

    Ok(ChatResponse {
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
    })
}

/// Gemini `finishReason` → OpenAI `finish_reason`. `MALFORMED_FUNCTION_CALL` is the model
/// failing to produce a valid tool call, not a block, so it is an error carrying Gemini's
/// `finishMessage` rather than `content_filter`.
fn map_finish_reason(candidate: &Value, reason: &str) -> Result<&'static str, ProviderError> {
    match reason {
        "STOP" => Ok("stop"),
        "MAX_TOKENS" => Ok("length"),
        "MALFORMED_FUNCTION_CALL" => Err(ProviderError::Parse(format!(
            "Gemini returned a malformed function call: {}",
            candidate["finishMessage"]
                .as_str()
                .unwrap_or("no finishMessage")
        ))),
        // Gemini adds blocking reasons over time. Anything except the two successful
        // terminals above must fail closed rather than become an executable response.
        _ => Ok("content_filter"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved() -> ResolvedConfig {
        ResolvedConfig {
            agent_id: String::new(),
            compress_enabled: false,
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
            custom_endpoint: None,
            is_coding_agent: false,
            is_discovered_agent: false,
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
        let body = to_gemini_request(&req, &resolved()).unwrap();

        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are helpful."
        );
        assert_eq!(body["contents"].as_array().unwrap().len(), 1); // system not in contents
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "translate_text");
        assert_eq!(
            decl["parametersJsonSchema"]["properties"]["text"]["type"],
            "string"
        );
        assert_eq!(body["generationConfig"]["temperature"], 0.1);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 4000);
    }

    #[test]
    fn a_turn_that_skips_brevity_shares_the_prefix_of_one_that_applied_it() {
        // Gemini caches implicitly, by prefix. In `systemInstruction` the directive would change
        // the very start of every turn that applies or skips it.
        let cfg = crate::config::GatewayConfig {
            brevity_enabled: true,
            brevity_min_bytes: 0,
            ..Default::default()
        };
        let brevity_on = ResolvedConfig {
            compress_enabled: true,
            ..resolved()
        };
        let tool = json!({ "type": "function", "function": { "name": "logs" } });
        let mut first: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "why did the deploy fail?" }
            ],
            "tools": [tool]
        }))
        .unwrap();
        crate::brevity::apply(&mut first, &cfg, &brevity_on, None).unwrap();
        let mut second: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "why did the deploy fail?" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": { "name": "logs", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "call_1", "content": "exit 1" }
            ],
            "tools": [tool]
        }))
        .unwrap();
        assert_eq!(
            crate::brevity::apply(&mut second, &cfg, &brevity_on, None),
            Err(crate::brevity::Skipped::ToolContinuation)
        );

        let first = to_gemini_request(&first, &resolved()).unwrap();
        let second = to_gemini_request(&second, &resolved()).unwrap();

        assert_eq!(first["systemInstruction"], second["systemInstruction"]);
        assert_eq!(first["tools"], second["tools"]);
        assert_eq!(
            first["contents"][0]["parts"],
            json!([{ "text": "why did the deploy fail?" }, { "text": crate::brevity::DIRECTIVE }])
        );
        assert_eq!(
            second["contents"][0]["parts"],
            json!([{ "text": "why did the deploy fail?" }])
        );
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
        let body = to_gemini_request(&req, &resolved()).unwrap();
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
    fn mixed_assistant_text_and_multiple_calls_round_trip_to_gemini() {
        let req: ChatRequest = serde_json::from_value(json!({
            "messages":[
                {"role":"user","content":"go"},
                {"role":"assistant","content":"Calling both.","tool_calls":[
                    {"id":"c1","type":"function","function":{"name":"one","arguments":"{\"x\":1}"}},
                    {"id":"c2","type":"function","function":{"name":"two","arguments":"{\"y\":2}"}}
                ]},
                {"role":"tool","tool_call_id":"c1","content":"one done"},
                {"role":"tool","tool_call_id":"c2","content":"two done"}
            ]
        }))
        .unwrap();
        let body = to_gemini_request(&req, &resolved()).unwrap();
        assert_eq!(body["contents"][1]["parts"][0]["text"], "Calling both.");
        assert_eq!(
            body["contents"][1]["parts"][1]["functionCall"]["name"],
            "one"
        );
        assert_eq!(
            body["contents"][1]["parts"][2]["functionCall"]["name"],
            "two"
        );
        assert_eq!(
            body["contents"][2]["parts"][0]["functionResponse"]["name"],
            "one"
        );
        assert_eq!(
            body["contents"][2]["parts"][1]["functionResponse"]["name"],
            "two"
        );
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
        let resp = from_gemini_response(&gemini, "gemini-1.5-pro").unwrap();
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
        let resp = from_gemini_response(&gemini, "gemini-1.5-pro").unwrap();
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("Hello there".into()))
        );
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn finish_reasons_fail_closed_and_missing_candidates_are_errors() {
        for reason in [
            "SAFETY",
            "RECITATION",
            "BLOCKLIST",
            "PROHIBITED_CONTENT",
            "SPII",
            "FUTURE_BLOCK_REASON",
        ] {
            let response = from_gemini_response(
                &json!({"candidates":[{"content":{"parts":[]},"finishReason":reason}]}),
                "m",
            )
            .unwrap();
            assert_eq!(
                response.choices[0].finish_reason.as_deref(),
                Some("content_filter"),
                "accepted {reason}"
            );
        }
        assert!(from_gemini_response(&json!({"candidates":[]}), "m").is_err());
        assert!(from_gemini_response(&json!({}), "m").is_err());
    }

    #[test]
    fn malformed_function_call_is_an_error_with_the_finish_message() {
        let error = from_gemini_response(
            &json!({"candidates":[{
                "content": {"parts": []},
                "finishReason": "MALFORMED_FUNCTION_CALL",
                "finishMessage": "Malformed function call: print(default_api.f(x=))"
            }]}),
            "m",
        )
        .unwrap_err();
        assert!(matches!(error, ProviderError::Parse(_)));
        assert!(
            error.to_string().contains("print(default_api.f(x=))"),
            "{error}"
        );
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
    async fn stream_of_real_events_skips_the_empty_thought_signature_part() {
        // The two events Gemini really streamed for QA test PR-01 (gemini-3.1-flash-lite). The
        // second carries only empty text plus a `thoughtSignature`, together with finishReason.
        // Framed with "\n\n" here; the "\r\n\r\n" framing Google uses is covered in `sse.rs`.
        let usage = concat!(
            "\"usageMetadata\":{\"promptTokenCount\":16,\"candidatesTokenCount\":2,\"totalTokenCount\":18,",
            "\"promptTokensDetails\":[{\"modality\":\"TEXT\",\"tokenCount\":16}],\"serviceTier\":\"standard\"},",
            "\"modelVersion\":\"gemini-3.1-flash-lite\",\"responseId\":\"OV30arqIA52v694PgqiioAY\"}",
        );
        let sse = format!(
            "data: {{\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"42\"}}],\"role\":\"model\"}},\"index\":0}}],{usage}\n\n\
             data: {{\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"\",\"thoughtSignature\":\"EnMKcQFpFH0T25PNK1ycdkqkRkLEbe5/dO/kNe1hnw5T0DdoeZ7aPDNfq9Teg6+FSRToPSEtNwuGkggv6OTYFbTLS5i6T55XXwLJUQ5jfC/ljM2H3JuM4NaYPXzSMkF/F443CpC0Wuqm8pRvRBBQilRQGMuy\"}}],\"role\":\"model\"}},\"finishReason\":\"STOP\",\"index\":0}}],{usage}\n\n"
        );
        let mut server = mockito::Server::new_async().await;
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
        let req: ChatRequest = serde_json::from_value(json!({
            "stream": true, "messages": [{ "role": "user", "content": "What is 6 times 7?" }]
        }))
        .unwrap();
        let chunks: Vec<ChatChunk> = provider
            .chat_stream(&req, &resolved())
            .await
            .unwrap()
            .map(|r| r.expect("no stream error"))
            .collect()
            .await;

        // Role, the "42" text, finish, usage: the empty-text part adds no chunk of its own.
        assert_eq!(chunks.len(), 4);
        let contents: Vec<String> = chunks
            .iter()
            .filter_map(|c| c.choices.first().and_then(|ch| ch.delta.content.clone()))
            .collect();
        assert_eq!(contents, vec!["42"]);
        let finish = chunks
            .iter()
            .find_map(|c| c.choices.first().and_then(|ch| ch.finish_reason.clone()));
        assert_eq!(finish.as_deref(), Some("stop"));
        let usage = chunks.iter().find_map(|c| c.usage.clone()).unwrap();
        assert_eq!(usage.prompt_tokens, Some(16));
        assert_eq!(usage.completion_tokens, Some(2));
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
    async fn stream_rejects_malformed_error_and_premature_eof() {
        for payload in [
            "data: {not-json}\n\n",
            "data: {\"error\":{\"code\":503,\"message\":\"unavailable\"}}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n",
            "data: [DONE]\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[]},\"finishReason\":\"MALFORMED_FUNCTION_CALL\",\"finishMessage\":\"bad call\"}]}\n\n",
        ] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock(
                    "POST",
                    "/models/gemini-1.5-pro:streamGenerateContent?alt=sse",
                )
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body(payload)
                .create_async()
                .await;
            let provider = GeminiProvider::new(reqwest::Client::new(), server.url());
            let req: ChatRequest =
                serde_json::from_value(json!({"messages":[{"role":"user","content":"hi"}]}))
                    .unwrap();
            let results: Vec<_> = provider
                .chat_stream(&req, &resolved())
                .await
                .unwrap()
                .collect()
                .await;
            assert!(results.iter().any(Result::is_err), "accepted {payload}");
            assert!(
                !results
                    .iter()
                    .filter_map(|result| result.as_ref().ok())
                    .any(|chunk| chunk
                        .choices
                        .first()
                        .is_some_and(|choice| choice.finish_reason.is_some()))
            );
        }
    }

    #[test]
    fn estimate_embedding_tokens_rounds_up_per_text() {
        let inputs: Vec<String> = vec!["abcde".into(), String::new(), "abcd".into()];
        assert_eq!(estimate_embedding_tokens(&inputs), 3);
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
        let resp = provider
            .embeddings(
                &req,
                &ResolvedEmbeddingConfig::for_test("gemini", "gemini-1.5-pro", "AIza-test"),
            )
            .await
            .unwrap();
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

    #[test]
    fn gemini_prompt_count_excludes_cached_content() {
        // promptTokenCount is cache-inclusive; billing charges input and
        // cache-read separately, so the gross count double-bills the cache.
        assert_eq!(uncached_prompt(Some(10000), Some(8000)), Some(2000));
    }

    #[test]
    fn gemini_prompt_count_passes_through_without_cache() {
        assert_eq!(uncached_prompt(Some(10000), None), Some(10000));
        assert_eq!(uncached_prompt(None, Some(8000)), None);
    }

    #[test]
    fn gemini_prompt_count_clamps_instead_of_going_negative() {
        assert_eq!(uncached_prompt(Some(100), Some(640)), Some(0));
    }

    #[test]
    fn tool_schema_is_sent_unchanged_as_parameters_json_schema() {
        // A Pydantic/MCP-style schema with keys Gemini's `parameters` field rejects.
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "additionalProperties": false,
            "$defs": { "Unit": { "type": "string", "enum": ["c", "f"] } },
            "properties": {
                "limit": { "type": "integer", "exclusiveMinimum": 0 },
                "unit": { "$ref": "#/$defs/Unit" },
                "tags": { "type": "array", "items": { "type": "string" }, "uniqueItems": true },
                "mode": { "oneOf": [{ "const": "fast" }, { "const": "exact" }], "examples": ["fast"] }
            },
            "required": ["limit"]
        });
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{ "type": "function", "function": { "name": "search", "parameters": schema } }]
        }))
        .unwrap();
        let body = to_gemini_request(&req, &resolved()).unwrap();
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["parametersJsonSchema"], schema);
        assert!(decl.get("parameters").is_none());
    }

    #[test]
    fn stop_becomes_stop_sequences() {
        for (stop, expected) in [
            (json!(["6", "7"]), json!(["6", "7"])),
            (json!("END"), json!(["END"])),
        ] {
            let req: ChatRequest = serde_json::from_value(json!({
                "messages": [{ "role": "user", "content": "count to 10" }],
                "stop": stop
            }))
            .unwrap();
            let body = to_gemini_request(&req, &resolved()).unwrap();
            assert_eq!(body["generationConfig"]["stopSequences"], expected);
            assert!(body.get("stop").is_none());
        }
    }

    #[test]
    fn tool_result_media_rides_beside_the_function_response() {
        // functionResponse.response is plain JSON with nowhere to put bytes, so the
        // image has to become a sibling part of the same turn. Before this, m.text()
        // silently dropped it and Gemini answered about an image it never saw.
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [
                { "role": "user", "content": "chart it" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1", "type": "function",
                    "function": { "name": "chart", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "c1", "content": [
                    { "type": "text", "text": "here is the chart" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }
                ]}
            ]
        }))
        .unwrap();

        let body = to_gemini_request(&req, &resolved()).unwrap();
        let parts = body["contents"][2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2, "media must survive as a sibling part");
        // Text stays inside the response envelope, keyed by function name.
        assert_eq!(parts[0]["functionResponse"]["name"], "chart");
        assert_eq!(
            parts[0]["functionResponse"]["response"]["result"],
            "here is the chart"
        );
        // Bytes ride alongside it.
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["data"], "iVBORw0KGgo=");
    }

    #[test]
    fn text_only_tool_results_are_unchanged() {
        // The common path must keep its exact previous shape: one part, no siblings.
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [
                { "role": "user", "content": "go" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1", "type": "function",
                    "function": { "name": "one", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "c1", "content": "done" }
            ]
        }))
        .unwrap();

        let body = to_gemini_request(&req, &resolved()).unwrap();
        let parts = body["contents"][2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["functionResponse"]["response"]["result"], "done");
    }

    #[test]
    fn unfetchable_tool_result_media_is_refused_rather_than_dropped() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [
                { "role": "user", "content": "go" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1", "type": "function",
                    "function": { "name": "one", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "c1", "content": [
                    { "type": "image_url", "image_url": { "url": "https://example.com/c.png" } }
                ]}
            ]
        }))
        .unwrap();

        let err = to_gemini_request(&req, &resolved()).unwrap_err();
        match err {
            ProviderError::Rejected(message) => {
                assert!(message.contains("web URL"), "{message}");
            }
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    #[test]
    fn response_format_maps_to_json_mode() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-4-hydrogen",
            "messages": [{"role": "user", "content": "hi"}],
            "response_format": {"type": "json_object"}
        }))
        .unwrap();
        let body = to_gemini_request(&req, &resolved()).unwrap();
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );

        let schema = json!({"type": "object", "properties": {"a": {"type": "string"}}});
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-4-hydrogen",
            "messages": [{"role": "user", "content": "hi"}],
            "response_format": {"type": "json_schema", "json_schema": {"name": "x", "schema": schema}}
        }))
        .unwrap();
        let body = to_gemini_request(&req, &resolved()).unwrap();
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(body["generationConfig"]["responseJsonSchema"], schema);
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn to_gemini_request_with_image_data_url() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "What is this image?" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==" } }
                ]
            }]
        })).unwrap();

        let body = to_gemini_request(&req, &resolved()).unwrap();
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "What is this image?");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(
            parts[1]["inlineData"]["data"],
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
        );
    }

    #[test]
    fn video_is_carried_as_inline_data() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [{ "role": "user", "content": [
                { "type": "video_url", "video_url": { "url": "data:video/mp4;base64,AAAA" } }
            ]}]
        }))
        .unwrap();
        let body = to_gemini_request(&req, &resolved()).unwrap();
        let part = &body["contents"][0]["parts"][0];
        assert_eq!(part["inlineData"]["mimeType"], "video/mp4");
        assert_eq!(part["inlineData"]["data"], "AAAA");
    }

    #[test]
    fn to_gemini_request_with_pdf_data_url() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "file", "file": { "file_data": "data:application/pdf;base64,JVBERi0xLjQKJeLjz9MK" } }
                ]
            }]
        })).unwrap();

        let body = to_gemini_request(&req, &resolved()).unwrap();
        let contents = body["contents"].as_array().unwrap();
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["inlineData"]["mimeType"], "application/pdf");
        assert_eq!(parts[0]["inlineData"]["data"], "JVBERi0xLjQKJeLjz9MK");
    }

    fn with_parts(parts: Vec<Value>) -> ChatRequest {
        let mut content = vec![json!({ "type": "text", "text": "what does it say?" })];
        content.extend(parts);
        serde_json::from_value(json!({ "messages": [{ "role": "user", "content": content }] }))
            .unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn gemini_rejection(req: &ChatRequest) -> String {
        match to_gemini_request(req, &resolved()) {
            Err(ProviderError::Rejected(message)) => message,
            other => panic!("expected a by-name refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_image_type_gemini_cannot_read_is_refused_and_heic_is_sent() {
        let gif = with_parts(vec![json!({ "type": "image_url", "image_url": {
            "url": "data:image/gif;base64,R0lGOD"
        }})]);
        let message = gemini_rejection(&gif);
        assert!(
            message.starts_with("Gemini does not accept this input"),
            "{message}"
        );
        assert!(message.contains("image/gif"), "{message}");

        let heic = with_parts(vec![json!({ "type": "image_url", "image_url": {
            "url": "data:image/heic;base64,AAAA"
        }})]);
        let body = to_gemini_request(&heic, &resolved()).unwrap();
        assert_eq!(
            body["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/heic"
        );
    }

    #[test]
    fn a_text_document_reaches_gemini_as_text_and_others_are_refused() {
        let csv = with_parts(vec![json!({ "type": "file", "file": {
            "file_data": format!("data:text/csv;base64,{}", b64(b"region,q2\nEMEA,913")),
            "filename": "data.csv"
        }})]);
        let body = to_gemini_request(&csv, &resolved()).unwrap();
        assert_eq!(
            body["contents"][0]["parts"][1]["text"],
            "File: data.csv\n\nregion,q2\nEMEA,913"
        );

        let doc = "application/msword";
        let message = gemini_rejection(&with_parts(vec![json!({ "type": "file", "file": {
            "file_data": format!("data:{doc};base64,0M8R4A=="), "filename": "r.doc"
        }})]));
        assert!(message.contains(doc), "{message}");
    }

    #[test]
    fn more_than_20_mb_of_inline_data_is_refused_before_sending() {
        let chunk = "A".repeat(10_000_000);
        let image = |data: &str| {
            json!({ "type": "image_url", "image_url": {
                "url": format!("data:image/png;base64,{data}")
            }})
        };
        let at_limit = with_parts(vec![image(&chunk), image(&chunk)]);
        assert!(to_gemini_request(&at_limit, &resolved()).is_ok());

        let over = with_parts(vec![image(&chunk), image(&chunk), image("AAAA")]);
        let message = gemini_rejection(&over);
        assert!(message.contains("20 MB"), "{message}");
    }

    #[test]
    fn to_gemini_request_with_unsupported_parts_rejected() {
        // Public web URL not allowed in Gemini inlineData
        let req_url: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image_url", "image_url": { "url": "https://example.com/test.png" } }
                ]
            }]
        }))
        .unwrap();
        let err = to_gemini_request(&req_url, &resolved()).unwrap_err();
        match err {
            ProviderError::Rejected(message) => {
                assert!(message.contains("URL") || message.contains("base64"));
            }
            _ => panic!("Expected ProviderError::Rejected"),
        }

        // file.file_id not supported
        let req_file_id: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "file", "file": { "file_id": "file-123" } }
                ]
            }]
        }))
        .unwrap();
        let err = to_gemini_request(&req_file_id, &resolved()).unwrap_err();
        match err {
            ProviderError::Rejected(message) => {
                assert!(message.contains("file.file_id"));
            }
            _ => panic!("Expected ProviderError::Rejected"),
        }

        // Empty message content
        let req_empty: ChatRequest = serde_json::from_value(json!({
            "model": "gemini-1.5-pro",
            "messages": [{
                "role": "user",
                "content": []
            }]
        }))
        .unwrap();
        let err = to_gemini_request(&req_empty, &resolved()).unwrap_err();
        match err {
            ProviderError::Rejected(message) => {
                assert!(message.contains("empty"));
            }
            _ => panic!("Expected ProviderError::Rejected"),
        }
    }

    // ── explicit caching (cache_control markers → cachedContents) ───────────────

    const CACHED_MODEL: &str = "gemini-2.5-flash";

    fn flash() -> ResolvedConfig {
        ResolvedConfig {
            model: CACHED_MODEL.into(),
            ..resolved()
        }
    }

    /// A conversation whose system prompt (large enough to cache) and first user turn are
    /// marked; the last turn is not.
    fn marked_request(system: &str) -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": [
                    { "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }
                ] },
                { "role": "user", "content": [
                    { "type": "text", "text": "first", "cache_control": { "type": "ephemeral" } }
                ] },
                { "role": "assistant", "content": "ok" },
                { "role": "user", "content": "second" }
            ]
        }))
        .unwrap()
    }

    /// A prompt large enough to cache, distinct per `test`: what the process knows about a
    /// prefix is global, and mockito reuses server addresses across tests running in parallel,
    /// so tests sharing one prompt could find each other's cached or refused entry.
    fn large_system(test: &str) -> String {
        format!("{test}: {}", "You are a careful assistant. ".repeat(400))
    }

    fn generate_ok(cached_tokens: i64) -> String {
        json!({
            "candidates": [{ "content": { "role": "model", "parts": [{ "text": "done" }] }, "finishReason": "STOP" }],
            "usageMetadata": {
                "promptTokenCount": cached_tokens + 10,
                "candidatesTokenCount": 1,
                "totalTokenCount": cached_tokens + 11,
                "cachedContentTokenCount": cached_tokens
            }
        })
        .to_string()
    }

    fn generate_path() -> String {
        format!("/models/{CACHED_MODEL}:generateContent")
    }

    #[test]
    fn cache_plan_covers_through_the_last_marked_turn() {
        let marked = json!({ "type": "ephemeral" });
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "sys" },
                { "role": "user", "content": "run it" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "c1", "type": "function",
                    "function": { "name": "run", "arguments": "{}" }
                }] },
                { "role": "tool", "tool_call_id": "c1", "content": [
                    { "type": "text", "text": "output", "cache_control": marked }
                ] },
                { "role": "user", "content": "and now?" }
            ]
        }))
        .unwrap();
        // user, model, the tool-result turn: three turns through the marker.
        let plan = translate(&req, &flash()).unwrap().cache_plan.unwrap();
        assert_eq!(plan.prefix_len, 3);

        let unmarked: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        assert!(translate(&unmarked, &flash()).unwrap().cache_plan.is_none());

        // A marker on a tool definition caches the system instruction and tools only.
        let tool_marked: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{ "type": "function", "function": { "name": "run" }, "cache_control": marked }]
        }))
        .unwrap();
        let plan = translate(&tool_marked, &flash())
            .unwrap()
            .cache_plan
            .unwrap();
        assert_eq!(plan.prefix_len, 0);
    }

    #[tokio::test]
    async fn marked_prefix_is_cached_once_and_reused() {
        let mut server = mockito::Server::new_async().await;
        let create = server
            .mock("POST", "/cachedContents")
            .match_header("x-goog-api-key", "AIza-test")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": format!("models/{CACHED_MODEL}"),
                "ttl": "300s",
                "systemInstruction": { "parts": [{ "text": large_system("reused") }] },
                "contents": [{ "role": "user", "parts": [{ "text": "first" }] }]
            })))
            .with_status(200)
            .with_body(json!({ "name": "cachedContents/abc" }).to_string())
            .expect(1)
            .create_async()
            .await;
        let generate = server
            .mock("POST", generate_path().as_str())
            .match_body(mockito::Matcher::PartialJson(json!({
                "cachedContent": "cachedContents/abc",
                "contents": [
                    { "role": "model", "parts": [{ "text": "ok" }] },
                    { "role": "user", "parts": [{ "text": "second" }] }
                ]
            })))
            .with_status(200)
            .with_body(generate_ok(3000))
            .expect(2)
            .create_async()
            .await;

        let provider =
            GeminiProvider::new(reqwest::Client::new(), server.url()).with_explicit_cache(true);
        let req = marked_request(&large_system("reused"));
        for _ in 0..2 {
            let resp = provider.chat(&req, &flash()).await.unwrap();
            let usage = resp.usage.unwrap();
            assert_eq!(usage.cache_read_input_tokens, Some(3000));
            assert_eq!(usage.prompt_tokens, Some(10));
        }
        create.assert_async().await;
        generate.assert_async().await;
    }

    #[tokio::test]
    async fn small_or_disabled_prefixes_are_sent_whole() {
        let large = large_system("disabled");
        for (system, enabled) in [("short", true), (large.as_str(), false)] {
            let mut server = mockito::Server::new_async().await;
            let create = server
                .mock("POST", "/cachedContents")
                .expect(0)
                .create_async()
                .await;
            let generate = server
                .mock("POST", generate_path().as_str())
                .match_body(mockito::Matcher::PartialJson(json!({
                    "systemInstruction": { "parts": [{ "text": system }] }
                })))
                .with_status(200)
                .with_body(generate_ok(0))
                .create_async()
                .await;
            let provider = GeminiProvider::new(reqwest::Client::new(), server.url())
                .with_explicit_cache(enabled);
            provider
                .chat(&marked_request(system), &flash())
                .await
                .unwrap();
            create.assert_async().await;
            generate.assert_async().await;
        }
    }

    #[tokio::test]
    async fn a_prefix_gemini_will_not_cache_is_sent_whole_and_not_retried() {
        let mut server = mockito::Server::new_async().await;
        let create = server
            .mock("POST", "/cachedContents")
            .with_status(400)
            .with_body(r#"{"error":{"message":"Cached content is too small"}}"#)
            .expect(1)
            .create_async()
            .await;
        let generate = server
            .mock("POST", generate_path().as_str())
            .match_body(mockito::Matcher::PartialJson(json!({
                "systemInstruction": { "parts": [{ "text": large_system("too-small") }] }
            })))
            .with_status(200)
            .with_body(generate_ok(0))
            .expect(2)
            .create_async()
            .await;
        let provider =
            GeminiProvider::new(reqwest::Client::new(), server.url()).with_explicit_cache(true);
        let req = marked_request(&large_system("too-small"));
        for _ in 0..2 {
            provider.chat(&req, &flash()).await.unwrap();
        }
        create.assert_async().await;
        generate.assert_async().await;
    }

    #[tokio::test]
    async fn a_refused_cache_reference_is_resent_whole() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/cachedContents")
            .with_status(200)
            .with_body(json!({ "name": "cachedContents/gone" }).to_string())
            .create_async()
            .await;
        let refused = server
            .mock("POST", generate_path().as_str())
            .match_body(mockito::Matcher::PartialJson(
                json!({ "cachedContent": "cachedContents/gone" }),
            ))
            .with_status(404)
            .with_body(r#"{"error":{"message":"CachedContent not found"}}"#)
            .expect(1)
            .create_async()
            .await;
        let whole = server
            .mock("POST", generate_path().as_str())
            .match_body(mockito::Matcher::PartialJson(json!({
                "systemInstruction": { "parts": [{ "text": large_system("refused-ref") }] }
            })))
            .with_status(200)
            .with_body(generate_ok(0))
            .expect(1)
            .create_async()
            .await;
        let provider =
            GeminiProvider::new(reqwest::Client::new(), server.url()).with_explicit_cache(true);
        let resp = provider
            .chat(&marked_request(&large_system("refused-ref")), &flash())
            .await
            .unwrap();
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("done"));
        refused.assert_async().await;
        whole.assert_async().await;
    }

    #[test]
    fn missing_thought_signature_gets_bypass_on_first_call_only() {
        let call = |name: &str| ToolCall {
            id: name.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
            extra: Map::new(),
        };
        let mut m: Message = serde_json::from_value(json!({"role": "assistant"})).unwrap();
        m.tool_calls = Some(vec![call("a"), call("b")]);
        let v: Value = assistant_to_gemini(&m);
        assert_eq!(v["parts"][0]["thoughtSignature"], SKIP_THOUGHT_SIGNATURE);
        assert!(v["parts"][1].get("thoughtSignature").is_none());
    }

    #[test]
    fn thought_signature_round_trips_through_response_and_request() {
        let gemini = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": { "name": "lookup", "args": { "id": 1 } },
                        "thoughtSignature": "sig-abc-123"
                    }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 3, "totalTokenCount": 8 }
        });
        let resp = from_gemini_response(&gemini, "gemini-2.5-pro").unwrap();
        let tc = &resp.choices[0].message.tool_calls.as_ref().unwrap()[0];
        assert_eq!(
            tc.extra.get("thought_signature").and_then(|v| v.as_str()),
            Some("sig-abc-123")
        );

        let model_msg = assistant_to_gemini(&resp.choices[0].message);
        assert_eq!(
            model_msg["parts"][0]["thoughtSignature"].as_str(),
            Some("sig-abc-123")
        );
    }

    #[tokio::test]
    async fn stream_preserves_thought_signature_and_skips_usage_only_event() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"lookup\",\"args\":{\"id\":1}},\"thoughtSignature\":\"sig-stream-1\"}]},\"finishReason\":\"STOP\"}]}\n\n",
            "data: {\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5,\"totalTokenCount\":15}}\n\n"
        );
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
            json!({ "stream": true, "messages": [{ "role": "user", "content": "hi" }] }),
        )
        .unwrap();
        let results: Vec<Result<ChatChunk, ProviderError>> = provider
            .chat_stream(&req, &resolved())
            .await
            .unwrap()
            .collect()
            .await;
        assert!(
            results.iter().all(|r| r.is_ok()),
            "unexpected error: {results:?}"
        );
        let chunks: Vec<ChatChunk> = results.into_iter().map(|r| r.unwrap()).collect();
        let tc_delta = chunks
            .iter()
            .flat_map(|c| c.choices.iter())
            .filter_map(|ch| ch.delta.tool_calls.as_ref())
            .flatten()
            .next()
            .unwrap();
        assert_eq!(
            tc_delta
                .extra
                .get("thought_signature")
                .and_then(|v| v.as_str()),
            Some("sig-stream-1")
        );
        assert_eq!(
            chunks
                .last()
                .and_then(|c| c.usage.as_ref())
                .and_then(|u| u.total_tokens),
            Some(15)
        );
    }

    #[tokio::test]
    async fn embeddings_chunk_over_100_requests() {
        let mut server = mockito::Server::new_async().await;
        let first_body: Vec<Value> = (0..100).map(|i| json!({ "values": [i as f32] })).collect();
        let second_body = vec![json!({ "values": [100.0] })];
        let m1 = server
            .mock("POST", "/models/gemini-1.5-pro:batchEmbedContents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({ "embeddings": first_body }).to_string())
            .expect(1)
            .create_async()
            .await;
        let m2 = server
            .mock("POST", "/models/gemini-1.5-pro:batchEmbedContents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({ "embeddings": second_body }).to_string())
            .expect(1)
            .create_async()
            .await;

        let provider = GeminiProvider::new(reqwest::Client::new(), server.url());
        let inputs: Vec<String> = (0..101).map(|i| format!("t{i}")).collect();
        let req: EmbeddingsRequest =
            serde_json::from_value(json!({ "model": "gemini-1.5-pro", "input": inputs })).unwrap();
        let resp = provider
            .embeddings(
                &req,
                &ResolvedEmbeddingConfig::for_test("gemini", "gemini-1.5-pro", "AIza-test"),
            )
            .await
            .unwrap();
        assert_eq!(resp.data.len(), 101);
        assert_eq!(resp.data[100].index, 100);
        m1.assert_async().await;
        m2.assert_async().await;
    }
}

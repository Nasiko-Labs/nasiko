use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    pub model: String,
}

#[derive(Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
    /// The model the provider actually served. Can differ from the one asked
    /// for (aliases resolve to a dated snapshot, e.g. `gpt-4o-mini` →
    /// `gpt-4o-mini-2024-07-18`), and pricing must follow what was served.
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct Choice {
    message: MessageContent,
}

#[derive(Deserialize)]
struct MessageContent {
    content: String,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: i64,
    #[serde(default)]
    completion_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
    /// OpenAI reports cache hits nested here rather than as a flat field.
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: i64,
}

/// What one LLM call cost, in the shape both `token_usage` and the GenAI
/// span semconv need.
///
/// The client previously returned only `total_tokens`, which is enough to show
/// a number but not to **price** one: `model_pricing` charges input and output
/// at different rates, so a single total cannot be costed. Splitting it here is
/// what makes MAF's own spend meterable at all.
#[derive(Debug, Clone, Default)]
pub struct LlmUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    /// Prompt tokens served from the provider's cache. Billed at a lower rate
    /// where the provider supports it; recorded so that stays visible.
    pub cached_tokens: i64,
    /// The model the provider reported serving, falling back to the requested
    /// one when the response omits it.
    pub model: String,
    pub latency_ms: i64,
}

impl LlmClient {
    pub fn new(
        http: reqwest::Client,
        api_key: String,
        base_url: Option<String>,
        model: String,
    ) -> Self {
        Self {
            http,
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
            model,
        }
    }

    /// The provider this client talks to, as recorded on `token_usage.provider`.
    ///
    /// The wire format is OpenAI's regardless of host, so this reports the
    /// protocol rather than trying to guess a vendor from `base_url` — an
    /// Azure or proxy deployment is still served over the OpenAI API shape.
    pub fn provider(&self) -> &'static str {
        "openai"
    }

    /// Returns `(content, usage)`.
    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<(String, LlmUsage), String> {
        self.chat_inner(messages, None).await
    }

    /// Returns `(parsed_json, usage)`. Uses `json_object` mode — valid JSON only,
    /// no schema enforcement.
    pub async fn chat_json(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<(serde_json::Value, LlmUsage), String> {
        let fmt = serde_json::json!({"type": "json_object"});
        let (text, usage) = self.chat_inner(messages, Some(fmt)).await?;
        let json = serde_json::from_str(&text)
            .map_err(|e| format!("LLM returned invalid JSON: {e}\nRaw: {text}"))?;
        Ok((json, usage))
    }

    /// Returns `(parsed_json, usage)`. Uses OpenAI structured outputs with the provided
    /// JSON Schema — equivalent to Python's `with_structured_output(PydanticModel)`.
    /// With `strict: true` the API guarantees the response matches the schema exactly.
    pub async fn chat_json_schema(
        &self,
        messages: Vec<ChatMessage>,
        schema_name: &str,
        schema: serde_json::Value,
    ) -> Result<(serde_json::Value, LlmUsage), String> {
        let fmt = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": schema_name,
                "strict": true,
                "schema": schema
            }
        });
        let (text, usage) = self.chat_inner(messages, Some(fmt)).await?;
        let json = serde_json::from_str(&text)
            .map_err(|e| format!("LLM returned invalid JSON: {e}\nRaw: {text}"))?;
        Ok((json, usage))
    }

    async fn chat_inner(
        &self,
        messages: Vec<ChatMessage>,
        response_format: Option<serde_json::Value>,
    ) -> Result<(String, LlmUsage), String> {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0,
        });

        if let Some(fmt) = response_format {
            body["response_format"] = fmt;
        }

        let started = std::time::Instant::now();
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("LLM request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("LLM HTTP {status}: {body}"));
        }

        let parsed: CompletionResponse = resp
            .json()
            .await
            .map_err(|e| format!("LLM response parse error: {e}"))?;
        let latency_ms = started.elapsed().as_millis() as i64;
        let usage = match parsed.usage {
            Some(u) => LlmUsage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                // Some OpenAI-compatible servers omit `total_tokens` while
                // still reporting the split, so derive it rather than store 0.
                total_tokens: if u.total_tokens > 0 {
                    u.total_tokens
                } else {
                    u.prompt_tokens + u.completion_tokens
                },
                cached_tokens: u
                    .prompt_tokens_details
                    .map(|d| d.cached_tokens)
                    .unwrap_or(0),
                model: parsed.model.unwrap_or_else(|| self.model.clone()),
                latency_ms,
            },
            // No usage block at all: record the call happened, with zeroes.
            None => LlmUsage {
                model: parsed.model.unwrap_or_else(|| self.model.clone()),
                latency_ms,
                ..Default::default()
            },
        };

        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| "LLM returned no choices".to_string())?;

        Ok((content, usage))
    }
}

use std::sync::Arc;

use nasiko_pricing::{NoUsageLedger, UsageLedger};
use serde::{Deserialize, Serialize};

/// OpenAI reports a model/parameter mismatch as a 400 whose body names the offending
/// field: `{"error":{"param":"temperature","code":"unsupported_value",...}}` — reasoning
/// models (the `o` series, gpt-5-class) only accept the default `temperature`. Detected
/// from the response rather than hardcoding which models reject it, since that set
/// changes with every new model release.
fn openai_rejects_temperature(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let Some(error) = value.get("error") else {
        return false;
    };
    let code = error
        .get("code")
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    let param = error
        .get("param")
        .and_then(|p| p.as_str())
        .unwrap_or_default();
    param == "temperature" && matches!(code, "unsupported_value" | "unsupported_parameter")
}

tokio::task_local! {
    /// Step index of the MAF phase running on this task; `None` for run-level phases.
    pub static MAF_STEP: Option<i32>;
}

/// Routed-call label: `maf` becomes `maf:step<i>` inside a step phase; other labels unchanged.
pub fn step_component(component: &str, step: Option<i32>) -> String {
    match step {
        Some(i) if component == "maf" => format!("maf:step{i}"),
        _ => component.to_string(),
    }
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    pub model: String,
    /// Where this client's metered calls are recorded besides `trace_usage`.
    ledger: Arc<dyn UsageLedger>,
    /// In-process LLM router. When set, calls go through it (any provider) and the
    /// router writes `trace_usage`, so callers must not record usage themselves.
    backend: Option<crate::llm_backend::SharedLlmBackend>,
    /// Billing/labeling context for routed calls.
    call: Option<crate::llm_backend::LlmCallContext>,
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
    /// The same client spending `api_key`, for a user who saved their own. A user's key is an
    /// OpenAI key, so the call goes to OpenAI directly, not through the router.
    pub fn with_api_key(&self, api_key: String) -> Self {
        Self {
            api_key,
            backend: None,
            call: None,
            ..self.clone()
        }
    }

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
            ledger: Arc::new(NoUsageLedger),
            backend: None,
            call: None,
        }
    }

    /// The same client, its calls served by the in-process LLM router.
    pub fn with_backend(self, backend: crate::llm_backend::SharedLlmBackend) -> Self {
        Self {
            backend: Some(backend),
            ..self
        }
    }

    /// The same client, routed calls billed/labelled with `call`.
    pub fn with_call(&self, call: crate::llm_backend::LlmCallContext) -> Self {
        Self {
            call: Some(call),
            ..self.clone()
        }
    }

    /// Whether calls go through the router (which then owns usage recording).
    pub fn is_routed(&self) -> bool {
        self.backend.is_some()
    }

    /// The same client, its metered calls handed to `ledger`.
    pub fn with_ledger(self, ledger: Arc<dyn UsageLedger>) -> Self {
        Self { ledger, ..self }
    }

    pub fn ledger(&self) -> Arc<dyn UsageLedger> {
        self.ledger.clone()
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

        if let Some(backend) = &self.backend {
            let mut call = self.call.clone().unwrap_or(crate::llm_backend::LlmCallContext {
                user_id: uuid::Uuid::nil(),
                component: "maf".into(),
                session_id: None,
            });
            // A step phase runs inside `MAF_STEP.scope`; label its row `maf:step<i>` so
            // `platform_spend` keeps the per-step split for router-recorded calls.
            let step: Option<i32> = MAF_STEP.try_with(|s| *s).ok().flatten();
            call.component = step_component(&call.component, step);
            if self.model.is_empty() {
                body.as_object_mut().unwrap().remove("model");
            }
            let started = std::time::Instant::now();
            let value: serde_json::Value = backend.chat(&call, body).await?;
            let parsed: CompletionResponse = serde_json::from_value(value)
                .map_err(|e| format!("LLM response parse error: {e}"))?;
            return self.finish(parsed, started);
        }

        // Direct OpenAI call (user key) with no configured model: OpenAI default.
        if body["model"].as_str().is_some_and(str::is_empty) {
            body["model"] = serde_json::json!("gpt-6-luna");
        }

        let send = |body: &serde_json::Value| {
            self.http
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(body)
                .timeout(std::time::Duration::from_secs(120))
                .send()
        };

        let started = std::time::Instant::now();
        let mut resp = send(&body)
            .await
            .map_err(|e| format!("LLM request failed: {e}"))?;

        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            if openai_rejects_temperature(&err_body) {
                body.as_object_mut().unwrap().remove("temperature");
                resp = send(&body)
                    .await
                    .map_err(|e| format!("LLM request failed: {e}"))?;
            } else {
                return Err(format!("LLM HTTP {status}: {err_body}"));
            }
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("LLM HTTP {status}: {body}"));
        }

        let parsed: CompletionResponse = resp
            .json()
            .await
            .map_err(|e| format!("LLM response parse error: {e}"))?;
        self.finish(parsed, started)
    }

    fn finish(
        &self,
        parsed: CompletionResponse,
        started: std::time::Instant,
    ) -> Result<(String, LlmUsage), String> {
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

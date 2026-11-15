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

/// Whose OpenAI key a client spends, so a refused key can say who has to fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// The key the run's owner saved from the UI.
    Owner,
    /// The server's own `OPENAI_API_KEY`.
    Server,
}

/// Why one of MAF's model calls failed, worded for the person who reads it on the run page.
///
/// The provider's response body never reaches the message for a refused key (it can echo
/// part of the key back); it is logged instead. Other refusals keep the provider's short
/// `error.message`, which names what was wrong (an unknown model, a too-long prompt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    /// The provider refused the call and no key was configured at all.
    NoKey,
    /// The provider refused the key (HTTP 401 or 403).
    KeyRejected(KeySource),
    /// Any other non-success status, with the provider's short message when it gave one.
    Status { code: u16, detail: Option<String> },
    /// A transport failure, or a reply that could not be used.
    Other(String),
}

/// What a run says when no OpenAI key exists for it: the same words the planner uses.
pub const NO_OPENAI_KEY_MESSAGE: &str =
    "No OpenAI API key: add one in the setup guide or Settings → Secrets";

impl LlmError {
    /// True when the provider refused the call for a reason the user or an admin must fix
    /// (a missing or refused key, or an error status), rather than a malformed reply.
    pub fn is_provider_refusal(&self) -> bool {
        !matches!(self, LlmError::Other(_))
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::NoKey => f.write_str(NO_OPENAI_KEY_MESSAGE),
            LlmError::KeyRejected(KeySource::Owner) => f.write_str(
                "OpenAI rejected your saved API key. Update it in LLM router, then try again.",
            ),
            LlmError::KeyRejected(KeySource::Server) => f.write_str(
                "OpenAI rejected this server's API key. Ask your admin to check OPENAI_API_KEY.",
            ),
            LlmError::Status { code: 429, .. } => {
                f.write_str("OpenAI returned HTTP 429 (rate limited). Try again shortly.")
            }
            LlmError::Status { code, .. } if *code >= 500 => {
                write!(f, "OpenAI returned HTTP {code}. Try again shortly.")
            }
            LlmError::Status {
                code,
                detail: Some(detail),
            } => write!(f, "OpenAI returned HTTP {code}: {detail}"),
            LlmError::Status { code, detail: None } => write!(f, "OpenAI returned HTTP {code}."),
            LlmError::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LlmError {}

/// Lets a caller whose own error is a `String` keep using `?` on a model call.
impl From<LlmError> for String {
    fn from(error: LlmError) -> Self {
        error.to_string()
    }
}

impl From<LlmError> for super::types::RunError {
    fn from(error: LlmError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// The longest provider message carried into a user-facing error.
const MAX_DETAIL_CHARS: usize = 200;

/// `error.message` from an OpenAI-shaped error body, shortened, or `None` when absent.
fn provider_detail(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = value.get("error")?.get("message")?.as_str()?.trim();
    if message.is_empty() {
        return None;
    }
    Some(message.chars().take(MAX_DETAIL_CHARS).collect())
}

/// Classifies a non-success reply. `key_missing` is whether the call was sent without a key.
fn classify_failure(
    status: reqwest::StatusCode,
    body: &str,
    key_source: KeySource,
    key_missing: bool,
) -> LlmError {
    let code = status.as_u16();
    match code {
        401 | 403 if key_missing => LlmError::NoKey,
        401 | 403 => LlmError::KeyRejected(key_source),
        _ => LlmError::Status {
            code,
            detail: provider_detail(body),
        },
    }
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    api_key: String,
    /// Whose key `api_key` is, for the message when the provider refuses it.
    key_source: KeySource,
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
            key_source: KeySource::Owner,
            ..self.clone()
        }
    }

    /// Whose key this client spends.
    pub fn key_source(&self) -> KeySource {
        self.key_source
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
            key_source: KeySource::Server,
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
    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<(String, LlmUsage), LlmError> {
        self.chat_inner(messages, None).await
    }

    /// Returns `(parsed_json, usage)`. Uses `json_object` mode — valid JSON only,
    /// no schema enforcement.
    pub async fn chat_json(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<(serde_json::Value, LlmUsage), LlmError> {
        let fmt = serde_json::json!({"type": "json_object"});
        let (text, usage) = self.chat_inner(messages, Some(fmt)).await?;
        let json = serde_json::from_str(&text)
            .map_err(|e| LlmError::Other(format!("LLM returned invalid JSON: {e}\nRaw: {text}")))?;
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
    ) -> Result<(serde_json::Value, LlmUsage), LlmError> {
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
            .map_err(|e| LlmError::Other(format!("LLM returned invalid JSON: {e}\nRaw: {text}")))?;
        Ok((json, usage))
    }

    async fn chat_inner(
        &self,
        messages: Vec<ChatMessage>,
        response_format: Option<serde_json::Value>,
    ) -> Result<(String, LlmUsage), LlmError> {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0,
        });

        if let Some(fmt) = response_format {
            body["response_format"] = fmt;
        }

        if let Some(backend) = &self.backend {
            let mut call = self
                .call
                .clone()
                .unwrap_or(crate::llm_backend::LlmCallContext {
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
            let value: serde_json::Value = backend
                .chat(&call, body)
                .await
                .map_err(|e| LlmError::Other(e.to_string()))?;
            let parsed: CompletionResponse = serde_json::from_value(value)
                .map_err(|e| LlmError::Other(format!("LLM response parse error: {e}")))?;
            return self
                .finish(parsed, started, &self.model)
                .map_err(LlmError::Other);
        }

        // Direct OpenAI call (user key): empty model → OpenAI default; a foreign model (e.g. an
        // inherited Gemini `DEFAULT_MODEL`) → OpenAI default only on api.openai.com.
        let fallback: String = crate::providers::direct_openai_fallback();
        let direct_model: String =
            crate::providers::resolve_direct_chat_model(&self.model, &self.base_url, &fallback);
        body["model"] = serde_json::json!(direct_model);

        let send = |body: &serde_json::Value| {
            self.http
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(body)
                .timeout(std::time::Duration::from_secs(120))
                .send()
        };

        let started = std::time::Instant::now();
        let request_failed =
            |e: reqwest::Error| LlmError::Other(format!("LLM request failed: {e}"));
        let mut resp = send(&body).await.map_err(request_failed)?;

        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            if openai_rejects_temperature(&err_body) {
                body.as_object_mut().unwrap().remove("temperature");
                resp = send(&body).await.map_err(request_failed)?;
            } else {
                return Err(self.refusal(status, &err_body));
            }
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(self.refusal(status, &body));
        }

        let parsed: CompletionResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::Other(format!("LLM response parse error: {e}")))?;
        self.finish(parsed, started, &direct_model)
            .map_err(LlmError::Other)
    }

    fn finish(
        &self,
        parsed: CompletionResponse,
        started: std::time::Instant,
        sent_model: &str,
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
                model: parsed.model.unwrap_or_else(|| sent_model.to_owned()),
                latency_ms,
            },
            // No usage block at all: record the call happened, with zeroes.
            None => LlmUsage {
                model: parsed.model.unwrap_or_else(|| sent_model.to_owned()),
                latency_ms,
                ..Default::default()
            },
        };

        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| LlmError::Other("LLM returned no choices".to_string()))?;

        Ok((content, usage))
    }

    /// A refused call as an [`LlmError`]. The body is logged here and goes no further.
    fn refusal(&self, status: reqwest::StatusCode, body: &str) -> LlmError {
        tracing::warn!(
            status = status.as_u16(),
            key_source = ?self.key_source,
            body = %body,
            "maf: model provider refused the call"
        );
        classify_failure(
            status,
            body,
            self.key_source,
            self.api_key.trim().is_empty(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn a_refused_key_names_whose_key_and_drops_the_body() {
        let body = r#"{"error":{"message":"Incorrect API key provided: sk-abc...wxyz"}}"#;
        let owner = classify_failure(StatusCode::UNAUTHORIZED, body, KeySource::Owner, false);
        assert_eq!(owner, LlmError::KeyRejected(KeySource::Owner));
        assert!(owner.to_string().contains("your saved API key"));
        assert!(!owner.to_string().contains("sk-abc"));

        let server = classify_failure(StatusCode::FORBIDDEN, body, KeySource::Server, false);
        assert!(server.to_string().contains("OPENAI_API_KEY"));
    }

    #[test]
    fn a_refusal_sent_without_a_key_says_no_key_is_configured() {
        let error = classify_failure(StatusCode::UNAUTHORIZED, "", KeySource::Server, true);
        assert_eq!(error, LlmError::NoKey);
        assert_eq!(error.to_string(), NO_OPENAI_KEY_MESSAGE);
    }

    #[test]
    fn other_statuses_keep_only_the_providers_short_message() {
        let body = r#"{"error":{"message":"The model `gpt-4.0` does not exist"}}"#;
        let missing = classify_failure(StatusCode::NOT_FOUND, body, KeySource::Server, false);
        assert_eq!(
            missing.to_string(),
            "OpenAI returned HTTP 404: The model `gpt-4.0` does not exist"
        );

        let limited =
            classify_failure(StatusCode::TOO_MANY_REQUESTS, body, KeySource::Owner, false);
        assert_eq!(
            limited.to_string(),
            "OpenAI returned HTTP 429 (rate limited). Try again shortly."
        );

        let down = classify_failure(StatusCode::BAD_GATEWAY, "<html>", KeySource::Owner, false);
        assert_eq!(
            down.to_string(),
            "OpenAI returned HTTP 502. Try again shortly."
        );
    }

    #[test]
    fn only_a_malformed_reply_is_not_a_provider_refusal() {
        assert!(LlmError::NoKey.is_provider_refusal());
        assert!(
            LlmError::Status {
                code: 400,
                detail: None
            }
            .is_provider_refusal()
        );
        assert!(!LlmError::Other("LLM returned no choices".into()).is_provider_refusal());
    }

    #[test]
    fn a_client_given_the_owners_key_reports_it_as_the_owners() {
        let client = LlmClient::new(reqwest::Client::new(), "k".into(), None, "m".into());
        assert_eq!(client.key_source(), KeySource::Server);
        assert_eq!(
            client.with_api_key("u".into()).key_source(),
            KeySource::Owner
        );
    }
}

//! DeepSeek-backed `RequestClassifier` implementation for P2.
//!
//! Selected when `CLASSIFIER_BACKEND=hosted`. Endpoint, API key and timeout
//! are read by the binary's `config.rs` and injected at construction — never
//! hard-coded here.
//!
//! # Determinism
//! `temperature = 0.0` is hard-coded so identical inputs always produce
//! identical outputs. The judges run the eval twice and diff the JSONL; any
//! variance is a failure.
//!
//! # Prompt structure
//! Three optional sections (only non-empty ones are appended):
//! 1. User query          (always present)
//! 2. Conversation / file context  (`ClassifyInput.context`)
//! 3. User profile hint            (`ClassifyInput.user_hint.to_prompt_text()`)
//!
//! # Failure handling
//! Network errors, HTTP errors, JSON parse failures, and timeouts all return
//! `Err(ClassifyError::*)`. The `WithFallback` wrapper transparently retries
//! with `RegexClassifier` and increments the fallback counter.

use std::time::Duration;

use reqwest::Client;
use serde_json::Value;

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
};

/// DeepSeek hosted classifier. Constructed **once** before the eval loop so
/// model initialisation time is excluded from per-call `latency_us`.
pub struct DeepSeekClassifier {
    client: Client,
    endpoint: String,
    api_key: String,
    model: String,
    timeout: Duration,
}

impl DeepSeekClassifier {
    /// Construct from already-resolved config values.
    /// The binary (`config.rs`) reads env vars and passes them in;
    /// this library never reads env vars directly.
    ///
    /// `model` selects the model string sent in the request body:
    /// - DeepSeek direct API:  `"deepseek-chat"`
    /// - Vercel AI Gateway:    `"deepseek/deepseek-v4-flash-0731"`
    pub fn new(endpoint: String, api_key: String, model: String, timeout_ms: u64) -> Self {
        let timeout = Duration::from_millis(timeout_ms);
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .expect("failed to build reqwest::Client for DeepSeekClassifier");
        Self {
            client,
            endpoint,
            api_key,
            model,
            timeout,
        }
    }

    /// Build the prompt that is sent to DeepSeek.
    ///
    /// Sections are only appended when they carry real content, keeping
    /// the eval-mode prompt (no context, no user_hint) minimal and cheap.
    fn build_prompt(&self, input: &ClassifyInput<'_>) -> String {
        // Section: conversation / file context (from eval JSON or live router)
        let context_section = match input.context {
            Some(ctx) if !ctx.trim().is_empty() => {
                format!("\n\nConversation context:\n{}", ctx.trim())
            }
            _ => String::new(),
        };

        // Section: user profile enrichment loaded from the Nasiko DB
        let user_section = input
            .user_hint
            .as_ref()
            .and_then(|h| h.to_prompt_text())
            .map(|text| format!("\n\nUser profile:\n{}", text))
            .unwrap_or_default();

        format!(
            r#"You are a request classifier for an LLM cost-routing system.
Classify the user query into exactly one request_type and estimate its complexity.

REQUEST TYPES (pick exactly one):
- code_generation      : write new code, implement a feature, fix a bug
- code_understanding   : explain existing code, trace logic, review code
- technical_design     : pure architecture/API/system design from scratch with no evidence to analyse
- analytical_reasoning : diagnose, investigate, reconstruct failures from logs/evidence, reconcile records,
                         reason under uncertainty, calculate trade-offs — even if a fix is proposed at the end.
                         KEY RULE: if the query asks you to INVESTIGATE, DIAGNOSE, RECONSTRUCT, or REASON
                         through existing evidence/logs/data, choose analytical_reasoning NOT technical_design.
- writing              : draft text, emails, documentation, summaries
- factual_lookup       : simple fact, definition, quick reference
- general              : everything else

COMPLEXITY SCALE (integer 1-5):
1 = trivial   (typo fix, single-word lookup)
2 = simple    (basic question, small isolated change)
3 = moderate  (multi-step reasoning, some domain context needed)
4 = complex   (debugging, multi-file, design thinking required)
5 = very complex (system architecture, deep optimization, research){context}{user}

User Query: {query}

Return ONLY valid JSON, no markdown, no explanation:
{{"request_type": "<type>", "complexity": <1-5>, "confidence": <0.00-1.00>}}"#,
            context = context_section,
            user = user_section,
            query = input.query,
        )
    }
}

#[async_trait::async_trait]
impl RequestClassifier for DeepSeekClassifier {
    fn name(&self) -> &str {
        "deepseek"
    }

    async fn classify(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError> {
        let prompt = self.build_prompt(input);

        let body = serde_json::json!({
            "model": self.model.as_str(),
            "temperature": 0.0,  // deterministic: same input -> same output always
            "max_tokens": 512,   // reasoning model needs budget: ~400 thinking + ~80 JSON answer
            "messages": [{"role": "user", "content": prompt}]
        });

        // Apply the configured timeout so a slow response never blocks the eval loop.
        let resp = tokio::time::timeout(self.timeout, async {
            self.client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
        })
        .await
        .map_err(|_| ClassifyError::Timeout)?
        .map_err(|e| ClassifyError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClassifyError::Network(format!(
                "HTTP {} — {}",
                status,
                &text[..text.len().min(200)]
            )));
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ClassifyError::Parse(e.to_string()))?;

        // DeepSeek reasoning models put the chain-of-thought in a separate field
        // and the final answer in "content". Field names differ by gateway:
        //   DeepSeek direct API  → "reasoning_content"  (content has the answer)
        //   Vercel AI Gateway    → "reasoning"           (content was sometimes empty)
        //   Standard models      → no reasoning field    (content has everything)
        //
        // Strategy: use content if it has JSON, then try both reasoning field names.
        let msg = &json["choices"][0]["message"];
        let content_str = msg["content"].as_str().unwrap_or("").trim();
        // DeepSeek direct API field name
        let reasoning_content_str = msg["reasoning_content"].as_str().unwrap_or("").trim();
        // Vercel AI Gateway field name (kept for compatibility)
        let reasoning_str = msg["reasoning"].as_str().unwrap_or("").trim();

        let raw = if content_str.contains('{') {
            // Normal path: direct DeepSeek API always populates content
            content_str
        } else if reasoning_content_str.contains('{') {
            // Fallback: DeepSeek direct reasoning field (reasoning_content)
            reasoning_content_str
        } else if reasoning_str.contains('{') {
            // Fallback: Vercel gateway reasoning field name
            reasoning_str
        } else {
            return Err(ClassifyError::Parse(format!(
                "no JSON found in content={:?} reasoning_content={:?} reasoning={:?}",
                &content_str[..content_str.len().min(80)],
                &reasoning_content_str[..reasoning_content_str.len().min(80)],
                &reasoning_str[..reasoning_str.len().min(80)],
            )));
        };
        let content = raw;

        // Strip markdown fences if the model wraps the JSON in ```json ... ```
        // Some gateway/model combinations ignore the "no markdown" instruction.
        let json_str = {
            let trimmed = content.trim();
            if let Some(inner) = trimmed
                .strip_prefix("```json")
                .or_else(|| trimmed.strip_prefix("```"))
            {
                inner.trim_end_matches("```").trim()
            } else {
                trimmed
            }
        };

        let parsed: Value = serde_json::from_str(json_str)
            .map_err(|e| ClassifyError::Parse(format!("content is not valid JSON ({e}): {json_str:?}")))?;

        let request_type = parsed["request_type"]
            .as_str()
            .and_then(RequestType::from_wire)
            .ok_or_else(|| {
                ClassifyError::Parse(format!(
                    "unknown request_type value: {:?}",
                    parsed["request_type"]
                ))
            })?;

        // Clamp both fields so a misbehaving model can't produce out-of-range values.
        let complexity = parsed["complexity"]
            .as_u64()
            .map(|c| c.clamp(1, 5) as u8)
            .unwrap_or(3);

        let confidence = parsed["confidence"]
            .as_f64()
            .map(|c| c.clamp(0.0, 1.0) as f32)
            .unwrap_or(0.5);

        tracing::debug!(
            target: "nasiko::llm_router::classifier",
            backend = "deepseek",
            request_type = request_type.as_str(),
            complexity,
            confidence,
            "deepseek classifier result"
        );

        Ok(Classification {
            request_type,
            complexity,
            confidence,
        })
    }
}

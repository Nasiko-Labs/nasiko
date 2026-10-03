//! Pluggable request classification with a deterministic regex baseline and safe fallback.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::classifier::{RequestType, classify_request_type_scored};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f64,
}

#[async_trait]
pub trait RequestClassifier: Send + Sync {
    /// Short backend identifier for logs and eval output.
    fn name(&self) -> &str {
        "classifier"
    }

    /// Number of requests answered by the regex fallback instead of this backend
    /// (errors, timeouts, invalid output and below-`min_confidence` results). Backends that
    /// cannot fall back report 0.
    fn fallback_count(&self) -> u64 {
        0
    }

    async fn classify(&self, query: &str, context: Option<&str>) -> Result<Classification, String>;
}

/// Current regex classifier exposed through the common interface. Its request type is exactly
/// the existing routing classifier; complexity/confidence are deterministic evaluation signals.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexRequestClassifier;

#[async_trait]
impl RequestClassifier for RegexRequestClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, query: &str, context: Option<&str>) -> Result<Classification, String> {
        let text = context.map_or_else(|| query.to_owned(), |ctx| format!("{ctx}\n{query}"));
        let (request_type, confidence) = classify_request_type_scored(&text);
        let words = text.split_whitespace().count();
        let complexity = match words {
            0..=8 => 1,
            9..=24 => 2,
            25..=60 => 3,
            61..=120 => 4,
            _ => 5,
        };
        Ok(Classification {
            request_type,
            complexity,
            confidence,
        })
    }
}

/// Generic JSON HTTP backend. Expected response: `{request_type, complexity, confidence}`.
pub struct HttpRequestClassifier {
    client: reqwest::Client,
    endpoint: String,
    bearer_token: Option<String>,
}

impl HttpRequestClassifier {
    pub fn new(
        endpoint: String,
        bearer_token: Option<String>,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(timeout).build()?,
            endpoint,
            bearer_token,
        })
    }
}

#[derive(Serialize)]
struct ClassifyRequest<'a> {
    query: &'a str,
    context: Option<&'a str>,
}

#[derive(Deserialize)]
struct ClassifyResponse {
    request_type: String,
    complexity: u8,
    confidence: f64,
}

#[async_trait]
impl RequestClassifier for HttpRequestClassifier {
    fn name(&self) -> &str {
        "http"
    }

    async fn classify(&self, query: &str, context: Option<&str>) -> Result<Classification, String> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .json(&ClassifyRequest { query, context });
        if let Some(token) = self.bearer_token.as_deref() {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let wire: ClassifyResponse = response.json().await.map_err(|e| e.to_string())?;
        let request_type = RequestType::from_wire(&wire.request_type)
            .ok_or_else(|| "unknown request_type".to_owned())?;
        if !(1..=5).contains(&wire.complexity) {
            return Err("complexity must be between 1 and 5".into());
        }
        if !wire.confidence.is_finite() || !(0.0..=1.0).contains(&wire.confidence) {
            return Err("confidence must be between 0 and 1".into());
        }
        Ok(Classification {
            request_type,
            complexity: wire.complexity,
            confidence: wire.confidence,
        })
    }
}

const LLM_SYSTEM_PROMPT: &str = "You classify requests for a cost-aware LLM router. Reply with ONLY a JSON object \
{\"request_type\":...,\"complexity\":...,\"confidence\":...}. request_type is one of: code_generation (write/modify code), \
code_understanding (explain/trace/debug existing code), technical_design (architecture/design decisions), \
analytical_reasoning (diagnosis, estimation, multi-step logic), writing (prose, rewriting, summarising), \
factual_lookup (short fact or definition), general (chit-chat, vague, other). Classify by the work the user \
wants done, not by keywords. complexity: 1 trivial single operation; 2 straightforward; 3 multi-step with \
limited constraints; 4 substantial reasoning or design; 5 intricate cross-component reasoning. confidence is \
your probability (0-1) that request_type is right.";

/// Hosted LLM backend: any OpenAI-compatible `/chat/completions` endpoint, temperature 0.
/// `base_url` is the API root (e.g. `https://host/v1`). The model's self-reported confidence is
/// uncalibrated, so treat it as a rough signal and rely on `min_confidence` for safe fallback.
pub struct LlmRequestClassifier {
    client: reqwest::Client,
    url: String,
    model: String,
    bearer_token: Option<String>,
}

impl LlmRequestClassifier {
    pub fn new(
        base_url: &str,
        model: String,
        bearer_token: Option<String>,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(timeout).build()?,
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            model,
            bearer_token,
        })
    }
}

/// Pull the first `{...}` object out of a model reply (tolerates prose or code fences around it).
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

#[async_trait]
impl RequestClassifier for LlmRequestClassifier {
    fn name(&self) -> &str {
        "llm"
    }

    async fn classify(&self, query: &str, context: Option<&str>) -> Result<Classification, String> {
        let user = match context {
            Some(ctx) if !ctx.trim().is_empty() => format!("Context:\n{ctx}\n\nRequest:\n{query}"),
            _ => format!("Request:\n{query}"),
        };
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            // Reasoning models spend tokens before answering; too small a budget yields null content.
            "max_tokens": 1500,
            "messages": [
                {"role": "system", "content": LLM_SYSTEM_PROMPT},
                {"role": "user", "content": user},
            ],
        });
        let mut request = self.client.post(&self.url).json(&body);
        if let Some(token) = self.bearer_token.as_deref() {
            request = request.bearer_auth(token);
        }
        let reply: serde_json::Value = request
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let content = reply["choices"][0]["message"]["content"]
            .as_str()
            .ok_or("model returned no content")?;
        let json = extract_json_object(content).ok_or("no JSON object in model reply")?;
        let wire: ClassifyResponse = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let request_type = RequestType::from_wire(&wire.request_type)
            .ok_or_else(|| "unknown request_type".to_owned())?;
        if !(1..=5).contains(&wire.complexity) {
            return Err("complexity must be between 1 and 5".into());
        }
        if !wire.confidence.is_finite() || !(0.0..=1.0).contains(&wire.confidence) {
            return Err("confidence must be between 0 and 1".into());
        }
        Ok(Classification {
            request_type,
            complexity: wire.complexity,
            confidence: wire.confidence,
        })
    }
}

/// Keeps routing available when an opt-in backend errors, times out, or returns invalid data.
pub struct FallbackRequestClassifier {
    primary: Box<dyn RequestClassifier>,
    fallback: RegexRequestClassifier,
    min_confidence: f64,
    fallbacks: std::sync::atomic::AtomicU64,
}

impl FallbackRequestClassifier {
    pub fn new(primary: Box<dyn RequestClassifier>, min_confidence: f64) -> Self {
        Self {
            primary,
            fallback: RegexRequestClassifier,
            min_confidence: if min_confidence.is_finite() {
                min_confidence.clamp(0.0, 1.0)
            } else {
                1.0
            },
            fallbacks: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

/// Build the configured classifier. Missing/unknown HTTP settings fail closed to regex.
pub fn from_config(cfg: &crate::config::GatewayConfig) -> std::sync::Arc<dyn RequestClassifier> {
    match cfg.request_classifier_backend.as_str() {
        "regex" => std::sync::Arc::new(RegexRequestClassifier),
        "http" if !cfg.request_classifier_endpoint.trim().is_empty() => {
            match HttpRequestClassifier::new(
                cfg.request_classifier_endpoint.clone(),
                (!cfg.request_classifier_api_key.is_empty())
                    .then(|| cfg.request_classifier_api_key.clone()),
                Duration::from_millis(cfg.request_classifier_timeout_ms.max(1)),
            ) {
                Ok(classifier) => std::sync::Arc::new(FallbackRequestClassifier::new(
                    Box::new(classifier),
                    cfg.request_classifier_min_confidence,
                )),
                Err(error) => {
                    tracing::warn!(error = %error, "could not build HTTP request classifier; using regex");
                    std::sync::Arc::new(RegexRequestClassifier)
                }
            }
        }
        "local" => {
            let loaded = if cfg.request_classifier_model_path.trim().is_empty() {
                super::local_classifier::LocalRequestClassifier::bundled()
            } else {
                super::local_classifier::LocalRequestClassifier::from_path(
                    &cfg.request_classifier_model_path,
                )
            };
            match loaded {
                Ok(classifier) => std::sync::Arc::new(FallbackRequestClassifier::new(
                    Box::new(classifier),
                    cfg.request_classifier_min_confidence,
                )),
                Err(error) => {
                    tracing::warn!(error = %error, "could not load local request classifier; using regex");
                    std::sync::Arc::new(RegexRequestClassifier)
                }
            }
        }
        "llm"
            if !cfg.request_classifier_endpoint.trim().is_empty()
                && !cfg.request_classifier_model.trim().is_empty() =>
        {
            match LlmRequestClassifier::new(
                &cfg.request_classifier_endpoint,
                cfg.request_classifier_model.clone(),
                (!cfg.request_classifier_api_key.is_empty())
                    .then(|| cfg.request_classifier_api_key.clone()),
                Duration::from_millis(cfg.request_classifier_timeout_ms.max(1)),
            ) {
                Ok(classifier) => std::sync::Arc::new(FallbackRequestClassifier::new(
                    Box::new(classifier),
                    cfg.request_classifier_min_confidence,
                )),
                Err(error) => {
                    tracing::warn!(error = %error, "could not build LLM request classifier; using regex");
                    std::sync::Arc::new(RegexRequestClassifier)
                }
            }
        }
        backend => {
            tracing::warn!(
                backend,
                "invalid or incomplete request classifier configuration; using regex"
            );
            std::sync::Arc::new(RegexRequestClassifier)
        }
    }
}

#[async_trait]
impl RequestClassifier for FallbackRequestClassifier {
    fn name(&self) -> &str {
        "fallback-wrapped"
    }

    fn fallback_count(&self) -> u64 {
        self.fallbacks.load(std::sync::atomic::Ordering::Relaxed)
    }

    async fn classify(&self, query: &str, context: Option<&str>) -> Result<Classification, String> {
        match self.primary.classify(query, context).await {
            Ok(result)
                if (1..=5).contains(&result.complexity)
                    && result.confidence.is_finite()
                    && (self.min_confidence..=1.0).contains(&result.confidence) =>
            {
                Ok(result)
            }
            Ok(_) => {
                self.fallbacks
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!(
                    min_confidence = self.min_confidence,
                    "request classifier returned invalid or low-confidence values; using regex fallback"
                );
                self.fallback.classify(query, context).await
            }
            Err(error) => {
                self.fallbacks
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!(error = %error, "request classifier failed; using regex fallback");
                self.fallback.classify(query, context).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn regex_is_deterministic_and_accepts_context() {
        let classifier = RegexRequestClassifier;
        let first = classifier
            .classify("explain this", Some("Code review request"))
            .await
            .unwrap();
        let second = classifier
            .classify("explain this", Some("Code review request"))
            .await
            .unwrap();
        assert_eq!(first, second);
        assert!((1..=5).contains(&first.complexity));
        assert!((0.0..=1.0).contains(&first.confidence));
        for query in [
            "build me a python script that parses CSV",
            "what is the capital of France?",
            "draft an email to my team",
            "hello there",
        ] {
            assert_eq!(
                classifier.classify(query, None).await.unwrap().request_type,
                super::super::classifier::classify_request_type(query),
                "default adapter must preserve the existing regex label for {query:?}",
            );
        }
    }

    struct Broken;
    #[async_trait]
    impl RequestClassifier for Broken {
        async fn classify(&self, _: &str, _: Option<&str>) -> Result<Classification, String> {
            Err("offline".into())
        }
    }

    #[tokio::test]
    async fn backend_error_uses_regex_fallback() {
        let classifier = FallbackRequestClassifier::new(Box::new(Broken), 0.5);
        assert_eq!(
            classifier
                .classify("what is the capital of France?", None)
                .await
                .unwrap()
                .request_type,
            RequestType::FactualLookup
        );
    }

    #[tokio::test]
    async fn invalid_backend_values_use_regex_fallback() {
        struct Invalid;
        #[async_trait]
        impl RequestClassifier for Invalid {
            async fn classify(&self, _: &str, _: Option<&str>) -> Result<Classification, String> {
                Ok(Classification {
                    request_type: RequestType::Writing,
                    complexity: 9,
                    confidence: 2.0,
                })
            }
        }
        let classifier = FallbackRequestClassifier::new(Box::new(Invalid), 0.5);
        assert_eq!(
            classifier
                .classify("what is the capital of France?", None)
                .await
                .unwrap()
                .request_type,
            RequestType::FactualLookup
        );
    }

    #[tokio::test]
    async fn http_timeout_uses_regex_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/classify")
            .with_status(200)
            .with_chunked_body(|writer| {
                std::thread::sleep(Duration::from_millis(100));
                std::io::Write::write_all(
                    writer,
                    br#"{"request_type":"writing","complexity":2,"confidence":0.8}"#,
                )
            })
            .create_async()
            .await;
        let http =
            HttpRequestClassifier::new(server.url() + "/classify", None, Duration::from_millis(5))
                .unwrap();
        let classifier = FallbackRequestClassifier::new(Box::new(http), 0.5);
        let result = classifier
            .classify("what is the capital of France?", None)
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::FactualLookup);
    }

    #[tokio::test]
    async fn fallbacks_are_counted_only_when_they_happen() {
        let ok = FallbackRequestClassifier::new(Box::new(RegexRequestClassifier), 0.0);
        ok.classify("hello", None).await.unwrap();
        assert_eq!(ok.fallback_count(), 0);

        let broken = FallbackRequestClassifier::new(Box::new(Broken), 0.5);
        broken.classify("hello", None).await.unwrap();
        broken.classify("hello again", None).await.unwrap();
        assert_eq!(broken.fallback_count(), 2);
    }

    #[test]
    fn local_backend_is_selected_by_config_and_bad_path_falls_back_to_regex() {
        let mut cfg = crate::config::GatewayConfig::default();
        cfg.request_classifier_backend = "local".into();
        let local = from_config(&cfg);
        assert_eq!(local.name(), "fallback-wrapped");

        cfg.request_classifier_model_path = "/nonexistent/model.json".into();
        assert_eq!(from_config(&cfg).name(), "regex");

        cfg.request_classifier_backend = "bogus".into();
        assert_eq!(from_config(&cfg).name(), "regex");
    }

    #[tokio::test]
    async fn unknown_http_label_uses_regex_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/classify")
            .with_status(200)
            .with_body(r#"{"request_type":"unsupported","complexity":2,"confidence":0.8}"#)
            .create_async()
            .await;
        let http =
            HttpRequestClassifier::new(server.url() + "/classify", None, Duration::from_secs(1))
                .unwrap();
        let classifier = FallbackRequestClassifier::new(Box::new(http), 0.5);
        let result = classifier
            .classify("what is the capital of France?", None)
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::FactualLookup);
    }

    #[tokio::test]
    async fn llm_backend_parses_reply_and_sends_temperature_zero() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer k")
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"model": "m", "temperature": 0}),
            ))
            .with_status(200)
            .with_body(
                r#"{"choices":[{"message":{"content":"```json\n{\"request_type\":\"writing\",\"complexity\":2,\"confidence\":0.9}\n```"}}]}"#,
            )
            .create_async()
            .await;
        let llm = LlmRequestClassifier::new(
            &(server.url() + "/v1/"),
            "m".into(),
            Some("k".into()),
            Duration::from_secs(1),
        )
        .unwrap();
        let out = llm.classify("draft an email", None).await.unwrap();
        assert_eq!(out.request_type, RequestType::Writing);
        assert_eq!(out.complexity, 2);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn llm_null_content_uses_regex_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":null,"reasoning":"..."}}]}"#)
            .create_async()
            .await;
        let llm =
            LlmRequestClassifier::new(&server.url(), "m".into(), None, Duration::from_secs(1))
                .unwrap();
        let classifier = FallbackRequestClassifier::new(Box::new(llm), 0.5);
        let out = classifier
            .classify("what is the capital of France?", None)
            .await
            .unwrap();
        assert_eq!(out.request_type, RequestType::FactualLookup);
    }

    #[tokio::test]
    async fn low_confidence_uses_regex_fallback() {
        struct LowConfidence;
        #[async_trait]
        impl RequestClassifier for LowConfidence {
            async fn classify(&self, _: &str, _: Option<&str>) -> Result<Classification, String> {
                Ok(Classification {
                    request_type: RequestType::Writing,
                    complexity: 2,
                    confidence: 0.49,
                })
            }
        }
        let classifier = FallbackRequestClassifier::new(Box::new(LowConfidence), 0.5);
        assert_eq!(
            classifier
                .classify("what is the capital of France?", None)
                .await
                .unwrap()
                .request_type,
            RequestType::FactualLookup
        );
    }
}

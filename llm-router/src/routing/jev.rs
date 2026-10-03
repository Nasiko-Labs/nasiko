//! Jev (typesafe.ai) hosted backend for the request classifier.
//!
//! Jev is a "System One" model: it answers typed questions about a `state` rather than
//! generating text. One HTTPS `POST` carries the query and context as state plus two narrow
//! questions — a **Choice** over the seven [`RequestType`]s and a **Score** over the five
//! complexity levels — and returns a probability distribution for each. Verified against
//! `docs.typesafe.ai/api`, `/primitives/choice`, `/primitives/score`, `/confidence` and
//! `/models` (October 2026); the request/response shapes below are those documents', not
//! an OpenAI-compatible guess.
//!
//! What this adapter does and does not promise:
//!
//! - **Request shape.** `{"model", "state": {"query", "context"?}, "questions": {…}}`.
//!   Only the query and context ever leave the process — never an eval label, a system
//!   prompt or a credential.
//! - **Model pinning.** The versioned id from config (`jev-1.13.0` by default). The
//!   response's own `model` field is recorded as the version that actually answered;
//!   `jev-latest` is a moving alias and is not treated as reproducible.
//! - **Public confidence** is the probability Jev assigns to the chosen request type.
//!   Jev's own `confidence` field is a spread statistic ("1 when all the probability is
//!   on one outcome and 0 when spread evenly"), *not* a probability of correctness; it is
//!   kept separately as `vendor_type_confidence`.
//! - **Complexity** is the mode of the Score distribution (ties → the lower level) mapped
//!   from Jev's 0-based levels to the rubric's 1..=5. The probability-weighted `score`
//!   is reported as `complexity_expected` so the alternative rounding rule can be
//!   evaluated offline from the same run. See [`complexity_level_from_probabilities`].
//! - **Validation.** Types, labels, level keys, probability ranges and sums (±0.02),
//!   finiteness, argmax consistency and the answer set are all checked; anything else is
//!   [`ClassifyError::InvalidResponse`] and the service falls back.
//! - **Bounds.** Input is capped by the service; the response body is capped at
//!   [`MAX_RESPONSE_BYTES`]; in-flight calls are capped by a semaphore; the whole call
//!   (queueing, connect, attempts, backoff, parsing) sits under one deadline.
//! - **Credential hygiene.** The key is sent only to the configured endpoint; the client
//!   follows no redirects; errors carry status codes, never bodies or headers.
//! - **Determinism.** Jev documents no seed or temperature. Repeatability across runs is
//!   measured, not assumed (see the live test and `docs/classifier/RESULTS.md`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;

use super::classifier::{
    BackendDiagnostics, Classification, Classified, ClassifyError, ClassifyInput,
    RequestClassifier, RequestType,
};
use crate::config::{ClassifierConfig, Secret};

/// Largest response body accepted. A valid answer is a few hundred bytes.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Probability mass tolerance for "sums to one".
const SUM_TOLERANCE: f32 = 0.02;
/// Base backoff before a retry; doubles per attempt, always inside the deadline.
const RETRY_BACKOFF: Duration = Duration::from_millis(200);

pub use super::rubric::RUBRIC_VERSION;
use super::rubric::{
    COMPLEXITY_CRITERIA, COMPLEXITY_INSTRUCTIONS, TYPE_INSTRUCTIONS, type_criteria,
};

const TYPE_QUESTION: &str = "request_type";
const COMPLEXITY_QUESTION: &str = "complexity";

/// Order of the Choice options on the wire. Jev 1.13 leans toward earlier options, so the
/// live robustness check classifies with both orders and reports disagreement; routing
/// always uses `Canonical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionOrder {
    Canonical,
    Reversed,
}

/// The hosted backend. Build once ([`JevClassifier::new`]) and share.
pub struct JevClassifier {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    model: String,
    api_key: Secret,
    deadline: Duration,
    retries: u32,
    gate: Arc<Semaphore>,
    option_order: OptionOrder,
}

impl std::fmt::Debug for JevClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JevClassifier")
            .field("endpoint", &self.endpoint.as_str())
            .field("model", &self.model)
            .field("api_key", &self.api_key)
            .field("deadline", &self.deadline)
            .field("retries", &self.retries)
            .finish()
    }
}

impl JevClassifier {
    /// Validate config and build the dedicated HTTP client. Fails (an init error the
    /// service counts) on a missing key, an unusable endpoint or an empty model id. No
    /// network I/O.
    pub fn new(cfg: &ClassifierConfig) -> Result<Self, ClassifyError> {
        if cfg.api_key.is_empty() {
            return Err(ClassifyError::Init(
                "TYPESAFE_API_KEY is not set (required for CLASSIFIER_BACKEND=jev)".into(),
            ));
        }
        let endpoint = reqwest::Url::parse(cfg.endpoint.trim())
            .map_err(|e| ClassifyError::Init(format!("CLASSIFIER_ENDPOINT is not a URL: {e}")))?;
        let loopback = matches!(
            endpoint.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        );
        if !(endpoint.scheme() == "https" || (endpoint.scheme() == "http" && loopback)) {
            return Err(ClassifyError::Init(
                "CLASSIFIER_ENDPOINT must be https:// (http:// is allowed only for loopback)"
                    .into(),
            ));
        }
        if cfg.model.trim().is_empty() {
            return Err(ClassifyError::Init("CLASSIFIER_MODEL is empty".into()));
        }
        let deadline = Duration::from_millis(cfg.timeout_ms.max(1));
        let http = reqwest::Client::builder()
            // The key must only ever reach the configured endpoint: a redirect, even to the
            // same host, is refused rather than followed.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(deadline)
            .connect_timeout(deadline)
            .build()
            .map_err(|e| ClassifyError::Init(format!("http client: {e}")))?;
        Ok(Self {
            http,
            endpoint,
            model: cfg.model.trim().to_string(),
            api_key: cfg.api_key.clone(),
            deadline,
            retries: cfg.retries,
            gate: Arc::new(Semaphore::new(cfg.max_concurrency.max(1))),
            option_order: OptionOrder::Canonical,
        })
    }

    /// Reorder the Choice options (robustness experiments only).
    pub fn with_option_order(mut self, order: OptionOrder) -> Self {
        self.option_order = order;
        self
    }

    /// The exact body sent for `input` — exposed so tests and the eval sidecar can pin
    /// the request shape and prove no label ever rides along.
    pub fn request_body(&self, input: &ClassifyInput<'_>) -> JevRequest {
        let mut criteria = type_criteria();
        if self.option_order == OptionOrder::Reversed {
            criteria.reverse();
        }
        JevRequest {
            model: self.model.clone(),
            state: JevState {
                query: input.query.to_string(),
                context: input.context.map(str::to_string),
            },
            questions: JevQuestions {
                request_type: ChoiceQuestion {
                    kind: "choice",
                    instructions: TYPE_INSTRUCTIONS,
                    criteria: OrderedMap(criteria),
                },
                complexity: ScoreQuestion {
                    kind: "score",
                    instructions: COMPLEXITY_INSTRUCTIONS,
                    criteria: COMPLEXITY_CRITERIA.to_vec(),
                },
            },
        }
    }

    async fn post_once(&self, body: &JevRequest, remaining: Duration) -> AttemptOutcome {
        let resp = match self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(self.api_key.expose())
            .timeout(remaining)
            .json(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return AttemptOutcome::Final(ClassifyError::Timeout),
            Err(e) if e.is_connect() || e.is_request() => {
                return AttemptOutcome::Retryable(ClassifyError::Network(sanitize(&e)));
            }
            Err(e) => return AttemptOutcome::Final(ClassifyError::Network(sanitize(&e))),
        };
        let status = resp.status();
        if !status.is_success() {
            let err = ClassifyError::Upstream {
                status: status.as_u16(),
                detail: match status.as_u16() {
                    401 => "missing or invalid API key".into(),
                    422 => "request rejected by validation".into(),
                    429 => "rate limited".into(),
                    529 => "service overloaded".into(),
                    _ => status
                        .canonical_reason()
                        .unwrap_or("upstream error")
                        .to_ascii_lowercase(),
                },
            };
            return if matches!(status.as_u16(), 429 | 529) || status.is_server_error() {
                AttemptOutcome::Retryable(err)
            } else {
                AttemptOutcome::Final(err)
            };
        }
        if let Some(len) = resp.content_length()
            && len as usize > MAX_RESPONSE_BYTES
        {
            return AttemptOutcome::Final(ClassifyError::InvalidResponse(format!(
                "response body {len} bytes exceeds the {MAX_RESPONSE_BYTES}-byte cap"
            )));
        }
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) if e.is_timeout() => return AttemptOutcome::Final(ClassifyError::Timeout),
            Err(e) => return AttemptOutcome::Final(ClassifyError::Network(sanitize(&e))),
        };
        if bytes.len() > MAX_RESPONSE_BYTES {
            return AttemptOutcome::Final(ClassifyError::InvalidResponse(format!(
                "response body {} bytes exceeds the {MAX_RESPONSE_BYTES}-byte cap",
                bytes.len()
            )));
        }
        AttemptOutcome::Body(bytes)
    }
}

enum AttemptOutcome {
    Body(bytes::Bytes),
    Retryable(ClassifyError),
    Final(ClassifyError),
}

/// A reqwest error without its URL (which could carry a query string) — status and kind
/// are what diagnostics need.
fn sanitize(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_request() {
        "request"
    } else if e.is_body() || e.is_decode() {
        "body"
    } else {
        "other"
    };
    // `source()` is the transport-level cause (DNS, TLS, reset) without the URL, which
    // `Display` would include.
    let cause = std::error::Error::source(e)
        .map(|s| s.to_string())
        .unwrap_or_default();
    format!("{kind}: {}", cause.chars().take(160).collect::<String>())
}

#[async_trait]
impl RequestClassifier for JevClassifier {
    fn name(&self) -> &str {
        "jev"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        self.classify_detailed(input)
            .await
            .map(|c| c.classification)
    }

    async fn classify_detailed(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classified, ClassifyError> {
        let started = Instant::now();
        let deadline = started + self.deadline;
        let remaining = || deadline.saturating_duration_since(Instant::now());

        // Concurrency bound, inside the deadline so a queue never outlives it.
        let _permit = tokio::time::timeout(remaining(), self.gate.acquire())
            .await
            .map_err(|_| ClassifyError::Timeout)?
            .map_err(|_| ClassifyError::Init("classifier semaphore closed".into()))?;

        let body = self.request_body(input);
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let left = remaining();
            if left.is_zero() {
                return Err(ClassifyError::Timeout);
            }
            match self.post_once(&body, left).await {
                AttemptOutcome::Body(bytes) => {
                    let mut classified = parse_response(&bytes)?;
                    if let Some(d) = classified.diagnostics.as_mut() {
                        d.attempts = attempt;
                    }
                    return Ok(classified);
                }
                AttemptOutcome::Final(e) => return Err(e),
                AttemptOutcome::Retryable(e) => {
                    let backoff = RETRY_BACKOFF * 2u32.saturating_pow(attempt - 1);
                    // Retry only if allowed and if the backoff plus a minimal request still
                    // fits the deadline; otherwise the retryable error is the final one.
                    if attempt > self.retries || remaining() <= backoff + Duration::from_millis(50)
                    {
                        return Err(e);
                    }
                    tracing::debug!(
                        target: "nasiko::llm_router::classifier",
                        attempt, backoff_ms = backoff.as_millis() as u64, error = %e,
                        "jev: retryable failure; backing off inside the deadline"
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}

// ── wire types ─────────────────────────────────────────────────────────────────────────

/// A JSON object whose key order is exactly the vector's order. `serde_json::Value`
/// reorders keys alphabetically (no `preserve_order`), which would silently fix the
/// option order Jev is sensitive to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedMap<V>(pub Vec<(&'static str, V)>);

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevRequest {
    pub model: String,
    pub state: JevState,
    pub questions: JevQuestions,
}

/// The state: clearly separated fields, so the model never has to guess where the request
/// ends and the surrounding material begins.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevState {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevQuestions {
    pub request_type: ChoiceQuestion,
    pub complexity: ScoreQuestion,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChoiceQuestion {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub instructions: &'static str,
    pub criteria: OrderedMap<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoreQuestion {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub instructions: &'static str,
    pub criteria: Vec<&'static str>,
}

#[derive(Debug, Deserialize)]
struct JevResponse {
    model: Option<String>,
    answers: serde_json::Map<String, Value>,
    #[serde(default)]
    usage: Option<JevUsage>,
}

#[derive(Debug, Deserialize)]
struct JevUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChoiceAnswer {
    #[serde(rename = "type")]
    kind: String,
    choice: String,
    probabilities: serde_json::Map<String, Value>,
    confidence: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ScoreAnswer {
    #[serde(rename = "type")]
    kind: String,
    score: Option<f64>,
    probabilities: serde_json::Map<String, Value>,
    confidence: Option<f64>,
}

fn invalid(msg: impl Into<String>) -> ClassifyError {
    ClassifyError::InvalidResponse(msg.into())
}

/// Parse and validate a 2xx body into a classification plus diagnostics. Every documented
/// guarantee is checked rather than trusted; a violation is an invalid response (fallback),
/// never a confident success.
pub fn parse_response(bytes: &[u8]) -> Result<Classified, ClassifyError> {
    let resp: JevResponse = serde_json::from_slice(bytes)
        .map_err(|e| invalid(format!("body is not the documented JSON shape: {e}")))?;

    let type_raw = resp
        .answers
        .get(TYPE_QUESTION)
        .ok_or_else(|| invalid("missing answer: request_type"))?;
    let choice: ChoiceAnswer = serde_json::from_value(type_raw.clone())
        .map_err(|e| invalid(format!("request_type answer malformed: {e}")))?;
    if choice.kind != "choice" {
        return Err(invalid(format!(
            "request_type answer type '{}' is not 'choice'",
            choice.kind
        )));
    }
    let type_probabilities = validate_choice_probabilities(&choice.probabilities)?;
    let request_type = RequestType::from_wire(&choice.choice)
        .ok_or_else(|| invalid(format!("unknown request_type label '{}'", choice.choice)))?;
    let chosen_p = type_probabilities
        .iter()
        .find(|(rt, _)| *rt == request_type)
        .map(|(_, p)| *p)
        .unwrap_or(0.0);
    let max_p = type_probabilities
        .iter()
        .map(|(_, p)| *p)
        .fold(0.0, f32::max);
    if chosen_p + 1e-4 < max_p {
        return Err(invalid("choice is not the most probable option"));
    }
    let vendor_type_confidence = finite_unit(choice.confidence, "request_type.confidence")?;

    let score_raw = resp
        .answers
        .get(COMPLEXITY_QUESTION)
        .ok_or_else(|| invalid("missing answer: complexity"))?;
    let score: ScoreAnswer = serde_json::from_value(score_raw.clone())
        .map_err(|e| invalid(format!("complexity answer malformed: {e}")))?;
    if score.kind != "score" {
        return Err(invalid(format!(
            "complexity answer type '{}' is not 'score'",
            score.kind
        )));
    }
    let complexity_probabilities = validate_score_probabilities(&score.probabilities)?;
    let complexity = complexity_level_from_probabilities(&complexity_probabilities);
    let expected = match score.score {
        Some(s) if s.is_finite() && (-1e-3..=4.0 + 1e-3).contains(&s) => Some(s as f32 + 1.0),
        Some(s) => return Err(invalid(format!("complexity score {s} outside 0..=4"))),
        None => None,
    };
    let vendor_complexity_confidence = finite_unit(score.confidence, "complexity.confidence")?;

    let classification = Classification {
        request_type,
        complexity,
        confidence: chosen_p,
    };
    classification.validate()?;
    Ok(Classified {
        classification,
        diagnostics: Some(BackendDiagnostics {
            model_version: resp.model,
            type_probabilities,
            complexity_probabilities,
            complexity_expected: expected,
            vendor_type_confidence,
            vendor_complexity_confidence,
            input_tokens: resp.usage.as_ref().and_then(|u| u.input_tokens),
            output_tokens: resp.usage.as_ref().and_then(|u| u.output_tokens),
            attempts: 1,
            input_truncated: false,
        }),
    })
}

fn finite_unit(v: Option<f64>, what: &str) -> Result<Option<f32>, ClassifyError> {
    match v {
        None => Ok(None),
        Some(x) if x.is_finite() && (0.0..=1.0).contains(&x) => Ok(Some(x as f32)),
        Some(x) => Err(invalid(format!(
            "{what} {x} is not a finite number in 0..=1"
        ))),
    }
}

fn probability(v: &Value, what: &str) -> Result<f32, ClassifyError> {
    let p = v
        .as_f64()
        .ok_or_else(|| invalid(format!("{what} probability is not a number")))?;
    if !p.is_finite() || !(0.0..=1.0 + 1e-6).contains(&p) {
        return Err(invalid(format!("{what} probability {p} outside 0..=1")));
    }
    Ok(p.min(1.0) as f32)
}

fn check_sum(ps: impl Iterator<Item = f32>, what: &str) -> Result<(), ClassifyError> {
    let sum: f32 = ps.sum();
    if (sum - 1.0).abs() > SUM_TOLERANCE {
        return Err(invalid(format!(
            "{what} probabilities sum to {sum:.4}, not 1"
        )));
    }
    Ok(())
}

/// All seven labels present, nothing else, each a probability, summing to one. Returned in
/// [`RequestType::ALL`] order regardless of the wire order.
fn validate_choice_probabilities(
    map: &serde_json::Map<String, Value>,
) -> Result<Vec<(RequestType, f32)>, ClassifyError> {
    for key in map.keys() {
        if RequestType::from_wire(key).is_none() {
            return Err(invalid(format!(
                "unknown request_type option '{key}' in probabilities"
            )));
        }
    }
    let mut out = Vec::with_capacity(7);
    for rt in RequestType::ALL {
        let v = map
            .get(rt.as_str())
            .ok_or_else(|| invalid(format!("probabilities missing option '{}'", rt.as_str())))?;
        out.push((rt, probability(v, rt.as_str())?));
    }
    check_sum(out.iter().map(|(_, p)| *p), "request_type")?;
    Ok(out)
}

/// Level keys exactly `"0".."4"`, each a probability, summing to one. Returned in level
/// order (index `i` ⇒ rubric level `i + 1`).
fn validate_score_probabilities(
    map: &serde_json::Map<String, Value>,
) -> Result<Vec<f32>, ClassifyError> {
    if map.len() != COMPLEXITY_CRITERIA.len() {
        return Err(invalid(format!(
            "complexity probabilities have {} levels, expected {}",
            map.len(),
            COMPLEXITY_CRITERIA.len()
        )));
    }
    let mut out = Vec::with_capacity(COMPLEXITY_CRITERIA.len());
    for i in 0..COMPLEXITY_CRITERIA.len() {
        let key = i.to_string();
        let v = map
            .get(&key)
            .ok_or_else(|| invalid(format!("complexity probabilities missing level '{key}'")))?;
        out.push(probability(v, &format!("complexity level {key}"))?);
    }
    check_sum(out.iter().copied(), "complexity")?;
    Ok(out)
}

/// The frozen conversion from Jev's five 0-based levels to the rubric's 1..=5: the **mode**
/// of the distribution, ties resolved to the **lower** level (the first maximum in level
/// order). Chosen over rounding the probability-weighted `score` because the mode is what
/// the model most believes, is unaffected by mass spread symmetrically around it, and is
/// exactly reproducible from the reported distribution. The expected value is still carried
/// in diagnostics so the rounding rule can be compared offline; neither rule was tuned
/// against the held-out set.
pub fn complexity_level_from_probabilities(levels: &[f32]) -> u8 {
    let mut best = 0usize;
    for (i, p) in levels.iter().enumerate() {
        if *p > levels[best] {
            best = i;
        }
    }
    (best as u8 + 1).clamp(1, 5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClassifierBackend;
    use serde_json::json;

    fn cfg(endpoint: &str) -> ClassifierConfig {
        ClassifierConfig {
            backend: ClassifierBackend::Jev,
            endpoint: endpoint.into(),
            model: "jev-1.13.0".into(),
            api_key: Secret::new("sk-test-key"),
            timeout_ms: 2000,
            min_confidence: 0.0,
            routing_seed: None,
            max_concurrency: 2,
            retries: 0,
            ..Default::default()
        }
    }

    fn good_body() -> Value {
        json!({
            "model": "jev-1.13.0",
            "answers": {
                "request_type": {
                    "type": "choice",
                    "choice": "technical_design",
                    "confidence": 0.71,
                    "probabilities": {
                        "code_generation": 0.05, "code_understanding": 0.02,
                        "technical_design": 0.80, "analytical_reasoning": 0.08,
                        "writing": 0.02, "factual_lookup": 0.01, "general": 0.02
                    }
                },
                "complexity": {
                    "type": "score",
                    "score": 3.1,
                    "confidence": 0.6,
                    "legend": {"0":"a","1":"b","2":"c","3":"d","4":"e"},
                    "probabilities": {"0": 0.0, "1": 0.05, "2": 0.15, "3": 0.45, "4": 0.35}
                }
            },
            "usage": {"input_tokens": 612, "output_tokens": 40}
        })
    }

    fn input() -> ClassifyInput<'static> {
        ClassifyInput {
            query: "Design migration from callbacks to a queue",
            context: Some("POST /confirm returns 200 after charge"),
        }
    }

    // ── init ───────────────────────────────────────────────────────────────────────

    #[test]
    fn init_requires_key_https_and_model() {
        let mut c = cfg("https://api.typesafe.ai/v1/systemone");
        c.api_key = Secret::default();
        assert!(
            matches!(JevClassifier::new(&c), Err(ClassifyError::Init(m)) if m.contains("TYPESAFE_API_KEY"))
        );

        let c = cfg("http://api.typesafe.ai/v1/systemone");
        assert!(
            matches!(JevClassifier::new(&c), Err(ClassifyError::Init(m)) if m.contains("https"))
        );

        let c = cfg("not a url");
        assert!(matches!(
            JevClassifier::new(&c),
            Err(ClassifyError::Init(_))
        ));

        let mut c = cfg("https://api.typesafe.ai/v1/systemone");
        c.model = " ".into();
        assert!(
            matches!(JevClassifier::new(&c), Err(ClassifyError::Init(m)) if m.contains("CLASSIFIER_MODEL"))
        );

        // loopback over http is allowed (local mocks); Debug never shows the key.
        let j = JevClassifier::new(&cfg("http://127.0.0.1:1/x")).unwrap();
        assert!(!format!("{j:?}").contains("sk-test-key"));
        assert_eq!(j.name(), "jev");
    }

    // ── request shape ──────────────────────────────────────────────────────────────

    #[test]
    fn request_body_matches_the_documented_api_and_carries_no_labels() {
        let j = JevClassifier::new(&cfg("http://127.0.0.1:1/x")).unwrap();
        let body = serde_json::to_value(j.request_body(&input())).unwrap();
        assert_eq!(body["model"], "jev-1.13.0");
        assert_eq!(body["state"]["query"], input().query);
        assert_eq!(body["state"]["context"], input().context.unwrap());
        assert_eq!(body["questions"]["request_type"]["type"], "choice");
        assert_eq!(body["questions"]["complexity"]["type"], "score");
        assert_eq!(
            body["questions"]["complexity"]["criteria"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        let crit = body["questions"]["request_type"]["criteria"]
            .as_object()
            .unwrap();
        assert_eq!(crit.len(), 7);
        for rt in RequestType::ALL {
            assert!(crit.contains_key(rt.as_str()));
        }
        // Only the three documented top-level fields; nothing that looks like a label,
        // expected answer, test tag or seed.
        assert_eq!(body.as_object().unwrap().len(), 3);
        let text = body.to_string();
        for forbidden in [
            "tier_hypothesis",
            "\"tests\"",
            "expected",
            "seed",
            "temperature",
        ] {
            assert!(
                !text.contains(forbidden),
                "{forbidden} leaked into the request"
            );
        }
        // No context ⇒ the key is absent, not null.
        let no_ctx = serde_json::to_value(j.request_body(&ClassifyInput {
            query: "hi",
            context: None,
        }))
        .unwrap();
        assert!(no_ctx["state"].get("context").is_none());
    }

    #[test]
    fn option_order_is_preserved_on_the_wire_and_reversible() {
        let j = JevClassifier::new(&cfg("http://127.0.0.1:1/x")).unwrap();
        let s = serde_json::to_string(&j.request_body(&input())).unwrap();
        let first = s.find("\"code_generation\"").unwrap();
        let last = s.find("\"general\"").unwrap();
        assert!(
            first < last,
            "canonical order must start with code_generation"
        );
        let j = j.with_option_order(OptionOrder::Reversed);
        let s = serde_json::to_string(&j.request_body(&input())).unwrap();
        assert!(s.find("\"general\"").unwrap() < s.find("\"code_generation\"").unwrap());
    }

    // ── response validation ────────────────────────────────────────────────────────

    #[test]
    fn parses_a_documented_response_into_a_classification_and_diagnostics() {
        let c = parse_response(good_body().to_string().as_bytes()).unwrap();
        assert_eq!(c.classification.request_type, RequestType::TechnicalDesign);
        // Public confidence is the chosen option's probability, not Jev's spread statistic.
        assert!((c.classification.confidence - 0.80).abs() < 1e-6);
        // Mode of {0,.05,.15,.45,.35} is level index 3 ⇒ rubric 4.
        assert_eq!(c.classification.complexity, 4);
        let d = c.diagnostics.unwrap();
        assert_eq!(d.model_version.as_deref(), Some("jev-1.13.0"));
        assert_eq!(d.vendor_type_confidence, Some(0.71));
        assert!((d.complexity_expected.unwrap() - 4.1).abs() < 1e-5);
        assert_eq!(d.input_tokens, Some(612));
        assert_eq!(d.type_probabilities.len(), 7);
        assert_eq!(d.type_probabilities[0].0, RequestType::CodeGeneration);
    }

    fn expect_invalid(body: Value, needle: &str) {
        match parse_response(body.to_string().as_bytes()) {
            Err(ClassifyError::InvalidResponse(m)) => {
                assert!(m.contains(needle), "expected '{needle}' in '{m}'")
            }
            other => panic!("expected invalid response containing '{needle}', got {other:?}"),
        }
    }

    #[test]
    fn malformed_and_out_of_contract_responses_are_rejected() {
        expect_invalid(json!({"nope": 1}), "documented JSON shape");
        // Missing an answer.
        let mut b = good_body();
        b["answers"].as_object_mut().unwrap().remove("complexity");
        expect_invalid(b, "missing answer: complexity");
        // Wrong primitive type.
        let mut b = good_body();
        b["answers"]["request_type"]["type"] = json!("noul");
        expect_invalid(b, "not 'choice'");
        // Unknown label.
        let mut b = good_body();
        b["answers"]["request_type"]["choice"] = json!("poetry");
        b["answers"]["request_type"]["probabilities"]["poetry"] = json!(0.0);
        expect_invalid(b, "unknown request_type option");
        // Missing option.
        let mut b = good_body();
        b["answers"]["request_type"]["probabilities"]
            .as_object_mut()
            .unwrap()
            .remove("writing");
        expect_invalid(b, "missing option 'writing'");
        // Probabilities not summing to one.
        let mut b = good_body();
        b["answers"]["request_type"]["probabilities"]["general"] = json!(0.5);
        expect_invalid(b, "sum to");
        // Out-of-range probability.
        let mut b = good_body();
        b["answers"]["complexity"]["probabilities"]["3"] = json!(1.4);
        b["answers"]["complexity"]["probabilities"]["4"] = json!(-0.6);
        expect_invalid(b, "outside 0..=1");
        // Non-numeric probability.
        let mut b = good_body();
        b["answers"]["complexity"]["probabilities"]["3"] = json!("high");
        expect_invalid(b, "not a number");
        // Choice contradicts the distribution.
        let mut b = good_body();
        b["answers"]["request_type"]["choice"] = json!("writing");
        expect_invalid(b, "not the most probable");
        // Wrong number of complexity levels.
        let mut b = good_body();
        b["answers"]["complexity"]["probabilities"] = json!({"0":0.5,"1":0.5});
        expect_invalid(b, "levels");
        // Score outside the level range.
        let mut b = good_body();
        b["answers"]["complexity"]["score"] = json!(7.0);
        expect_invalid(b, "outside 0..=4");
        // Vendor confidence not a unit value.
        let mut b = good_body();
        b["answers"]["request_type"]["confidence"] = json!(1.5);
        expect_invalid(b, "request_type.confidence");
    }

    #[test]
    fn complexity_conversion_covers_every_level_ties_and_bounds() {
        for level in 0..5 {
            let mut p = vec![0.0; 5];
            p[level] = 1.0;
            assert_eq!(complexity_level_from_probabilities(&p), level as u8 + 1);
        }
        // Exact tie ⇒ the lower level.
        assert_eq!(
            complexity_level_from_probabilities(&[0.0, 0.5, 0.5, 0.0, 0.0]),
            2
        );
        assert_eq!(
            complexity_level_from_probabilities(&[0.2, 0.2, 0.2, 0.2, 0.2]),
            1
        );
        // Mass spread around a peak still returns the peak, unlike rounding the mean.
        assert_eq!(
            complexity_level_from_probabilities(&[0.3, 0.0, 0.4, 0.0, 0.3]),
            3
        );
        // Expected-value disagreement case: mean 2.0 (⇒ 3) but mode is level 0 (⇒ 1).
        assert_eq!(
            complexity_level_from_probabilities(&[0.4, 0.1, 0.1, 0.1, 0.3]),
            1
        );
        // Degenerate inputs stay in bounds.
        assert_eq!(complexity_level_from_probabilities(&[]), 1);
        assert_eq!(complexity_level_from_probabilities(&[0.0; 9]), 1);
    }

    // ── HTTP behaviour against a local mock ────────────────────────────────────────

    async fn mock_server() -> mockito::ServerGuard {
        mockito::Server::new_async().await
    }

    #[tokio::test]
    async fn success_round_trip_sends_bearer_key_json_and_returns_diagnostics() {
        let mut server = mock_server().await;
        let m = server
            .mock("POST", "/v1/systemone")
            .match_header("authorization", "Bearer sk-test-key")
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "jev-1.13.0",
                "state": {"query": input().query, "context": input().context.unwrap()},
            })))
            .with_status(200)
            .with_body(good_body().to_string())
            .create_async()
            .await;
        let j = JevClassifier::new(&cfg(&format!("{}/v1/systemone", server.url()))).unwrap();
        let out = j.classify_detailed(&input()).await.unwrap();
        m.assert_async().await;
        assert_eq!(
            out.classification.request_type,
            RequestType::TechnicalDesign
        );
        assert_eq!(out.diagnostics.unwrap().attempts, 1);
        // The simple `classify` path gives the same verdict.
        let plain = j.classify(&input()).await.unwrap();
        assert_eq!(plain, out.classification);
    }

    #[tokio::test]
    async fn upstream_statuses_map_to_typed_errors_without_leaking_bodies() {
        for (status, needle) in [
            (401, "invalid API key"),
            (422, "validation"),
            (429, "rate limited"),
            (529, "overloaded"),
            (500, "internal server error"),
        ] {
            let mut server = mock_server().await;
            let _m = server
                .mock("POST", "/x")
                .with_status(status)
                .with_body("{\"error\":\"SECRET-BODY query text echoed\"}")
                .create_async()
                .await;
            let j = JevClassifier::new(&cfg(&format!("{}/x", server.url()))).unwrap();
            let err = j.classify(&input()).await.unwrap_err();
            match err {
                ClassifyError::Upstream { status: s, detail } => {
                    assert_eq!(s as usize, status);
                    assert!(detail.contains(needle), "{status}: {detail}");
                    assert!(!detail.contains("SECRET-BODY"));
                }
                other => panic!("{status}: expected Upstream, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn malformed_2xx_body_is_an_invalid_response() {
        let mut server = mock_server().await;
        let _m = server
            .mock("POST", "/x")
            .with_status(200)
            .with_body("{\"model\":\"jev-1.13.0\",\"answers\":{}}")
            .create_async()
            .await;
        let j = JevClassifier::new(&cfg(&format!("{}/x", server.url()))).unwrap();
        assert!(matches!(
            j.classify(&input()).await,
            Err(ClassifyError::InvalidResponse(m)) if m.contains("missing answer")
        ));
    }

    #[tokio::test]
    async fn oversized_body_is_rejected() {
        let mut server = mock_server().await;
        let _m = server
            .mock("POST", "/x")
            .with_status(200)
            .with_body("x".repeat(MAX_RESPONSE_BYTES + 1))
            .create_async()
            .await;
        let j = JevClassifier::new(&cfg(&format!("{}/x", server.url()))).unwrap();
        assert!(matches!(
            j.classify(&input()).await,
            Err(ClassifyError::InvalidResponse(m)) if m.contains("cap")
        ));
    }

    #[tokio::test]
    async fn connection_refused_is_a_network_error() {
        // Port 1 refuses immediately.
        let j = JevClassifier::new(&cfg("http://127.0.0.1:1/x")).unwrap();
        assert!(matches!(
            j.classify(&input()).await,
            Err(ClassifyError::Network(_))
        ));
    }

    #[tokio::test]
    async fn slow_upstream_times_out_within_the_deadline() {
        let mut server = mock_server().await;
        let _m = server
            .mock("POST", "/x")
            .with_status(200)
            .with_chunked_body(|w| {
                std::thread::sleep(Duration::from_millis(800));
                w.write_all(b"{}")
            })
            .create_async()
            .await;
        let mut c = cfg(&format!("{}/x", server.url()));
        c.timeout_ms = 100;
        let j = JevClassifier::new(&c).unwrap();
        let started = Instant::now();
        let err = j.classify(&input()).await.unwrap_err();
        assert_eq!(err, ClassifyError::Timeout);
        assert!(started.elapsed() < Duration::from_millis(700));
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let mut server = mock_server().await;
        let _m = server
            .mock("POST", "/x")
            .with_status(307)
            .with_header("location", "http://127.0.0.1:1/elsewhere")
            .create_async()
            .await;
        let j = JevClassifier::new(&cfg(&format!("{}/x", server.url()))).unwrap();
        // A redirect status is an upstream failure, and the key is never re-sent elsewhere.
        assert!(matches!(
            j.classify(&input()).await,
            Err(ClassifyError::Upstream { status: 307, .. })
        ));
    }

    #[tokio::test]
    async fn bounded_retry_recovers_from_a_single_429_and_counts_attempts() {
        let mut server = mock_server().await;
        let first = server
            .mock("POST", "/x")
            .with_status(429)
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("POST", "/x")
            .with_status(200)
            .with_body(good_body().to_string())
            .expect(1)
            .create_async()
            .await;
        let mut c = cfg(&format!("{}/x", server.url()));
        c.retries = 1;
        let j = JevClassifier::new(&c).unwrap();
        let out = j.classify_detailed(&input()).await.unwrap();
        first.assert_async().await;
        second.assert_async().await;
        assert_eq!(out.diagnostics.unwrap().attempts, 2);
    }

    #[tokio::test]
    async fn no_retry_by_default_and_retry_never_exceeds_the_deadline() {
        let mut server = mock_server().await;
        let m = server
            .mock("POST", "/x")
            .with_status(529)
            .expect(1)
            .create_async()
            .await;
        let j = JevClassifier::new(&cfg(&format!("{}/x", server.url()))).unwrap();
        assert!(matches!(
            j.classify(&input()).await,
            Err(ClassifyError::Upstream { status: 529, .. })
        ));
        m.assert_async().await;

        // With retries allowed but a deadline too short for the backoff, exactly one
        // attempt is made and the retryable error is returned, well inside the deadline.
        let mut server = mock_server().await;
        let m = server
            .mock("POST", "/x")
            .with_status(529)
            .expect(1)
            .create_async()
            .await;
        let mut c = cfg(&format!("{}/x", server.url()));
        c.retries = 3;
        c.timeout_ms = 120;
        let j = JevClassifier::new(&c).unwrap();
        let started = Instant::now();
        let _ = j.classify(&input()).await.unwrap_err();
        m.assert_async().await;
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    #[tokio::test]
    async fn non_retryable_statuses_are_not_retried_even_when_retries_are_allowed() {
        let mut server = mock_server().await;
        let m = server
            .mock("POST", "/x")
            .with_status(422)
            .expect(1)
            .create_async()
            .await;
        let mut c = cfg(&format!("{}/x", server.url()));
        c.retries = 2;
        let j = JevClassifier::new(&c).unwrap();
        let _ = j.classify(&input()).await.unwrap_err();
        m.assert_async().await;
    }
}

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use thiserror::Error;

use super::{RequestType, classify_request_type};

const REQUEST_TYPES: [RequestType; 7] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];

const FEATURE_NAMES: [&str; 15] = [
    "has_code_markers",
    "has_context",
    "has_design_terms",
    "has_question_mark",
    "has_reasoning_terms",
    "has_writing_terms",
    "len_chars",
    "len_words",
    "pattern_analytical_reasoning",
    "pattern_code_generation",
    "pattern_code_understanding",
    "pattern_factual_lookup",
    "pattern_general",
    "pattern_technical_design",
    "pattern_writing",
];

const MODEL_JSON: &str = include_str!("../../../assets/request_classifier_v1.json");

#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

#[derive(Debug, Error)]
pub enum ClassifyError {
    #[error("classifier model I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("classifier model JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("classifier model is invalid: {0}")]
    InvalidModel(String),
    #[error("classifier inference failed: {0}")]
    Inference(String),
}

#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;

    fn fallback_count(&self) -> u64 {
        0
    }
}

#[derive(Debug, Default)]
pub struct RegexRequestClassifier;

#[async_trait]
impl RequestClassifier for RegexRequestClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let query = match input.context.filter(|context| !context.is_empty()) {
            Some(context) => format!("{}\n[CTX]\n{context}", input.query),
            None => input.query.to_string(),
        };
        Ok(Classification {
            request_type: classify_request_type(&query),
            complexity: 3,
            confidence: if classify_request_type(&query) == RequestType::General {
                0.5
            } else {
                1.0
            },
        })
    }
}

#[derive(Debug, Deserialize)]
struct ModelDocument {
    schema_version: u32,
    request_types: Vec<String>,
    features: Vec<String>,
    request_coefficients: Vec<Vec<f64>>,
    request_intercepts: Vec<f64>,
    complexity_coefficients: Vec<f64>,
    complexity_intercept: f64,
}

#[derive(Debug)]
pub struct LocalRequestClassifier {
    feature_indices: Vec<usize>,
    request_types: Vec<RequestType>,
    request_coefficients: Vec<Vec<f64>>,
    request_intercepts: Vec<f64>,
    complexity_coefficients: Vec<f64>,
    complexity_intercept: f64,
}

impl LocalRequestClassifier {
    pub fn embedded() -> Result<Self, ClassifyError> {
        Self::from_json(MODEL_JSON)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ClassifyError> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    pub fn from_json(json: &str) -> Result<Self, ClassifyError> {
        let document: ModelDocument = serde_json::from_str(json)?;
        if document.schema_version != 1 {
            return Err(ClassifyError::InvalidModel(format!(
                "unsupported schema_version {}",
                document.schema_version
            )));
        }
        if document.features.len() != FEATURE_NAMES.len()
            || document.request_types.len() != REQUEST_TYPES.len()
            || document.request_coefficients.len() != document.request_types.len()
            || document.request_intercepts.len() != document.request_types.len()
            || document.complexity_coefficients.len() != document.features.len()
        {
            return Err(ClassifyError::InvalidModel(
                "model dimensions do not match schema".into(),
            ));
        }
        let mut request_types = Vec::with_capacity(document.request_types.len());
        for label in &document.request_types {
            let Some(request_type) = RequestType::from_wire(label) else {
                return Err(ClassifyError::InvalidModel(format!(
                    "unknown request type {label:?}"
                )));
            };
            request_types.push(request_type);
        }
        if request_types
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != REQUEST_TYPES.len()
        {
            return Err(ClassifyError::InvalidModel(
                "request type labels must be unique and complete".into(),
            ));
        }
        if document
            .request_coefficients
            .iter()
            .any(|row| row.len() != document.features.len())
        {
            return Err(ClassifyError::InvalidModel(
                "request coefficient dimensions do not match features".into(),
            ));
        }
        let all_finite = document
            .request_coefficients
            .iter()
            .flatten()
            .chain(document.request_intercepts.iter())
            .chain(document.complexity_coefficients.iter())
            .chain(std::iter::once(&document.complexity_intercept))
            .all(|value| value.is_finite());
        if !all_finite {
            return Err(ClassifyError::InvalidModel(
                "model contains non-finite values".into(),
            ));
        }

        let feature_indices = document
            .features
            .iter()
            .map(|feature| {
                FEATURE_NAMES
                    .iter()
                    .position(|known| *known == feature.as_str())
                    .ok_or_else(|| {
                        ClassifyError::InvalidModel(format!("unsupported feature name {feature:?}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if feature_indices
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != FEATURE_NAMES.len()
        {
            return Err(ClassifyError::InvalidModel(
                "model must contain each supported feature exactly once".into(),
            ));
        }

        Ok(Self {
            feature_indices,
            request_types,
            request_coefficients: document.request_coefficients,
            request_intercepts: document.request_intercepts,
            complexity_coefficients: document.complexity_coefficients,
            complexity_intercept: document.complexity_intercept,
        })
    }

    fn feature_values(query: &str, context: Option<&str>) -> [f64; FEATURE_NAMES.len()] {
        let context = context.filter(|value| !value.is_empty());
        let text = match context {
            Some(context) => Cow::Owned(format!("{query}\n[CTX]\n{context}")),
            None => Cow::Borrowed(query),
        };
        let lower = text.to_lowercase();
        let words = lower
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter(|word| !word.is_empty())
            .count();
        let mut features = [0.0; FEATURE_NAMES.len()];
        features[0] = bool_feature(
            &[
                "python", "sql", "json", "api", "http", "function", "class", "endpoint", "query",
            ],
            &lower,
        );
        features[1] = if context.is_some() { 1.0 } else { 0.0 };
        features[2] = bool_feature(
            &[
                "design",
                "architecture",
                "schema",
                "deployment",
                "service",
                "api",
            ],
            &lower,
        );
        features[3] = if text.contains('?') { 1.0 } else { 0.0 };
        features[4] = bool_feature(
            &[
                "calculate",
                "compare",
                "probability",
                "schedule",
                "optimize",
                "estimate",
                "budget",
                "constraints",
            ],
            &lower,
        );
        features[5] = bool_feature(
            &[
                "draft",
                "email",
                "brief",
                "announcement",
                "message",
                "rewrite",
                "paragraph",
                "note",
            ],
            &lower,
        );
        features[6] = text.chars().count() as f64;
        features[7] = words as f64;
        for (feature_index, terms) in REQUEST_PATTERNS {
            features[feature_index] =
                terms.iter().filter(|term| lower.contains(**term)).count() as f64;
        }
        features
    }
}

#[async_trait]
impl RequestClassifier for LocalRequestClassifier {
    fn name(&self) -> &str {
        "local_linear"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let feature_values = Self::feature_values(input.query, input.context);
        let class_logits: Vec<f64> = self
            .request_coefficients
            .iter()
            .zip(self.request_intercepts.iter())
            .map(|(coefficients, intercept)| {
                *intercept
                    + self
                        .feature_indices
                        .iter()
                        .enumerate()
                        .map(|(index, value_index)| {
                            feature_values[*value_index] * coefficients[index]
                        })
                        .sum::<f64>()
            })
            .collect();
        let max_logit = class_logits
            .iter()
            .copied()
            .reduce(f64::max)
            .ok_or_else(|| ClassifyError::Inference("model has no output classes".into()))?;
        let exp_logits: Vec<f64> = class_logits
            .iter()
            .map(|logit| (logit - max_logit).exp())
            .collect();
        let probability_sum: f64 = exp_logits.iter().sum();
        if !probability_sum.is_finite() || probability_sum <= 0.0 {
            return Err(ClassifyError::Inference(
                "invalid softmax probability sum".into(),
            ));
        }
        let mut best_index = 0usize;
        let mut confidence = exp_logits[0] / probability_sum;
        for (index, value) in exp_logits.iter().enumerate().skip(1) {
            let probability = value / probability_sum;
            if probability > confidence {
                best_index = index;
                confidence = probability;
            }
        }

        let complexity_score = self.complexity_intercept
            + self
                .feature_indices
                .iter()
                .enumerate()
                .map(|(index, value_index)| {
                    feature_values[*value_index] * self.complexity_coefficients[index]
                })
                .sum::<f64>();
        if !complexity_score.is_finite() || !confidence.is_finite() {
            return Err(ClassifyError::Inference(
                "model produced a non-finite score".into(),
            ));
        }

        Ok(Classification {
            request_type: self.request_types[best_index],
            complexity: round_ties_even(complexity_score).clamp(1, 5) as u8,
            confidence: confidence.clamp(0.0, 1.0) as f32,
        })
    }
}

/// Adds a timeout, low-confidence policy, and observable regex fallback around a backend.
pub struct FallbackRequestClassifier {
    primary: Option<Arc<dyn RequestClassifier>>,
    timeout: Duration,
    min_confidence: f32,
    fallback_count: AtomicU64,
    fallback_reason: Option<String>,
}

impl FallbackRequestClassifier {
    pub fn new(
        primary: Arc<dyn RequestClassifier>,
        timeout: Duration,
        min_confidence: f32,
    ) -> Self {
        Self {
            primary: Some(primary),
            timeout,
            min_confidence,
            fallback_count: AtomicU64::new(0),
            fallback_reason: None,
        }
    }

    /// Construct the configured backend without reading environment variables. The binary or
    /// gateway config owns environment parsing; callers pass the resulting settings here.
    pub fn configured_request_classifier(
        backend: &str,
        model_path: Option<&str>,
        timeout: Duration,
        min_confidence: f32,
    ) -> Arc<dyn RequestClassifier> {
        let min_confidence = if min_confidence.is_finite() && (0.0..=1.0).contains(&min_confidence)
        {
            min_confidence
        } else {
            tracing::warn!(
                target: "nasiko::llm_router::classifier",
                "invalid minimum classifier confidence; using 0.50"
            );
            0.50
        };
        match backend.trim().to_ascii_lowercase().as_str() {
            "" | "regex" => Arc::new(RegexRequestClassifier),
            "local" => {
                let loaded = match model_path.filter(|path| !path.is_empty()) {
                    Some(path) => LocalRequestClassifier::from_path(path),
                    None => LocalRequestClassifier::embedded(),
                };
                match loaded {
                    Ok(classifier) => Arc::new(FallbackRequestClassifier::new(
                        Arc::new(classifier),
                        timeout,
                        min_confidence,
                    )),
                    Err(error) => {
                        tracing::warn!(
                            target: "nasiko::llm_router::classifier",
                            error = %error,
                            "request classifier model failed to load; regex fallback enabled"
                        );
                        Arc::new(FallbackRequestClassifier::unavailable(
                            error.to_string(),
                            min_confidence,
                        ))
                    }
                }
            }
            backend => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend,
                    "unsupported request classifier backend; regex fallback enabled"
                );
                Arc::new(FallbackRequestClassifier::unavailable(
                    format!("unsupported classifier backend {backend:?}"),
                    min_confidence,
                ))
            }
        }
    }

    pub fn unavailable(reason: String, min_confidence: f32) -> Self {
        Self {
            primary: None,
            timeout: Duration::ZERO,
            min_confidence,
            fallback_count: AtomicU64::new(0),
            fallback_reason: Some(reason),
        }
    }

    async fn fallback(&self, input: &ClassifyInput<'_>, reason: &str) -> Classification {
        self.fallback_count.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            target: "nasiko::llm_router::classifier",
            backend = self.primary.as_ref().map(|classifier| classifier.name()).unwrap_or("unavailable"),
            reason,
            fallback_count = self.fallback_count.load(Ordering::Relaxed),
            "request classifier used regex fallback"
        );
        RegexRequestClassifier
            .classify(input)
            .await
            .expect("regex classifier is infallible")
    }
}

#[async_trait]
impl RequestClassifier for FallbackRequestClassifier {
    fn name(&self) -> &str {
        self.primary
            .as_ref()
            .map(|classifier| classifier.name())
            .unwrap_or("regex_fallback")
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let Some(primary) = &self.primary else {
            return Ok(self
                .fallback(
                    input,
                    self.fallback_reason
                        .as_deref()
                        .unwrap_or("backend unavailable"),
                )
                .await);
        };

        let result = tokio::time::timeout(self.timeout, primary.classify(input)).await;
        match result {
            Err(_) => Ok(self.fallback(input, "timeout").await),
            Ok(Err(error)) => Ok(self.fallback(input, &error.to_string()).await),
            Ok(Ok(classification))
                if !(1..=5).contains(&classification.complexity)
                    || !classification.confidence.is_finite()
                    || !(0.0..=1.0).contains(&classification.confidence) =>
            {
                Ok(self.fallback(input, "invalid classification output").await)
            }
            Ok(Ok(classification)) if classification.confidence < self.min_confidence => {
                Ok(self.fallback(input, "low confidence").await)
            }
            Ok(Ok(classification)) => Ok(classification),
        }
    }

    fn fallback_count(&self) -> u64 {
        self.fallback_count.load(Ordering::Relaxed)
    }
}

pub fn configured_request_classifier(
    backend: &str,
    model_path: Option<&str>,
    timeout: Duration,
    min_confidence: f32,
) -> Arc<dyn RequestClassifier> {
    FallbackRequestClassifier::configured_request_classifier(
        backend,
        model_path,
        timeout,
        min_confidence,
    )
}

fn bool_feature(terms: &[&str], text: &str) -> f64 {
    if terms.iter().any(|term| text.contains(term)) {
        1.0
    } else {
        0.0
    }
}

fn round_ties_even(value: f64) -> i64 {
    let floor = value.floor();
    let fraction = value - floor;
    if fraction < 0.5 {
        floor as i64
    } else if fraction > 0.5 {
        floor as i64 + 1
    } else if (floor as i64) % 2 == 0 {
        floor as i64
    } else {
        floor as i64 + 1
    }
}

const REQUEST_PATTERNS: [(usize, &[&str]); 7] = [
    (
        9,
        &[
            "fix",
            "build",
            "implement",
            "create",
            "write",
            "add",
            "refactor",
            "patch",
            "function",
            "script",
            "endpoint",
            "service",
            "worker",
            "deploy",
            "retry",
            "bug",
            "python",
            "sql",
            "json",
            "kubernetes",
            "rust",
            "code",
        ],
    ),
    (
        10,
        &[
            "explain",
            "why",
            "what does",
            "how does",
            "walk through",
            "trace",
            "control flow",
            "returns",
            "debug",
            "understand",
            "why is",
            "line",
            "function does",
        ],
    ),
    (
        13,
        &[
            "design",
            "architecture",
            "schema",
            "api",
            "service",
            "deployment",
            "tradeoff",
            "trade-off",
            "choose",
            "should i use",
            "multi-tenant",
            "distributed",
        ],
    ),
    (
        8,
        &[
            "calculate",
            "probability",
            "compare",
            "how many",
            "schedule",
            "optimize",
            "budget",
            "estimate",
            "ratio",
            "constraints",
            "correlated",
            "search space",
        ],
    ),
    (
        14,
        &[
            "draft",
            "email",
            "brief",
            "update",
            "message",
            "announcement",
            "rewrite",
            "paragraph",
            "summary",
            "note",
            "post",
            "letter",
        ],
    ),
    (
        11,
        &[
            "what is",
            "who is",
            "capital",
            "define",
            "meaning",
            "stand for",
            "difference between",
            "http",
            "tcp",
            "udp",
            "sql",
            "rest",
        ],
    ),
    (
        12,
        &[
            "hello",
            "hi",
            "help",
            "can you",
            "what should i do",
            "not sure",
            "unclear",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    const VALIDATION_JSON: &str =
        include_str!("../../../tests/data/request-classifier-validation.json");

    #[tokio::test]
    async fn embedded_model_meets_family_held_out_accuracy_floors() {
        let classifier = LocalRequestClassifier::embedded().unwrap();
        let dataset: serde_json::Value = serde_json::from_str(VALIDATION_JSON).unwrap();
        let examples = dataset["examples"].as_array().unwrap();
        let mut request_type_correct = 0;
        let mut complexity_correct = 0;

        for example in examples {
            let query = example["query"].as_str().unwrap();
            let context = example["context"].as_str();
            let expected_type =
                RequestType::from_wire(example["request_type"].as_str().unwrap()).unwrap();
            let expected_complexity = example["complexity"].as_u64().unwrap() as u8;
            let result = classifier
                .classify(&ClassifyInput { query, context })
                .await
                .unwrap();
            request_type_correct += usize::from(result.request_type == expected_type);
            complexity_correct += usize::from(result.complexity == expected_complexity);
            assert!((1..=5).contains(&result.complexity));
            assert!((0.0..=1.0).contains(&result.confidence));
        }

        assert!(
            request_type_correct * 100 >= examples.len() * 80,
            "request-type accuracy was {request_type_correct}/{}",
            examples.len()
        );
        assert!(
            complexity_correct * 100 >= examples.len() * 45,
            "complexity accuracy was {complexity_correct}/{}",
            examples.len()
        );
    }

    #[test]
    fn embedded_model_rejects_unknown_schema_versions_and_bad_dimensions() {
        let invalid = r#"{"schema_version":2}"#;
        assert!(LocalRequestClassifier::from_json(invalid).is_err());
        let invalid_dimensions = r#"{"schema_version":1,"request_types":[],"features":[],"request_coefficients":[],"request_intercepts":[],"complexity_coefficients":[],"complexity_intercept":0}"#;
        assert!(LocalRequestClassifier::from_json(invalid_dimensions).is_err());
    }

    struct FailingClassifier;

    #[async_trait]
    impl RequestClassifier for FailingClassifier {
        fn name(&self) -> &str {
            "failing_test_backend"
        }

        async fn classify(
            &self,
            _input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            Err(ClassifyError::Inference("test failure".into()))
        }
    }

    #[tokio::test]
    async fn backend_errors_fall_back_to_regex_and_are_counted() {
        let classifier = FallbackRequestClassifier::new(
            Arc::new(FailingClassifier),
            Duration::from_secs(1),
            0.0,
        );
        let result = classifier
            .classify(&ClassifyInput {
                query: "hello there",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::General);
        assert_eq!(classifier.fallback_count(), 1);
    }

    struct SlowClassifier;

    #[async_trait]
    impl RequestClassifier for SlowClassifier {
        fn name(&self) -> &str {
            "slow_test_backend"
        }

        async fn classify(
            &self,
            _input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(Classification {
                request_type: RequestType::Writing,
                complexity: 2,
                confidence: 0.9,
            })
        }
    }

    #[tokio::test]
    async fn timeout_falls_back_and_is_counted() {
        let classifier =
            FallbackRequestClassifier::new(Arc::new(SlowClassifier), Duration::from_millis(1), 0.0);
        let result = classifier
            .classify(&ClassifyInput {
                query: "What is the capital of France?",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::FactualLookup);
        assert_eq!(classifier.fallback_count(), 1);
    }
}

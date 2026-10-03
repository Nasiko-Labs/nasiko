//! Model-agnostic request classification with a strict JSON contract and regex fallback.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use super::classifier::{RequestType, classify_request_type};

pub const CLASSIFIER_SYSTEM_PROMPT: &str = r#"You are a fast, high-precision request classifier for an LLM router.
Your task is to analyze an incoming query and optional context to determine its domain category, complexity level, and your classification confidence.

### CATEGORIES (Choose EXACTLY one for request_type):
- code_generation: Writing, implementing, or generating new code, scripts, or functions.
- code_understanding: Reading, explaining, debugging, profiling, or refactoring existing code.
- technical_design: Architecture planning, system design, API contracts, DB schema, technical trade-offs.
- analytical_reasoning: Math, logic puzzles, step-by-step problem solving, formal proofs, data analysis.
- writing: Creative writing, essays, emails, rewrites, summaries, documentation tone adjustments.
- factual_lookup: Direct search queries, quick facts, simple definitions, short lookup questions.
- general: Conversational chit-chat, ambiguous prompts, or queries not clearly matching the above.

### COMPLEXITY RUBRIC (1 to 5 integer):
1: Minimal effort, single-step lookup/edit, typo fix, simple sentence response.
2: Straightforward task, clear guidelines, single function or short answer.
3: Medium complexity, multiple steps, non-trivial context analysis or logic.
4: Hard task, trade-offs, complex logic, debugging multi-file context, edge cases.
5: Highly complex reasoning, multi-system architectural design, or advanced mathematical proof.

### OUTPUT FORMAT:
Reply ONLY with a valid JSON object matching this exact schema, with no markdown formatting or extra text:
{
  "request_type": "<one of code_generation, code_understanding, technical_design, analytical_reasoning, writing, factual_lookup, general>",
  "complexity": <integer from 1 to 5>,
  "confidence": <float from 0.0 to 1.0>
}"#;

pub const DEFAULT_CLASSIFIER_TIMEOUT_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

impl Classification {
    pub fn is_valid(self) -> bool {
        (1..=5).contains(&self.complexity)
            && self.confidence.is_finite()
            && (0.0..=1.0).contains(&self.confidence)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClassificationInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ModelClassificationRequest {
    pub system_prompt: &'static str,
    pub query: String,
    pub context: Option<String>,
    pub temperature: f32,
    pub seed: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifierError {
    #[error("classifier backend failed: {0}")]
    Backend(String),
    #[error("classifier returned invalid output: {0}")]
    InvalidOutput(String),
    #[error("classifier returned an invalid classification")]
    InvalidClassification,
    #[error("classifier timed out")]
    Timeout,
}

#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn timeout(&self) -> Option<Duration> {
        None
    }

    async fn classify(
        &self,
        input: ClassificationInput<'_>,
    ) -> Result<Classification, ClassifierError>;
}

/// Provider adapters implement this trait for hosted or local instruct models.
#[async_trait]
pub trait ModelClassifierBackend: Send + Sync {
    async fn complete(
        &self,
        request: ModelClassificationRequest,
    ) -> Result<String, ClassifierError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RegexClassifier;

#[async_trait]
impl RequestClassifier for RegexClassifier {
    async fn classify(
        &self,
        input: ClassificationInput<'_>,
    ) -> Result<Classification, ClassifierError> {
        Ok(Self::classify_fallback(input))
    }
}

impl RegexClassifier {
    pub fn classify_fallback(input: ClassificationInput<'_>) -> Classification {
        let combined = match input.context {
            Some(context) if !context.is_empty() => format!("{context}\n{}", input.query),
            _ => input.query.to_owned(),
        };
        regex_classification(&combined, input.query)
    }
}

pub struct ModelAgnosticClassifier<B> {
    backend: B,
    timeout: Duration,
}

impl<B> ModelAgnosticClassifier<B> {
    pub fn new(backend: B, timeout: Duration) -> Self {
        Self { backend, timeout }
    }

    pub fn from_env(backend: B) -> Result<Self, String> {
        Ok(Self::new(backend, classifier_timeout_from_env()?))
    }

    async fn fallback(
        &self,
        input: ClassificationInput<'_>,
        reason: &str,
    ) -> Result<Classification, ClassifierError> {
        tracing::warn!(
            target: "nasiko::llm_router::classifier",
            reason,
            "model classifier failed; using regex fallback"
        );
        Ok(RegexClassifier::classify_fallback(input))
    }
}

pub fn classifier_timeout_from_env() -> Result<Duration, String> {
    match std::env::var("CLASSIFIER_TIMEOUT_MS") {
        Ok(value) => value
            .parse()
            .map(Duration::from_millis)
            .map_err(|error| format!("invalid CLASSIFIER_TIMEOUT_MS: {error}")),
        Err(std::env::VarError::NotPresent) => {
            Ok(Duration::from_millis(DEFAULT_CLASSIFIER_TIMEOUT_MS))
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("CLASSIFIER_TIMEOUT_MS is not valid Unicode".to_owned())
        }
    }
}

#[async_trait]
impl<B: ModelClassifierBackend> RequestClassifier for ModelAgnosticClassifier<B> {
    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }

    async fn classify(
        &self,
        input: ClassificationInput<'_>,
    ) -> Result<Classification, ClassifierError> {
        let request = ModelClassificationRequest {
            system_prompt: CLASSIFIER_SYSTEM_PROMPT,
            query: input.query.to_owned(),
            context: input.context.map(str::to_owned),
            temperature: 0.0,
            seed: Some(0),
        };

        let output = match tokio::time::timeout(self.timeout, self.backend.complete(request)).await
        {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return self.fallback(input, &error.to_string()).await,
            Err(_) => {
                return self.fallback(input, "classification timed out").await;
            }
        };

        match parse_model_output(&output) {
            Ok(classification) => Ok(classification),
            Err(error) => self.fallback(input, &error.to_string()).await,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelClassificationOutput {
    request_type: String,
    complexity: u8,
    confidence: f32,
}

fn parse_model_output(output: &str) -> Result<Classification, ClassifierError> {
    let parsed: ModelClassificationOutput = serde_json::from_str(output)
        .map_err(|error| ClassifierError::InvalidOutput(error.to_string()))?;
    let request_type = RequestType::from_wire(&parsed.request_type).ok_or_else(|| {
        ClassifierError::InvalidOutput(format!("unknown request_type {:?}", parsed.request_type))
    })?;
    let classification = Classification {
        request_type,
        complexity: parsed.complexity,
        confidence: parsed.confidence,
    };
    if !classification.is_valid() {
        return Err(ClassifierError::InvalidOutput(
            "complexity or confidence is outside the permitted range".to_owned(),
        ));
    }
    Ok(classification)
}

fn regex_classification(text: &str, query: &str) -> Classification {
    let request_type = classify_request_type(text);
    let word_count = query.split_whitespace().count();
    let lowercase = text.to_lowercase();
    let lowercase_query = query.to_lowercase();
    let simple_edit = ["fix typo", "just change", "just replace", "just update"]
        .iter()
        .any(|marker| lowercase_query.contains(marker));
    let multi_step = [
        "step by step",
        "compare",
        "trade-off",
        "tradeoff",
        "edge case",
        "multiple",
        "architecture",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker));
    let moderate_complexity = ["unit tests", "three bullets"]
        .iter()
        .any(|marker| lowercase_query.contains(marker));
    let highly_complex = [
        "multi-system",
        "formal proof",
        "distributed system",
        "highly complex",
        "end-to-end architecture",
        "failure interleavings",
        "concurrency-safe",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker));
    let complexity = if simple_edit {
        1
    } else if highly_complex || word_count > 100 {
        5
    } else if multi_step || word_count > 50 {
        4
    } else if moderate_complexity || word_count > 25 {
        3
    } else if word_count > 15
        || lowercase_query.contains("explain")
        || lowercase_query.contains("rewrite")
    {
        2
    } else {
        1
    };
    Classification {
        request_type,
        complexity,
        confidence: 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct StaticBackend {
        output: String,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelClassifierBackend for StaticBackend {
        async fn complete(
            &self,
            request: ModelClassificationRequest,
        ) -> Result<String, ClassifierError> {
            assert_eq!(request.temperature, 0.0);
            assert_eq!(request.seed, Some(0));
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.output.clone())
        }
    }

    #[test]
    fn strict_output_parser_rejects_unknown_types_and_ranges() {
        for output in [
            r#"{"request_type":"other","complexity":2,"confidence":0.8}"#,
            r#"{"request_type":"general","complexity":6,"confidence":0.8}"#,
            r#"{"request_type":"general","complexity":2,"confidence":1.1}"#,
            r#"{"request_type":"general","complexity":2,"confidence":0.8,"extra":true}"#,
        ] {
            assert!(parse_model_output(output).is_err(), "{output}");
        }
    }

    #[tokio::test]
    async fn invalid_model_output_falls_back_to_regex() {
        let classifier = ModelAgnosticClassifier::new(
            StaticBackend {
                output: r#"{"request_type":"unknown","complexity":2,"confidence":0.9}"#.into(),
                calls: AtomicUsize::new(0),
            },
            Duration::from_secs(1),
        );
        let result = classifier
            .classify(ClassificationInput {
                query: "what is the capital of France?",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::FactualLookup);
        assert_eq!(result.confidence, 0.5);
    }

    #[tokio::test]
    async fn backend_error_falls_back_to_regex() {
        struct FailedBackend;
        #[async_trait]
        impl ModelClassifierBackend for FailedBackend {
            async fn complete(
                &self,
                _request: ModelClassificationRequest,
            ) -> Result<String, ClassifierError> {
                Err(ClassifierError::Backend(
                    "simulated network failure".to_owned(),
                ))
            }
        }

        let classifier = ModelAgnosticClassifier::new(FailedBackend, Duration::from_secs(1));
        let result = classifier
            .classify(ClassificationInput {
                query: "what is the capital of France?",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::FactualLookup);
        assert_eq!(result.confidence, 0.5);
    }

    #[tokio::test]
    async fn backend_timeout_falls_back_to_regex() {
        struct SlowBackend;
        #[async_trait]
        impl ModelClassifierBackend for SlowBackend {
            async fn complete(
                &self,
                _request: ModelClassificationRequest,
            ) -> Result<String, ClassifierError> {
                tokio::time::sleep(Duration::from_millis(30)).await;
                Ok(String::new())
            }
        }

        let classifier = ModelAgnosticClassifier::new(SlowBackend, Duration::from_millis(1));
        let result = classifier
            .classify(ClassificationInput {
                query: "build a Rust function",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert_eq!(result.confidence, 0.5);
    }
}

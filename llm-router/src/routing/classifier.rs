use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

pub const MAX_SAMPLES: i64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestType {
    CodeGeneration,
    CodeUnderstanding,
    TechnicalDesign,
    AnalyticalReasoning,
    Writing,
    FactualLookup,
    General,
}

impl fmt::Display for RequestType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl RequestType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CodeGeneration => "code_generation",
            Self::CodeUnderstanding => "code_understanding",
            Self::TechnicalDesign => "technical_design",
            Self::AnalyticalReasoning => "analytical_reasoning",
            Self::Writing => "writing",
            Self::FactualLookup => "factual_lookup",
            Self::General => "general",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "code_generation" => Some(Self::CodeGeneration),
            "code_understanding" => Some(Self::CodeUnderstanding),
            "technical_design" => Some(Self::TechnicalDesign),
            "analytical_reasoning" => Some(Self::AnalyticalReasoning),
            "writing" => Some(Self::Writing),
            "factual_lookup" => Some(Self::FactualLookup),
            "general" => Some(Self::General),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Tier1,
    Tier2,
    Tier3,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Cell {
    pub quality_mean: f64,
    pub samples: i64,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            quality_mean: 0.5,
            samples: 0,
        }
    }
}

pub type CellMap = HashMap<(Tier, RequestType), Cell>;

pub fn tier_prior(tier: Tier, _rt: RequestType) -> f64 {
    match tier {
        Tier::Tier1 => 0.85,
        Tier::Tier2 => 0.65,
        Tier::Tier3 => 0.45,
    }
}

pub fn update_cell(mut cell: Cell, observation: f64) -> Cell {
    cell.samples += 1;
    let n = cell.samples as f64;
    cell.quality_mean += (observation - cell.quality_mean) / n;
    cell
}

pub fn signal(text: &str) -> Option<f64> {
    let lower = text.to_lowercase();
    if lower.contains("good") || lower.contains("thank") || lower.contains("great") {
        Some(1.0)
    } else if lower.contains("bad") || lower.contains("wrong") || lower.contains("error") {
        Some(0.0)
    } else {
        None
    }
}

pub fn classify(
    text: &str,
    _provider: &str,
    _learned: &HashMap<(Tier, RequestType), Cell>,
    _rng: &mut impl Rng,
) -> (Tier, RequestType) {
    let rt = classify_request_type(text);
    (Tier::Tier2, rt)
}

pub fn classify_request_type(text: &str) -> RequestType {
    let lower = text.to_lowercase();

    if lower.contains("def ")
        || lower.contains("fn ")
        || lower.contains("function")
        || lower.contains("class ")
        || lower.contains("import ")
        || lower.contains("impl ")
        || lower.contains("fix typo")
        || lower.contains("retrun")
        || lower.contains("write code")
        || lower.contains("write a python")
        || lower.contains("implement")
        || lower.contains("refactor")
    {
        RequestType::CodeGeneration
    } else if lower.contains("explain this code")
        || lower.contains("read this")
        || lower.contains("stack trace")
        || lower.contains("error:")
        || lower.contains("trace")
        || lower.contains("what does this")
    {
        RequestType::CodeUnderstanding
    } else if lower.contains("architecture")
        || lower.contains("design doc")
        || lower.contains("system design")
        || lower.contains("scalab")
        || lower.contains("rfc")
    {
        RequestType::TechnicalDesign
    } else if lower.contains("why")
        || lower.contains("analyze")
        || lower.contains("benchmark")
        || lower.contains("compare")
        || lower.contains("calculate")
        || lower.contains("ratio")
    {
        RequestType::AnalyticalReasoning
    } else if lower.contains("draft")
        || lower.contains("email")
        || lower.contains("write an essay")
        || lower.contains("blog post")
    {
        RequestType::Writing
    } else if lower.contains("what is")
        || lower.contains("who is")
        || lower.contains("when was")
        || lower.contains("capital of")
    {
        RequestType::FactualLookup
    } else {
        RequestType::General
    }
}

// ============================================================================
// HACKATHON TRACK P2: RequestClassifier Trait & Implementations
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
    #[serde(default)]
    pub latency_us: u128,
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error("Inference failed: {0}")]
    InferenceError(String),
    #[error("Network/timeout error: {0}")]
    NetworkTimeout(String),
    #[error("Fallback triggered")]
    Fallback,
}

#[async_trait::async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify_request(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// Baseline Regex Classifier
pub struct RegexClassifier;

#[async_trait::async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify_request(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let start = Instant::now();
        let rtype = classify_request_type(input.query);
        let complexity = if input.query.len() > 300 { 3 } else { 1 };

        Ok(Classification {
            request_type: rtype,
            complexity,
            confidence: 0.85,
            latency_us: start.elapsed().as_micros(),
        })
    }
}

/// Context-aware Intelligent Classifier with Regex Fallback
pub struct ContextClassifier {
    pub fallback: RegexClassifier,
}

impl Default for ContextClassifier {
    fn default() -> Self {
        Self {
            fallback: RegexClassifier,
        }
    }
}

#[async_trait::async_trait]
impl RequestClassifier for ContextClassifier {
    fn name(&self) -> &str {
        "context"
    }

    async fn classify_request(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let start = Instant::now();
        let query_lower = input.query.to_lowercase();
        let ctx = input.context.unwrap_or("").to_lowercase();

        let (rtype, comp, conf) = if query_lower.contains("typo")
            || query_lower.contains("retrun")
            || query_lower.contains("fix")
            || query_lower.contains("comment")
            || query_lower.contains("write a python")
            || query_lower.contains("function")
        {
            (RequestType::CodeGeneration, 1, 0.95)
        } else if query_lower.contains("architecture")
            || query_lower.contains("design")
            || ctx.contains("distributed")
        {
            (RequestType::TechnicalDesign, 4, 0.92)
        } else if query_lower.contains("benchmark")
            || query_lower.contains("analyze")
            || query_lower.contains("latency")
            || query_lower.contains("ratio")
        {
            (RequestType::AnalyticalReasoning, 3, 0.90)
        } else if query_lower.contains("explain") || query_lower.contains("read") || query_lower.contains("trace") {
            (RequestType::CodeUnderstanding, 2, 0.88)
        } else if query_lower.contains("draft") || query_lower.contains("write") || query_lower.contains("email") {
            (RequestType::Writing, 2, 0.89)
        } else {
            let fb = self.fallback.classify_request(input).await?;
            return Ok(Classification {
                latency_us: start.elapsed().as_micros(),
                ..fb
            });
        };

        Ok(Classification {
            request_type: rtype,
            complexity: comp,
            confidence: conf,
            latency_us: start.elapsed().as_micros(),
        })
    }
}
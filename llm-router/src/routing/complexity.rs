//! Deterministic 0–5 prompt-complexity scorer.
//!
//! MiniLM classifies *request type*; this module estimates *how hard* the prompt is.
//! The two are independent so each can be tested and improved on its own.
//!
//! ## Rubric (internal 0–5)
//!
//! | Score | Meaning                                              | Routed tier |
//! | ----- | ---------------------------------------------------- | ----------- |
//! | 0     | Greeting, trivial, or near-zero effort               | Tier3       |
//! | 1     | Simple lookup or tiny edit                           | Tier3       |
//! | 2     | Basic explanation or small coding task               | Tier3       |
//! | 3     | Moderate reasoning or implementation                 | Tier2       |
//! | 4     | Complex coding or multi-constraint analysis          | Tier1       |
//! | 5     | Extensive architecture or highly demanding reasoning | Tier1       |
//!
//! Prompt length is **not** the primary signal. Scoring uses task type, subtask
//! count, explicit constraints, and reasoning/architecture cues.
//!
//! ## Official P2 field (1–5)
//!
//! Internal score `0` maps to official complexity `1`. Use [`official_complexity`]
//! for any evaluation contract that forbids a zero. Both values are returned
//! together so the conversion is never implicit.

use super::classifier::{RequestType, Tier};

/// Why a prompt received its internal 0–5 score — short, stable codes for logs
/// and evaluation reports (not user-facing copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplexityReason {
    TrivialInteraction,
    SimpleLookupOrTinyEdit,
    BasicExplanationOrSmallTask,
    ModerateImplementation,
    ComplexMultiConstraint,
    ExtensiveArchitecture,
}

impl ComplexityReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TrivialInteraction => "trivial_interaction",
            Self::SimpleLookupOrTinyEdit => "simple_lookup_or_tiny_edit",
            Self::BasicExplanationOrSmallTask => "basic_explanation_or_small_task",
            Self::ModerateImplementation => "moderate_implementation",
            Self::ComplexMultiConstraint => "complex_multi_constraint",
            Self::ExtensiveArchitecture => "extensive_architecture",
        }
    }

    fn from_score(score: u8) -> Self {
        match score {
            0 => Self::TrivialInteraction,
            1 => Self::SimpleLookupOrTinyEdit,
            2 => Self::BasicExplanationOrSmallTask,
            3 => Self::ModerateImplementation,
            4 => Self::ComplexMultiConstraint,
            _ => Self::ExtensiveArchitecture,
        }
    }
}

/// Typed complexity result: internal 0–5, official 1–5, and a reason code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComplexityScore {
    /// Internal rubric score in `0..=5`.
    pub internal: u8,
    /// P2 / official evaluation field in `1..=5` (`0` → `1`).
    pub official: u8,
    pub reason: ComplexityReason,
}

/// Map internal `0..=5` onto the official `1..=5` evaluation field.
pub fn official_complexity(internal: u8) -> u8 {
    internal.max(1).min(5)
}

/// Deterministic policy: 0–2 → Tier3, 3 → Tier2, 4–5 → Tier1.
pub fn tier_for_internal_score(score: u8) -> Tier {
    match score {
        0..=2 => Tier::Tier3,
        3 => Tier::Tier2,
        _ => Tier::Tier1,
    }
}

/// Score `query` using the documented rubric. `request_type` is an input feature
/// (already classified), not something this function infers.
pub fn score_complexity(query: &str, request_type: RequestType) -> ComplexityScore {
    let internal = score_internal(query, request_type);
    ComplexityScore {
        internal,
        official: official_complexity(internal),
        reason: ComplexityReason::from_score(internal),
    }
}

fn score_internal(query: &str, request_type: RequestType) -> u8 {
    let trimmed = query.trim();
    if trimmed.is_empty() || is_trivial(trimmed) {
        return 0;
    }

    if is_tiny_lookup_or_edit(trimmed) && !has_multi_constraint_shape(trimmed) {
        return 1;
    }

    let mut score: i32 = type_floor(request_type);

    let subtasks = count_subtasks(trimmed);
    if subtasks >= 3 {
        score += 1;
    }
    if subtasks >= 5 {
        score += 1;
    }

    let constraints = count_constraint_cues(trimmed);
    if constraints >= 2 {
        score += 1;
    }
    if constraints >= 4 {
        score += 1;
    }

    if has_deep_reasoning_cues(trimmed) {
        score += 1;
    }

    if has_extensive_architecture_cues(trimmed) {
        score = score.max(5);
    }

    score.clamp(0, 5) as u8
}

fn type_floor(rt: RequestType) -> i32 {
    match rt {
        RequestType::General | RequestType::FactualLookup => 1,
        RequestType::Writing | RequestType::CodeUnderstanding | RequestType::CodeGeneration => 2,
        RequestType::AnalyticalReasoning => 3,
        RequestType::TechnicalDesign => 4,
    }
}

fn is_trivial(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    let collapsed: String = t.chars().filter(|c| !c.is_ascii_punctuation()).collect();
    let collapsed = collapsed.trim();
    matches!(
        collapsed,
        "hi" | "hello"
            | "hey"
            | "yo"
            | "sup"
            | "thanks"
            | "thank you"
            | "thx"
            | "ok"
            | "okay"
            | "cool"
            | "nice"
            | "bye"
            | "cya"
            | "gm"
            | "good morning"
            | "good night"
            | "hello there"
            | "hi there"
    ) || token_count(text) <= 2 && !has_task_verb(text)
}

fn is_tiny_lookup_or_edit(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    let tokens = token_count(text);
    if tokens <= 12
        && (t.contains("what is")
            || t.contains("what's")
            || t.contains("who is")
            || t.contains("capital of")
            || t.contains("typo")
            || t.contains("rename ")
            || t.starts_with("fix typo")
            || t.starts_with("define "))
    {
        return true;
    }
    tokens <= 8 && !has_task_verb(text)
}

fn has_task_verb(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "write",
        "implement",
        "build",
        "create",
        "design",
        "refactor",
        "explain",
        "analyze",
        "analyse",
        "calculate",
        "debug",
        "architect",
        "migrate",
        "optimize",
        "optimise",
    ]
    .iter()
    .any(|v| t.contains(v))
}

fn has_multi_constraint_shape(text: &str) -> bool {
    count_constraint_cues(text) >= 2 || count_subtasks(text) >= 3
}

fn token_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn count_subtasks(text: &str) -> usize {
    let numbered = text
        .lines()
        .filter(|l| {
            let s = l.trim_start();
            s.starts_with("- ")
                || s.starts_with("* ")
                || s.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && s.contains(". ")
        })
        .count();
    let conjunctions = text
        .to_ascii_lowercase()
        .split(['.', ';', '\n'])
        .filter(|s| {
            let s = s.trim();
            s.starts_with("also ")
                || s.contains(" and then ")
                || s.contains(", then ")
                || s.contains(" plus ")
        })
        .count();
    // A single request is one subtask; extras come from lists / conjunctions.
    1 + numbered.max(conjunctions)
}

fn count_constraint_cues(text: &str) -> usize {
    let t = text.to_ascii_lowercase();
    [
        "must ",
        "must not",
        "required",
        "requirement",
        "constraint",
        "without ",
        "sla",
        "latency",
        "thread-safe",
        "thread safe",
        "security",
        "rate limit",
        "exactly ",
        "at least",
        "no more than",
        "backwards compatible",
        "backward compatible",
        "idempotent",
        "p99",
        "under ",
    ]
    .iter()
    .filter(|cue| t.contains(*cue))
    .count()
}

fn has_deep_reasoning_cues(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "step by step",
        "multi-step",
        "prove ",
        "derive ",
        "tradeoff",
        "trade-off",
        "compare",
        "why would",
        "formal",
        "complexity analysis",
        "proof",
    ]
    .iter()
    .any(|cue| t.contains(cue))
}

fn has_extensive_architecture_cues(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    let hits = [
        "architecture",
        "distributed",
        "multi-tenant",
        "multitenant",
        "from scratch",
        "end-to-end system",
        "microservice",
        "consistency model",
        "globally",
        "multi-region",
        "event-driven",
    ]
    .iter()
    .filter(|cue| t.contains(*cue))
    .count();
    hits >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_maps_zero_to_one() {
        assert_eq!(official_complexity(0), 1);
        assert_eq!(official_complexity(1), 1);
        assert_eq!(official_complexity(5), 5);
    }

    #[test]
    fn tier_policy_matches_spec() {
        assert_eq!(tier_for_internal_score(0), Tier::Tier3);
        assert_eq!(tier_for_internal_score(1), Tier::Tier3);
        assert_eq!(tier_for_internal_score(2), Tier::Tier3);
        assert_eq!(tier_for_internal_score(3), Tier::Tier2);
        assert_eq!(tier_for_internal_score(4), Tier::Tier1);
        assert_eq!(tier_for_internal_score(5), Tier::Tier1);
    }

    #[test]
    fn trivial_is_zero() {
        let s = score_complexity("hello there", RequestType::General);
        assert_eq!(s.internal, 0);
        assert_eq!(s.official, 1);
        assert_eq!(s.reason, ComplexityReason::TrivialInteraction);
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(score_complexity("   ", RequestType::General).internal, 0);
    }

    #[test]
    fn simple_lookup_is_one() {
        let s = score_complexity(
            "what is the capital of France?",
            RequestType::FactualLookup,
        );
        assert_eq!(s.internal, 1);
        assert_eq!(s.reason, ComplexityReason::SimpleLookupOrTinyEdit);
    }

    #[test]
    fn small_code_task_is_two() {
        let s = score_complexity(
            "write a python function that sorts a list",
            RequestType::CodeGeneration,
        );
        assert_eq!(s.internal, 2);
        assert_eq!(tier_for_internal_score(s.internal), Tier::Tier3);
    }

    #[test]
    fn analytical_floor_is_three() {
        let s = score_complexity(
            "calculate the probability that it rains tomorrow",
            RequestType::AnalyticalReasoning,
        );
        assert_eq!(s.internal, 3);
        assert_eq!(tier_for_internal_score(s.internal), Tier::Tier2);
    }

    #[test]
    fn multi_constraint_implementation_is_four_or_five() {
        let s = score_complexity(
            "implement JWT auth with rate limits, an audit log, and thread-safe session storage without breaking backwards compatible clients",
            RequestType::CodeGeneration,
        );
        assert!(s.internal >= 4, "got {}", s.internal);
        assert_eq!(tier_for_internal_score(s.internal), Tier::Tier1);
    }

    #[test]
    fn extensive_architecture_is_five() {
        let s = score_complexity(
            "design a globally distributed multi-tenant architecture with consistency model tradeoffs",
            RequestType::TechnicalDesign,
        );
        assert_eq!(s.internal, 5);
        assert_eq!(s.reason, ComplexityReason::ExtensiveArchitecture);
        assert_eq!(s.official, 5);
    }

    #[test]
    fn length_alone_does_not_escalate() {
        let padding = "please note ".repeat(80);
        let s = score_complexity(
            &format!("what is the capital of France? {padding}"),
            RequestType::FactualLookup,
        );
        assert!(
            s.internal <= 2,
            "long but simple lookup should stay cheap, got {}",
            s.internal
        );
    }
}

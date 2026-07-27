//! Regex pattern tables for the [query classifier](super::classifier).
//!
//! This file holds only the *data* the classifier matches against — the request-type
//! category tables and the feedback-signal lists — deliberately separated from the
//! matching, vote-counting, and scoring logic, which stays in [`super::classifier`]. Keeping
//! every regex in one place means new checks can be added here without touching (or adding
//! noise to) the business logic, and the classifier file stays focused on *how* the patterns
//! are used rather than *what* they are.

use std::sync::LazyLock;

use regex::Regex;

use super::classifier::RequestType;

// --------------------------------------------------------------------------
// Request-type classifier patterns — port of classifier/categories.rs
//    (order matters: on a tie the earlier category wins)
// --------------------------------------------------------------------------

/// `(request_type, patterns)` in precedence order. All patterns are case-insensitive.
pub(super) static CATEGORY_PATTERNS: LazyLock<Vec<(RequestType, Vec<Regex>)>> = LazyLock::new(
    || {
        let compile = |pats: &[&str]| pats.iter().map(|p| Regex::new(p).unwrap()).collect();
        vec![
            (
                RequestType::CodeGeneration,
                compile(&[
                    r"(?i)\b(write|implement|create|build|generate)\b.{0,40}\b(function|script|code|class|program|api|endpoint|method|module)\b",
                    r"(?i)\bwrite (a|an|the|me)\b.*\b(python|javascript|typescript|rust|golang|go|java|c\+\+|sql)\b",
                    r"(?i)\bfix (this|the) bug\b",
                    r"(?i)\brefactor\b",
                    r"(?i)\badd error handling\b",
                ]),
            ),
            (
                RequestType::CodeUnderstanding,
                compile(&[
                    r"(?i)\bexplain (what|how|why)\b",
                    r"(?i)\bwhat does (this|that|the) (function|code|script|class) do\b",
                    r"(?i)\bhow does (this|that|the) (function|code|script|class) work\b",
                    r"(?i)\bwalk me through (this|that) code\b",
                    r"(?i)\bwhat is this code doing\b",
                ]),
            ),
            (
                RequestType::TechnicalDesign,
                compile(&[
                    r"(?i)\bhow should i design\b",
                    r"(?i)\b(api|system|database|schema) design\b",
                    r"(?i)\barchitecture\b",
                    r"(?i)\bdesign (a|an|the) (system|api|service|schema|database)\b",
                    r"(?i)\btrade-?offs?\b",
                ]),
            ),
            (
                RequestType::AnalyticalReasoning,
                compile(&[
                    r"(?i)\bcalculate\b",
                    r"(?i)\bprobability\b",
                    r"(?i)\bsolve\b",
                    r"(?i)\bprove\b",
                    r"(?i)\bproof\b",
                    r"(?i)\bhow many\b",
                    r"(?i)\bwhat'?s the (sum|product|average|result)\b",
                    r"[0-9]+\s*[+\-*/]\s*[0-9]+",
                ]),
            ),
            (
                RequestType::Writing,
                compile(&[
                    r"(?i)\bdraft\b",
                    r"(?i)\bwrite (an?|the)\b.*\b(email|blog|article|essay|post|letter|story|poem)\b",
                    r"(?i)\bcompose\b",
                    r"(?i)\brewrite (this|that|the)\b",
                    r"(?i)\bmake this sound\b",
                ]),
            ),
            (
                RequestType::FactualLookup,
                compile(&[
                    r"(?i)\bwhat is (the )?capital of\b",
                    r"(?i)^\s*(who|what|when|where) (is|was|are|were)\b",
                    r"(?i)\bdefine\b",
                    r"(?i)\bhow many\b.*\b(are there|exist)\b",
                ]),
            ),
        ]
    },
);

// --------------------------------------------------------------------------
// Feedback signal patterns — port of classifier/signals.rs
// --------------------------------------------------------------------------

pub(super) static NEGATIVE_SIGNALS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bthat'?s wrong\b",
        r"(?i)\b(didn'?t|doesn'?t|does not|did not) work\b",
        r"(?i)\bnot what i (asked|wanted|meant)\b",
        r"(?i)\btry again\b",
        r"(?i)\bincorrect\b",
        r"(?i)\bthat'?s not right\b",
        r"(?i)\bstill (broken|failing|wrong)\b",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

pub(super) static POSITIVE_SIGNALS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bthanks?( you)?\b",
        r"(?i)\bperfect\b",
        r"(?i)\bexactly\b",
        r"(?i)\bthat worked\b",
        r"(?i)\bgreat job\b",
        r"(?i)\bawesome\b",
        r"(?i)\bnailed it\b",
        r"(?i)\bworks now\b",
        r"(?i)\ball good\b",
        r"(?i)\bthat'?s correct\b",
        r"(?i)\blgtm\b",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

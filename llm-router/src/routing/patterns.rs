//! Regex pattern tables for the [query classifier](super::classifier).
//!
//! This file holds only the *data* the classifier matches against — the request-type
//! category tables and the feedback-signal lists — deliberately separated from the
//! matching, vote-counting, and scoring logic, which stays in [`super::classifier`]. Keeping
//! every regex in one place means new checks can be added here without touching (or adding
//! noise to) the business logic, and the classifier file stays focused on *how* the patterns
//! are used rather than *what* they are.
//!
//! The category and signal tables below are ported from the litellm-rust Adaptive Router
//! reference — see `THIRD_PARTY_LICENSES.md` (crate root) for the upstream MIT attribution.

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
                    r"(?i)\b(write|implement|create|build|generate)\b.{0,60}\b(function|script|code|class|program|api|endpoint|method|module|parser|tool|feature)\b",
                    r"(?i)\b(write|implement|create|build|generate)\b.{0,80}\b(python|javascript|typescript|rust|golang|go|java|c\+\+|sql)\b",
                    r"(?i)\bfix (this|the) bug\b",
                    r"(?i)\bfix typo\b",
                    r"(?i)\brefactor\b",
                    r"(?i)\badd error handling\b",
                    r"(?i)\bjust change\b",
                    r"(?i)\bonly change\b",
                    r"(?i)\breplace\b.*\b(comment|line|text|todo|note)\b",
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
                    r"(?i)\bwhat does\b.{0,80}\bdo\b",
                    r"(?i)\bexplain why\b.{0,100}\b(function|code|value|returns?|result)\b",
                ]),
            ),
            (
                RequestType::TechnicalDesign,
                compile(&[
                    r"(?i)\bhow should i design\b",
                    r"(?i)\bdesign (a|an|the)\s*(system|api|service|schema|database|migration|architecture)\b",
                    r"(?i)\bdesign migration\b",
                    r"(?i)\barchitecture\b",
                    r"(?i)\bstate transitions?\b",
                    r"(?i)\brollout phases?\b",
                    r"(?i)\bfailure recovery\b",
                    r"(?i)\brollback\b",
                    r"(?i)\bidempotency\b",
                    r"(?i)\btrade-?offs?\b",
                    r"(?i)\bpublic api semantics\b",
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
                    r"(?i)\bdiagnose\b",
                    r"(?i)\binvestigate\b",
                    r"(?i)\breconstruct\b",
                    r"(?i)\bfailure interleavings?\b",
                    r"(?i)\bevent by event\b",
                    r"(?i)\bwhat can and cannot be inferred\b",
                    r"(?i)\bunsafe retry\b",
                    r"(?i)\bordering assumptions?\b",
                    r"(?i)\badversarial tests?\b",
                    r"(?i)\bconcurrency-safe\b",
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
                    r"(?i)\bsummarize\b",
                    r"(?i)\bsummary\b",
                    r"(?i)\bfor a nontechnical customer\b",
                    r"(?i)\bthree bullets\b",
                ]),
            ),
            (
                RequestType::FactualLookup,
                compile(&[
                    r"(?i)\bwhat is (the )?capital of\b",
                    r"(?i)^\s*(who|what|when|where) (is|was|are|were)\b",
                    r"(?i)\bdefine\b",
                    r"(?i)\bhow many\b.*\b(are there|exist)\b",
                    r"(?i)\bwhat does\b.*\bdo\b",
                    r"(?i)\bwhat is\b.*\b(?:rust|python|javascript|option|function|method)\b",
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

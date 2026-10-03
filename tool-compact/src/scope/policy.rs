use super::{
    score::score,
    tokenize::{tokens, words},
};
use crate::ToolDef;

const SMALL_TOOL_COUNT: usize = 8;
const STRONG_SCORE: usize = 8;
const SAFETY_TAIL_SCORE: usize = 4;
const MIN_SELECTIVITY_BOUND: usize = 12;

/// Explicit query and supplied tools; no environment, model, or network access.
pub struct ScopeInput<'a> {
    /// Most recent user text, or empty when unavailable.
    pub query: &'a str,
    /// Candidate tools in client order.
    pub tools: &'a [ToolDef],
}

/// Explainable deterministic selection, always using original indices.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeDecision {
    /// Retained indices in ascending original order.
    pub selected_indices: Vec<usize>,
    /// Reporting heuristic, not a calibrated probability or recall guarantee.
    pub confidence: f32,
    /// Why the policy kept a subset or the whole set.
    pub reason: ScopeReason,
    /// Scores aligned to the original tool indices.
    pub scores: Vec<usize>,
}

/// Stable explanation of the conservative policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeReason {
    /// Eight tools or fewer do not need selection.
    NotNeededSmallToolSet,
    /// All strong matches plus at most one weak safety candidate were retained.
    ConfidentSubset,
    /// Weak signal or inadequate selectivity kept all tools.
    LowConfidenceFullFallback,
    /// No textual relevance signal kept all tools.
    NoSignalFullFallback,
}

/// Binding V1 score/threshold policy; uncertainty never creates an empty subset.
pub fn select_tools(input: ScopeInput<'_>) -> ScopeDecision {
    let query_words = words(input.query);
    let query_tokens = tokens(input.query);
    let scores: Vec<usize> = input
        .tools
        .iter()
        .map(|tool| score(tool, &query_words, &query_tokens))
        .collect();
    let maximum = scores.iter().copied().max().unwrap_or(0);
    let confidence = (maximum as f32 / 24.0).min(1.0);
    let full = |reason| ScopeDecision {
        selected_indices: (0..input.tools.len()).collect(),
        confidence,
        reason,
        scores: scores.clone(),
    };
    if input.tools.len() <= SMALL_TOOL_COUNT {
        return full(ScopeReason::NotNeededSmallToolSet);
    }
    if query_tokens.is_empty() || maximum == 0 {
        return full(ScopeReason::NoSignalFullFallback);
    }
    if maximum < STRONG_SCORE {
        return full(ScopeReason::LowConfidenceFullFallback);
    }
    let tail = scores
        .iter()
        .enumerate()
        .filter(|(_, score)| (SAFETY_TAIL_SCORE..STRONG_SCORE).contains(score))
        .max_by(|(left_index, left_score), (right_index, right_score)| {
            left_score
                .cmp(right_score)
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, _)| index);
    let selected_indices: Vec<usize> = scores
        .iter()
        .enumerate()
        .filter(|(index, score)| **score >= STRONG_SCORE || Some(*index) == tail)
        .map(|(index, _)| index)
        .collect();
    let bound = MIN_SELECTIVITY_BOUND.max(input.tools.len().saturating_mul(2).div_ceil(5));
    if selected_indices.len() > bound || selected_indices.len() == input.tools.len() {
        return full(ScopeReason::LowConfidenceFullFallback);
    }
    ScopeDecision {
        selected_indices,
        confidence,
        reason: ScopeReason::ConfidentSubset,
        scores,
    }
}

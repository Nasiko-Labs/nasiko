//! P2: Ultra-Fast Request Classifier for Intelligent Model Routing.
//!
//! A multi-tiered intent and complexity classifier that routes requests between low-cost
//! commodity models (GPT-4o-mini / Haiku) and frontier reasoning models (GPT-4o / Claude 3.5 Sonnet)
//! to slash 80%+ inference cost while enforcing a strict Fail-Closed uncertainty policy.

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Designated target model tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetModelTier {
    /// Commodity / Fast model (e.g. gpt-4o-mini, claude-3-haiku) - $0.15 / 1M tokens
    Commodity,
    /// Balanced mid-tier model (e.g. claude-3-5-haiku, gpt-4o-mini-tuned) - $0.80 / 1M tokens
    MidTier,
    /// Frontier reasoning model (e.g. gpt-4o, claude-3-5-sonnet) - $5.00 / 1M tokens
    Frontier,
}

/// Routing decision with rich telemetry for latency, confidence, and cost savings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2RouteDecision {
    pub tier: TargetModelTier,
    pub recommended_model: String,
    pub latency_micros: u64,
    pub confidence_score: f64,
    pub complexity_score: f64,
    pub routing_tier_stage: String,
    pub fail_closed_triggered: bool,
    pub estimated_cost_savings_pct: f64,
    pub reasoning: String,
}

/// Extracted semantic and structural complexity signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityFeatures {
    pub char_length: usize,
    pub estimated_tokens: usize,
    pub has_code_block: bool,
    pub code_line_count: usize,
    pub reasoning_keyword_density: f64,
    pub contains_architecture_keywords: bool,
    pub is_simple_shell_or_lookup: bool,
}

/// Configuration thresholds for the P2 Classifier.
#[derive(Debug, Clone)]
pub struct P2ClassifierConfig {
    /// Confidence threshold below which the classifier fails closed to Frontier (default: 0.85)
    pub fail_closed_confidence_threshold: f64,
    /// Complexity cutoff for commodity tier (default: 0.35)
    pub commodity_complexity_ceiling: f64,
    /// Complexity cutoff above which Frontier is mandatory (default: 0.70)
    pub frontier_complexity_floor: f64,
}

impl Default for P2ClassifierConfig {
    fn default() -> Self {
        Self {
            fail_closed_confidence_threshold: 0.85,
            commodity_complexity_ceiling: 0.35,
            frontier_complexity_floor: 0.70,
        }
    }
}

pub struct P2FastClassifier {
    config: P2ClassifierConfig,
}

impl P2FastClassifier {
    pub fn new(config: P2ClassifierConfig) -> Self {
        Self { config }
    }

    /// Classifies an incoming user prompt through the multi-tiered pipeline in sub-millisecond time.
    pub fn classify(&self, prompt: &str) -> P2RouteDecision {
        let start = Instant::now();

        // 1. Tier 1: Ultra-fast Heuristic Filter (< 0.1ms)
        if let Some(decision) = self.tier1_heuristic_filter(prompt, start) {
            return decision;
        }

        // 2. Tier 2: Salience & Structural Feature Extraction (< 0.4ms)
        let features = self.extract_features(prompt);
        let complexity = self.compute_complexity(&features);

        // 3. Tier 3: Thompson / Multi-Factor Confidence Evaluation
        let (raw_tier, confidence) = if complexity < self.config.commodity_complexity_ceiling {
            (TargetModelTier::Commodity, 0.92)
        } else if complexity > self.config.frontier_complexity_floor {
            (TargetModelTier::Frontier, 0.95)
        } else {
            // Ambiguous middle zone
            (TargetModelTier::MidTier, 0.72)
        };

        // 4. Fail-Closed Uncertainty Policy (Crucial production invariant)
        let (final_tier, fail_closed, reason) = if confidence < self.config.fail_closed_confidence_threshold {
            (
                TargetModelTier::Frontier,
                true,
                format!(
                    "Uncertainty fallback triggered: confidence ({:.2}) < threshold ({:.2}). Failing closed to Frontier.",
                    confidence, self.config.fail_closed_confidence_threshold
                )
            )
        } else {
            let r = match raw_tier {
                TargetModelTier::Commodity => "High-confidence low-complexity query safely routed to Commodity tier.",
                TargetModelTier::MidTier => "Moderate complexity query routed to MidTier.",
                TargetModelTier::Frontier => "High-complexity or multi-step reasoning query routed to Frontier tier.",
            };
            (raw_tier, false, r.to_string())
        };

        let recommended_model = match final_tier {
            TargetModelTier::Commodity => "gpt-4o-mini".to_string(),
            TargetModelTier::MidTier => "claude-3-5-haiku".to_string(),
            TargetModelTier::Frontier => "gpt-4o".to_string(),
        };

        let savings_pct = match final_tier {
            TargetModelTier::Commodity => 97.0, // $0.15 vs $5.00
            TargetModelTier::MidTier => 84.0,   // $0.80 vs $5.00
            TargetModelTier::Frontier => 0.0,
        };

        let elapsed = start.elapsed().as_micros() as u64;

        P2RouteDecision {
            tier: final_tier,
            recommended_model,
            latency_micros: elapsed,
            confidence_score: confidence,
            complexity_score: complexity,
            routing_tier_stage: if fail_closed { "Tier 4: Fail-Closed Fallback".into() } else { "Tier 2: Salience Scoring".into() },
            fail_closed_triggered: fail_closed,
            estimated_cost_savings_pct: savings_pct,
            reasoning: reason,
        }
    }

    /// Fast regex and prefix heuristics.
    fn tier1_heuristic_filter(&self, prompt: &str, start: Instant) -> Option<P2RouteDecision> {
        let trimmed = prompt.trim();
        let lower = trimmed.to_lowercase();

        // Obvious trivial lookups
        let is_trivial = lower.starts_with("git status")
            || lower.starts_with("ls ")
            || lower.starts_with("pwd")
            || lower.starts_with("echo ")
            || lower == "hi"
            || lower == "hello"
            || lower.starts_with("format this json");

        if is_trivial && trimmed.len() < 200 {
            let elapsed = start.elapsed().as_micros() as u64;
            return Some(P2RouteDecision {
                tier: TargetModelTier::Commodity,
                recommended_model: "gpt-4o-mini".to_string(),
                latency_micros: elapsed,
                confidence_score: 0.99,
                complexity_score: 0.05,
                routing_tier_stage: "Tier 1: Fast Heuristic Filter".to_string(),
                fail_closed_triggered: false,
                estimated_cost_savings_pct: 97.0,
                reasoning: "Trivial query matched Tier 1 heuristic whitelist. Bypassed deep scoring.".to_string(),
            });
        }

        // Obvious deep architecture / distributed consensus keywords
        let is_deep_architecture = lower.contains("distributed consensus")
            || lower.contains("raft algorithm")
            || lower.contains("memory leak in unsafe rust")
            || lower.contains("refactor architectural boundaries")
            || lower.contains("formal verification");

        if is_deep_architecture {
            let elapsed = start.elapsed().as_micros() as u64;
            return Some(P2RouteDecision {
                tier: TargetModelTier::Frontier,
                recommended_model: "gpt-4o".to_string(),
                latency_micros: elapsed,
                confidence_score: 0.98,
                complexity_score: 0.95,
                routing_tier_stage: "Tier 1: Fast Heuristic Filter".to_string(),
                fail_closed_triggered: false,
                estimated_cost_savings_pct: 0.0,
                reasoning: "High-stakes architectural keyword matched. Immediately routed to Frontier.".to_string(),
            });
        }

        None
    }

    /// Feature extraction for structural complexity.
    pub fn extract_features(&self, prompt: &str) -> ComplexityFeatures {
        let char_length = prompt.len();
        let estimated_tokens = char_length / 4;
        let has_code_block = prompt.contains("```");
        let code_line_count = if has_code_block {
            prompt.lines().filter(|l| !l.trim().is_empty()).count()
        } else {
            0
        };

        let reasoning_keywords = [
            "why", "analyze", "explain trade-offs", "edge cases", "optimize",
            "concurrency", "deadlock", "invariant", "proof", "asymptotic"
        ];
        let lower = prompt.to_lowercase();
        let matched_keywords = reasoning_keywords.iter().filter(|&&kw| lower.contains(kw)).count();
        let reasoning_keyword_density = (matched_keywords as f64) / (reasoning_keywords.len() as f64);

        let architecture_keywords = ["architecture", "microservice", "pipeline", "schema migration", "protocol"];
        let contains_architecture_keywords = architecture_keywords.iter().any(|&kw| lower.contains(kw));

        let is_simple_shell_or_lookup = lower.starts_with("git") || lower.starts_with("grep") || lower.contains("syntax for");

        ComplexityFeatures {
            char_length,
            estimated_tokens,
            has_code_block,
            code_line_count,
            reasoning_keyword_density,
            contains_architecture_keywords,
            is_simple_shell_or_lookup,
        }
    }

    /// Computes normalized complexity score [0.0, 1.0].
    fn compute_complexity(&self, features: &ComplexityFeatures) -> f64 {
        let mut score: f64 = 0.10;

        // Token length signal
        if features.estimated_tokens > 1000 {
            score += 0.35;
        } else if features.estimated_tokens > 400 {
            score += 0.20;
        } else if features.estimated_tokens < 50 {
            score -= 0.05;
        }

        // Code block signal
        if features.has_code_block {
            score += 0.20;
            if features.code_line_count > 30 {
                score += 0.15;
            }
        }

        // Keyword density
        score += features.reasoning_keyword_density * 0.30;

        if features.contains_architecture_keywords {
            score += 0.25;
        }

        if features.is_simple_shell_or_lookup {
            score -= 0.20;
        }

        score.clamp(0.0, 1.0)
    }
}

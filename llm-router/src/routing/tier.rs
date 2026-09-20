//! DronaHQ contract tiers (`cheap` / `balanced` / `premium`) → provider cascade.
//!
//! Distinct from the classifier's Thompson-sampling [`super::classifier::Tier`]
//! (Tier1/2/3 quality cells). This module is the frozen external policy table from
//! the Nasiko × DronaHQ handoff.

use serde::{Deserialize, Serialize};

/// External cost/capability hint from `x-nasiko-tier` (and overrides).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContractTier {
    Cheap,
    Balanced,
    Premium,
}

impl ContractTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cheap => "cheap",
            Self::Balanced => "balanced",
            Self::Premium => "premium",
        }
    }

    /// Parse header / body values; unknown → `None` (caller defaults to balanced).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "cheap" => Some(Self::Cheap),
            "balanced" => Some(Self::Balanced),
            "premium" => Some(Self::Premium),
            _ => None,
        }
    }
}

impl std::fmt::Display for ContractTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One hop in a tier cascade: provider id, bare model id, and USD rates per 1k tokens
/// (same units as the DronaHQ mock so `cost_usd` stays comparable).
#[derive(Debug, Clone, Copy)]
pub struct TierCandidate {
    pub provider: &'static str,
    pub model: &'static str,
    /// Input price USD per 1k tokens.
    pub in_rate_per_1k: f64,
    /// Output price USD per 1k tokens.
    pub out_rate_per_1k: f64,
}

impl TierCandidate {
    pub fn estimate_cost_usd(self, prompt_tokens: i64, completion_tokens: i64) -> f64 {
        let p = prompt_tokens.max(0) as f64;
        let c = completion_tokens.max(0) as f64;
        ((p * self.in_rate_per_1k) + (c * self.out_rate_per_1k)) / 1000.0
    }
}

/// Reference cascade from the frozen contract (try in order).
pub fn cascade_for(tier: ContractTier) -> &'static [TierCandidate] {
    match tier {
        ContractTier::Cheap => &CHEAP,
        ContractTier::Balanced => &BALANCED,
        ContractTier::Premium => &PREMIUM,
    }
}

/// Cheapest candidate across all tiers (for budget-exceeded checks).
pub fn cheapest_candidate() -> TierCandidate {
    CHEAP[0]
}

/// NVIDIA NIM escape hatch — used only when contract providers lack working keys.
const NVIDIA_FALLBACK: TierCandidate = TierCandidate {
    provider: "nvidia",
    model: "mistralai/mistral-nemotron",
    in_rate_per_1k: 0.0002,
    out_rate_per_1k: 0.0006,
};

const CHEAP: &[TierCandidate] = &[
    TierCandidate {
        provider: "groq",
        model: "llama-3.1-8b-instant",
        in_rate_per_1k: 0.00005,
        out_rate_per_1k: 0.00008,
    },
    TierCandidate {
        provider: "mistral",
        model: "mistral-small-latest",
        in_rate_per_1k: 0.0001,
        out_rate_per_1k: 0.0003,
    },
    TierCandidate {
        provider: "openai",
        model: "gpt-4o-mini",
        in_rate_per_1k: 0.00015,
        out_rate_per_1k: 0.0006,
    },
    NVIDIA_FALLBACK,
];

const BALANCED: &[TierCandidate] = &[
    TierCandidate {
        provider: "mistral",
        model: "mistral-large-latest",
        in_rate_per_1k: 0.0008,
        out_rate_per_1k: 0.0024,
    },
    TierCandidate {
        provider: "openai",
        model: "gpt-4o-mini",
        in_rate_per_1k: 0.00015,
        out_rate_per_1k: 0.0006,
    },
    TierCandidate {
        provider: "gemini",
        model: "gemini-2.0-flash",
        in_rate_per_1k: 0.0001,
        out_rate_per_1k: 0.0004,
    },
    NVIDIA_FALLBACK,
];

const PREMIUM: &[TierCandidate] = &[
    TierCandidate {
        provider: "openai",
        model: "gpt-4o",
        in_rate_per_1k: 0.0025,
        out_rate_per_1k: 0.01,
    },
    TierCandidate {
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        in_rate_per_1k: 0.003,
        out_rate_per_1k: 0.015,
    },
    TierCandidate {
        provider: "gemini",
        model: "gemini-2.0-flash",
        in_rate_per_1k: 0.0001,
        out_rate_per_1k: 0.0004,
    },
    NVIDIA_FALLBACK,
];

/// Resolve the effective tier from the header hint, complexity, and budget.
///
/// - Missing/invalid header → `balanced`.
/// - High complexity can escalate one step.
/// - Tight `budget_tokens` downshifts toward `cheap`.
pub fn resolve_tier(
    header: Option<&str>,
    complexity: Option<i32>,
    budget_tokens: Option<i64>,
) -> ContractTier {
    let mut tier = header
        .and_then(ContractTier::parse)
        .unwrap_or(ContractTier::Balanced);

    if let Some(c) = complexity {
        if c >= 5 && tier == ContractTier::Cheap {
            tier = ContractTier::Balanced;
        } else if c >= 4 && tier == ContractTier::Cheap {
            tier = ContractTier::Balanced;
        }
    }

    if let Some(b) = budget_tokens {
        if b < 800 {
            tier = ContractTier::Cheap;
        } else if b < 2000 && tier == ContractTier::Premium {
            tier = ContractTier::Balanced;
        }
    }

    tier
}

/// Rough prompt-token estimate (chars / 4), matching the DronaHQ mock.
pub fn estimate_prompt_tokens(messages: &[crate::ir::Message]) -> i64 {
    let chars: usize = messages
        .iter()
        .filter_map(|m| m.text())
        .map(|t| t.len())
        .sum();
    ((chars / 4).max(1)) as i64
}

/// Projected completion tokens under a soft budget (remaining headroom, capped).
pub fn projected_completion_tokens(prompt_tokens: i64, budget_tokens: Option<i64>) -> i64 {
    match budget_tokens {
        Some(b) if b > prompt_tokens => (b - prompt_tokens).clamp(1, 2048),
        Some(_) => 1,
        None => 256,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tier_names() {
        assert_eq!(ContractTier::parse("CHEAP"), Some(ContractTier::Cheap));
        assert_eq!(ContractTier::parse("nope"), None);
    }

    #[test]
    fn budget_downshifts_premium() {
        let t = resolve_tier(Some("premium"), Some(5), Some(1500));
        assert_eq!(t, ContractTier::Balanced);
    }

    #[test]
    fn tight_budget_forces_cheap() {
        let t = resolve_tier(Some("premium"), Some(5), Some(500));
        assert_eq!(t, ContractTier::Cheap);
    }

    #[test]
    fn cascade_order_cheap() {
        let c = cascade_for(ContractTier::Cheap);
        assert_eq!(c[0].provider, "groq");
        assert_eq!(c[2].provider, "openai");
        assert_eq!(c.last().map(|x| x.provider), Some("nvidia"));
    }
}

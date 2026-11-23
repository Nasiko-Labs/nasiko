//! Token usage and cost, as the Nasiko LLM router reports them.
//!
//! The CLI keeps no prices of its own: every dollar comes from the router's `token_usage`
//! rows, priced by its `PricingEngine`. This module only holds the shapes and the checks.

use serde::{Deserialize, Serialize};

/// Token counts for one call or a sum of calls. `input` excludes cache reads and writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub reasoning: u64,
}

impl TokenUsage {
    /// Adds `other` into `self`.
    pub fn add(&mut self, other: &TokenUsage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.reasoning += other.reasoning;
    }

    /// All tokens sent to the model, cached or not.
    pub fn prompt_total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read
    }
}

/// How the router found the price it used, worst last. Mirrors the router's `PriceSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PriceQuality {
    Exact,
    Family,
    Static,
    Ratio,
    /// No rate found at all: the cost is a guess and must not be reported.
    Default,
}

/// The router's totals for one eval attempt (or any other tag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouterUsage {
    pub calls: u32,
    pub usage: TokenUsage,
    pub cost_usd: f64,
    pub models: Vec<String>,
    pub worst_price: PriceQuality,
    pub estimated_calls: u32,
}

/// Why a router total cannot be used as a cost.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PriceError {
    #[error("the router had no price for {models:?}; add a price before reporting cost")]
    Unpriced { models: Vec<String> },
}

/// Fails when any call behind `usage` was priced with the router's default (guessed) rate.
pub fn check_priced(usage: &RouterUsage) -> Result<(), PriceError> {
    if usage.worst_price == PriceQuality::Default {
        return Err(PriceError::Unpriced {
            models: usage.models.clone(),
        });
    }
    Ok(())
}

/// Largest share by which two token counts may differ and still count as agreeing.
pub const TOKEN_AGREEMENT: f64 = 0.02;

/// True when the router's prompt-plus-output total is within [`TOKEN_AGREEMENT`] of the
/// agent's own count. A larger gap means a call was missed or not tagged.
pub fn tokens_agree(router: &TokenUsage, agent: &TokenUsage) -> bool {
    let r = (router.prompt_total() + router.output) as f64;
    let a = (agent.prompt_total() + agent.output) as f64;
    if r == 0.0 && a == 0.0 {
        return true;
    }
    (r - a).abs() / r.max(a) <= TOKEN_AGREEMENT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(worst: PriceQuality) -> RouterUsage {
        RouterUsage {
            calls: 1,
            usage: TokenUsage::default(),
            cost_usd: 0.1,
            models: vec!["zai-org/GLM-5.3".into()],
            worst_price: worst,
            estimated_calls: 0,
        }
    }

    #[test]
    fn default_price_is_rejected() {
        assert!(check_priced(&usage(PriceQuality::Default)).is_err());
        assert!(check_priced(&usage(PriceQuality::Family)).is_ok());
    }

    #[test]
    fn token_counts_within_two_percent_agree() {
        let a = TokenUsage {
            input: 1000,
            output: 100,
            ..Default::default()
        };
        let close = TokenUsage {
            input: 1010,
            output: 100,
            ..Default::default()
        };
        let far = TokenUsage {
            input: 1300,
            output: 100,
            ..Default::default()
        };
        assert!(tokens_agree(&a, &close));
        assert!(!tokens_agree(&a, &far));
    }
}

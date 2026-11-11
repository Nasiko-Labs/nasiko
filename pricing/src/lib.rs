//! Token normalization and cost computation — the single implementation both
//! the observability read path and the LLM gateway price through.
//!
//! The platform used to carry two cost engines: this one's predecessor in
//! `nasiko-observability`, and a plpgsql trigger on `token_usage`. They
//! disagreed on unknown models (Rust fell back to a default rate, SQL returned
//! NULL and booked $0), on missing cache rates (Rust charged the input rate, SQL
//! charged nothing), and on which date's price applied. This crate exists so
//! there is one answer.
//!
//! Four steps, in order:
//!
//! ```ignore
//! let key   = resolve_model(Some("aws-bedrock"), "us.anthropic.claude-opus-4-v1:0");
//! let usage = normalize_usage(raw, PromptConvention::Exclusive);
//! let rates = quote(&[&db_book, &StaticPriceBook], &key, call_started_at, &ratios).await;
//! let spend = cost(&usage, &rates);
//! ```
//!
//! Three invariants hold for every quote, and each replaces a measured defect:
//!
//! * **Never NULL.** A missing price yields an estimated rate, never an absent
//!   cost — a NULL reads as $0 and is indistinguishable from a free call.
//! * **Never the input rate for cache.** 133 of the 137 priced models in the
//!   live book charge 0.5x input or less for a cache read.
//! * **Never silent.** Every quote records which layer answered and whether it
//!   was a lookup or an inference.

mod book;
mod context;
pub use context::PricingContext;
mod cost;
mod db;
mod engine;
mod ledger;
mod model;
mod quote;
mod ratio;
mod usage;

pub use book::{
    BookTier, DEFAULT_INPUT_PER_1M, DEFAULT_OUTPUT_PER_1M, PriceBook, PriceRow, StaticPriceBook,
};
pub use cost::{CostBreakdown, cost};
pub use db::{DbPriceBook, load_cache_ratios};
pub use engine::{PricedCall, PricingEngine};
pub use ledger::{LedgerEntry, NoUsageLedger, UsageLedger};
pub use model::{ModelKey, Vendor, resolve_model};
pub use quote::{PriceQuote, PriceSource, quote};
pub use ratio::{CacheRatio, CacheRatioSample, CacheRatios};
pub use usage::{NormalizedUsage, PromptConvention, RawUsage, infer_fresh_prompt, normalize_usage};

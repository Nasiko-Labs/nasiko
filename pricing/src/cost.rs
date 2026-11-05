//! Cost: four token classes at four rates.

use crate::quote::PriceQuote;
use crate::usage::NormalizedUsage;

/// USD spend, split by token class.
///
/// The split is kept rather than collapsed because it is the only way cache
/// savings are visible: a turn that got cheaper because the cache warmed looks
/// identical to a smaller turn once the components are summed away.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CostBreakdown {
    pub input_usd: f64,
    pub output_usd: f64,
    pub cache_read_usd: f64,
    pub cache_creation_usd: f64,
    pub total_usd: f64,
    /// True when any contributing rate was inferred rather than looked up.
    pub estimated: bool,
}

impl CostBreakdown {
    /// Accumulate another breakdown. `estimated` is sticky: a total containing
    /// one estimated call is itself an estimate.
    pub fn add(&mut self, other: Self) {
        self.input_usd = round_usd(self.input_usd + other.input_usd);
        self.output_usd = round_usd(self.output_usd + other.output_usd);
        self.cache_read_usd = round_usd(self.cache_read_usd + other.cache_read_usd);
        self.cache_creation_usd = round_usd(self.cache_creation_usd + other.cache_creation_usd);
        self.total_usd = round_usd(
            self.input_usd + self.output_usd + self.cache_read_usd + self.cache_creation_usd,
        );
        self.estimated |= other.estimated;
    }
}

/// Price one call. Total is the sum of the four components, always.
pub fn cost(usage: &NormalizedUsage, quote: &PriceQuote) -> CostBreakdown {
    let per_million = |tokens: u64, rate: f64| round_usd(tokens as f64 / 1_000_000.0 * rate);

    let input_usd = per_million(usage.input, quote.input_per_1m);
    let output_usd = per_million(usage.output, quote.output_per_1m);
    let cache_read_usd = per_million(usage.cache_read, quote.cache_read_per_1m);
    let cache_creation_usd = per_million(usage.cache_creation, quote.cache_creation_per_1m);

    let total_usd = round_usd(input_usd + output_usd + cache_read_usd + cache_creation_usd);
    CostBreakdown {
        input_usd,
        output_usd,
        cache_read_usd,
        cache_creation_usd,
        total_usd,
        // An inferred rate only makes a *figure* uncertain if it moved one. A
        // call that spent nothing is exactly zero whatever rate was guessed,
        // and flagging it would matter: `add_assign` keeps `estimated` sticky,
        // so a single token-less span would mark a whole session's total as an
        // estimate while every dollar in it came from a looked-up rate. The same
        // holds per part: inferred cache rates leave a call with no cache use exact.
        estimated: if quote.only_cache_estimated {
            cache_read_usd + cache_creation_usd > 0.0
        } else {
            quote.estimated && total_usd > 0.0
        },
    }
}

/// Six decimal places — a sub-cent-per-call granularity that still sums cleanly
/// over a month of traffic.
fn round_usd(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quote::PriceSource;

    fn quote(input: f64, output: f64, read: f64, creation: f64) -> PriceQuote {
        PriceQuote {
            input_per_1m: input,
            output_per_1m: output,
            cache_read_per_1m: read,
            cache_creation_per_1m: creation,
            cache_creation_1h_per_1m: None,
            source: PriceSource::Exact,
            cache_source: PriceSource::Exact,
            estimated: false,
            only_cache_estimated: false,
            resolved_as: "test".into(),
        }
    }

    fn usage(input: u64, output: u64, read: u64, creation: u64) -> NormalizedUsage {
        NormalizedUsage {
            input,
            output,
            cache_read: read,
            cache_creation: creation,
        }
    }

    #[test]
    fn a_real_trace_prices_to_its_known_total() {
        // trace 432ee5dd from trace_usage: claude-opus-5, and the figure both the
        // observability path and the materializer agree on today.
        let c = cost(
            &usage(214, 55_141, 9_661_334, 265_502),
            &quote(15.0, 75.0, 1.50, 18.75),
        );
        assert!(
            (c.total_usd - 23.6089).abs() < 0.0005,
            "got {}",
            c.total_usd
        );
    }

    #[test]
    fn cached_tokens_are_not_billed_at_the_input_rate() {
        // The same call priced correctly vs. the live `unwrap_or(in_p)` bug.
        let u = usage(214, 55_141, 9_661_334, 265_502);
        let correct = cost(&u, &quote(15.0, 75.0, 1.50, 18.75));
        let buggy = cost(&u, &quote(15.0, 75.0, 15.0, 15.0));
        assert!(
            buggy.total_usd > correct.total_usd * 5.0,
            "the bug should be an order of magnitude: {} vs {}",
            buggy.total_usd,
            correct.total_usd
        );
    }

    #[test]
    fn the_total_is_always_the_sum_of_its_parts() {
        let c = cost(
            &usage(1_000, 2_000, 3_000, 4_000),
            &quote(1.0, 2.0, 3.0, 4.0),
        );
        let parts = c.input_usd + c.output_usd + c.cache_read_usd + c.cache_creation_usd;
        assert!((c.total_usd - parts).abs() < 1e-9);
    }

    #[test]
    fn a_zero_cache_write_rate_is_honoured_rather_than_inflated() {
        // OpenAI does not charge for cache writes; 102 of its 116 priced rows are 0.
        let c = cost(&usage(0, 0, 0, 1_000_000), &quote(2.50, 10.0, 0.25, 0.0));
        assert_eq!(c.cache_creation_usd, 0.0);
        assert_eq!(c.total_usd, 0.0);
    }

    #[test]
    fn an_estimated_rate_marks_the_cost_estimated() {
        let mut q = quote(2.50, 10.0, 0.25, 0.0);
        q.estimated = true;
        assert!(cost(&usage(100, 100, 0, 0), &q).estimated);
    }

    #[test]
    fn a_call_that_spent_nothing_is_not_an_estimate() {
        // `estimated` is sticky when breakdowns accumulate, so a token-less span
        // priced from the fallback table would otherwise mark an entire
        // session's total estimated while every dollar in it was looked up.
        let mut q = quote(2.50, 10.0, 0.25, 0.0);
        q.estimated = true;
        let c = cost(&usage(0, 0, 0, 0), &q);
        assert_eq!(c.total_usd, 0.0);
        assert!(!c.estimated);
    }

    #[test]
    fn accumulating_keeps_estimated_sticky() {
        let mut total = cost(&usage(100, 100, 0, 0), &quote(1.0, 1.0, 1.0, 1.0));
        assert!(!total.estimated);
        let mut estimated_quote = quote(1.0, 1.0, 1.0, 1.0);
        estimated_quote.estimated = true;
        total.add(cost(&usage(100, 100, 0, 0), &estimated_quote));
        assert!(total.estimated, "one estimate makes the total an estimate");
    }

    #[test]
    fn accumulation_keeps_the_total_consistent_with_the_parts() {
        let mut total = CostBreakdown::default();
        for _ in 0..3 {
            total.add(cost(
                &usage(1_000, 500, 200, 100),
                &quote(3.0, 15.0, 0.3, 3.75),
            ));
        }
        let parts =
            total.input_usd + total.output_usd + total.cache_read_usd + total.cache_creation_usd;
        assert!((total.total_usd - parts).abs() < 1e-9);
    }

    #[test]
    fn an_empty_call_costs_nothing() {
        assert_eq!(
            cost(&usage(0, 0, 0, 0), &quote(15.0, 75.0, 1.5, 18.75)).total_usd,
            0.0
        );
    }

    #[test]
    fn inferred_cache_rates_leave_a_call_without_cache_use_exact() {
        let mut q = quote(0.28, 0.42, 0.028, 0.0);
        q.estimated = true; // cache rates inferred, input/output exact
        q.only_cache_estimated = true;
        let c = cost(&usage(1_000, 500, 0, 0), &q);
        assert!(!c.estimated);
    }

    #[test]
    fn inferred_cache_rates_flag_a_call_that_read_the_cache() {
        let mut q = quote(0.28, 0.42, 0.028, 0.0);
        q.estimated = true;
        q.only_cache_estimated = true;
        let c = cost(&usage(1_000, 500, 8_000, 0), &q);
        assert!(c.estimated);
    }

    #[test]
    fn guessed_main_rates_flag_any_priced_call() {
        let mut q = quote(2.50, 10.0, 0.625, 1.5625);
        q.estimated = true;
        let c = cost(&usage(100, 100, 0, 0), &q);
        assert!(c.estimated);
    }
}

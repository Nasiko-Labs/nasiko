//! Turning what IP-1 removed into a priced [`nasiko_savings`] ledger row.
//!
//! The compression block has been in `token_usage.metadata` since IP-1 shipped and has never
//! reached a dashboard: FinOps aggregates `trace_usage`, which has neither a `metadata` column nor
//! a join to `token_usage`. This module writes the same facts somewhere the dashboard can read.
//!
//! The row type, the layer vocabulary and the insert live in `nasiko-savings`, shared with the
//! other writers. What lives here is the part that is genuinely about this seam: converting bytes
//! to tokens using *this call's* reported usage, and pricing them at *this call's* blended rate.
//!
//! # Why the calibration matters more than it looks
//!
//! The headline figure is `saved / (actual + saved)`, where `actual` is provider-reported and
//! real. If `saved` came from a fixed chars-per-token divisor, the error would not cancel between
//! numerator and denominator — a divisor 15% off moves the published reduction percentage by ~15%
//! of itself. So [`chars_per_token`] calibrates against the very request that was sent, and falls
//! back to the constant only when that is impossible, recording which happened.

use nasiko_pricing::CostBreakdown;
use nasiko_savings::{Layer, Method, SavingsRow};
use uuid::Uuid;

/// Fallback when a call reports no usable usage. Matches the divisor already used by
/// `nasiko_react_agent::ContextManager` and `PacmsSelector`.
const DEFAULT_CHARS_PER_TOKEN: f64 = 4.0;

/// Calibrated ratios outside this range are rejected rather than trusted. A request that is mostly
/// image parts, or one whose reported usage covers a different payload than the one measured,
/// lands outside it — and a wrong ratio is worse than the honest constant.
const PLAUSIBLE_CHARS_PER_TOKEN: std::ops::RangeInclusive<f64> = 1.0..=20.0;

/// What the request looked like by the time it went out, for calibration.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CalibrationInputs {
    /// Total text bytes actually sent — after compression and after the brevity directive.
    pub request_bytes: Option<usize>,
    /// The provider's reported prompt-side token count for that same payload.
    pub input_tokens: Option<i64>,
}

/// Identity and pricing context for one call.
///
/// A struct rather than eight positional parameters, so the call site reads as data and a
/// transposed `provider`/`model` pair cannot compile.
pub(crate) struct CallContext<'a> {
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub flow_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub cost: &'a CostBreakdown,
    /// Input + cache-read + cache-creation: the denominator of the blended rate.
    pub prompt_side_tokens: i64,
    pub calibration: CalibrationInputs,
}

/// Chars per token for this call, and whether it had to be assumed.
///
/// `(ratio, estimated)`. `estimated == false` means the ratio came from this call's own bytes and
/// reported tokens, so the saved-token figure is as exact as the provider's own count.
pub(crate) fn chars_per_token(inputs: CalibrationInputs) -> (f64, bool) {
    let calibrated = match (inputs.request_bytes, inputs.input_tokens) {
        (Some(bytes), Some(tokens)) if bytes > 0 && tokens > 0 => {
            Some(bytes as f64 / tokens as f64)
        }
        _ => None,
    };
    match calibrated {
        Some(ratio) if PLAUSIBLE_CHARS_PER_TOKEN.contains(&ratio) => (ratio, false),
        _ => (DEFAULT_CHARS_PER_TOKEN, true),
    }
}

/// The blended price of one prompt-side token on this call.
///
/// Not the list input rate. Compressed tool results sit inside the cacheable prefix, so part of
/// what was elided would have been billed at the cache-read rate — pricing the saving at list
/// would overstate it, systematically, in the flattering direction. Dividing prompt-side cost by
/// prompt-side tokens accounts for whatever cache mix the call actually had.
pub(crate) fn effective_input_rate(cost: &CostBreakdown, prompt_side_tokens: i64) -> f64 {
    if prompt_side_tokens <= 0 {
        return 0.0;
    }
    (cost.input_usd + cost.cache_read_usd + cost.cache_creation_usd) / prompt_side_tokens as f64
}

/// Build the `compress_payload` row, or `None` when IP-1 did not reduce anything.
///
/// Takes the two byte counts rather than [`crate::compress::CompressionStats`], so the usage path
/// threading them through stays free of the compression module's types.
///
/// `None` covers three distinct cases on purpose — the layer was off, it ran in dry-run, or it
/// found nothing to shrink. All three mean no tokens were saved, and a zero row would dilute every
/// average computed over this table.
pub(crate) fn compress_payload_row(
    bytes_in: usize,
    bytes_out: usize,
    ctx: CallContext<'_>,
) -> Option<SavingsRow> {
    if bytes_out >= bytes_in {
        return None;
    }
    let (ratio, token_estimated) = chars_per_token(ctx.calibration);
    // Truncate, never round. Rounding sends half the sub-token savings upward, and a ledger that
    // rounds up systematically claims tokens it cannot account for — every bias in this file is
    // deliberately pointed at under-claiming.
    let saved_tokens = ((bytes_in - bytes_out) as f64 / ratio).floor() as i64;
    if saved_tokens == 0 {
        return None;
    }

    Some(SavingsRow {
        user_id: ctx.user_id,
        agent_id: ctx.agent_id,
        flow_id: ctx.flow_id,
        // The llm-router holds the flow id, not the A2A contextId. The read path resolves it by
        // joining `flow_id` rather than spending a lookup per call on the hot path.
        session_id: None,
        provider: Some(ctx.provider),
        model: Some(ctx.model),
        layer: Layer::CompressPayload,
        bytes_before: Some(bytes_in as i64),
        bytes_after: Some(bytes_out as i64),
        saved_input_tokens: saved_tokens,
        // IP-1 shrinks what the model reads; it cannot shorten what the model writes.
        saved_output_tokens: 0,
        saved_cost_usd: saved_tokens as f64
            * effective_input_rate(ctx.cost, ctx.prompt_side_tokens),
        method: Method::MeasuredBytes,
        token_estimated,
        context_tier: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost() -> CostBreakdown {
        CostBreakdown {
            input_usd: 0.10,
            output_usd: 0.40,
            cache_read_usd: 0.01,
            cache_creation_usd: 0.0,
            total_usd: 0.51,
            estimated: false,
        }
    }

    fn ctx(calibration: CalibrationInputs, cost: &CostBreakdown) -> CallContext<'_> {
        CallContext {
            user_id: Uuid::nil(),
            agent_id: None,
            flow_id: Some("trace-1".into()),
            provider: "openai".into(),
            model: "gpt-4o".into(),
            cost,
            prompt_side_tokens: 1_000,
            calibration,
        }
    }

    fn row(
        bytes_in: usize,
        bytes_out: usize,
        calibration: CalibrationInputs,
    ) -> Option<SavingsRow> {
        let c = cost();
        compress_payload_row(bytes_in, bytes_out, ctx(calibration, &c))
    }

    fn calibrated() -> CalibrationInputs {
        CalibrationInputs {
            request_bytes: Some(8_000),
            input_tokens: Some(2_000),
        }
    }

    // ── calibration ──────────────────────────────────────────────────────────

    #[test]
    fn calibrates_from_the_calls_own_reported_usage() {
        let (ratio, estimated) = chars_per_token(CalibrationInputs {
            request_bytes: Some(9_000),
            input_tokens: Some(3_000),
        });

        assert_eq!(ratio, 3.0);
        assert!(
            !estimated,
            "a calibrated ratio must not be flagged estimated"
        );
    }

    #[test]
    fn falls_back_to_the_constant_without_usable_usage() {
        for inputs in [
            CalibrationInputs {
                request_bytes: None,
                input_tokens: Some(3_000),
            },
            CalibrationInputs {
                request_bytes: Some(9_000),
                input_tokens: None,
            },
            CalibrationInputs {
                request_bytes: Some(9_000),
                input_tokens: Some(0),
            },
            CalibrationInputs {
                request_bytes: Some(0),
                input_tokens: Some(3_000),
            },
        ] {
            let (ratio, estimated) = chars_per_token(inputs);
            assert_eq!(ratio, DEFAULT_CHARS_PER_TOKEN, "for {inputs:?}");
            assert!(estimated, "an assumed ratio must say so: {inputs:?}");
        }
    }

    #[test]
    fn rejects_an_implausible_calibration_rather_than_trusting_it() {
        // A mostly-multimodal request: huge byte count, few text tokens. The ratio it yields is
        // not a tokenizer property, and using it would inflate every saved-token figure.
        let (ratio, estimated) = chars_per_token(CalibrationInputs {
            request_bytes: Some(5_000_000),
            input_tokens: Some(100),
        });

        assert_eq!(ratio, DEFAULT_CHARS_PER_TOKEN);
        assert!(estimated);
    }

    // ── pricing ──────────────────────────────────────────────────────────────

    #[test]
    fn prices_the_saving_at_the_blended_prompt_side_rate() {
        // 0.11 prompt-side dollars over 1000 prompt-side tokens.
        assert!((effective_input_rate(&cost(), 1_000) - 0.000_11).abs() < 1e-12);
    }

    #[test]
    fn the_blended_rate_includes_cache_so_a_cached_call_prices_lower() {
        // The point of blending: a call served largely from cache must not have its saving priced
        // as though every elided token would have been billed fresh.
        let cheap = CostBreakdown {
            input_usd: 0.01,
            cache_read_usd: 0.01,
            ..cost()
        };
        assert!(effective_input_rate(&cheap, 1_000) < effective_input_rate(&cost(), 1_000));
    }

    #[test]
    fn a_call_with_no_prompt_tokens_prices_at_zero_rather_than_dividing_by_zero() {
        assert_eq!(effective_input_rate(&cost(), 0), 0.0);
        assert_eq!(effective_input_rate(&cost(), -5), 0.0);
    }

    // ── row construction ─────────────────────────────────────────────────────

    #[test]
    fn builds_a_row_when_compression_actually_shrank_the_payload() {
        let r = row(10_000, 4_000, calibrated()).expect("a real reduction must produce a row");

        assert_eq!(r.layer, Layer::CompressPayload);
        assert_eq!(r.bytes_before, Some(10_000));
        assert_eq!(r.bytes_after, Some(4_000));
        // 6000 bytes saved at a calibrated 4.0 chars/token.
        assert_eq!(r.saved_input_tokens, 1_500);
        assert_eq!(
            r.saved_output_tokens, 0,
            "IP-1 cannot shorten what the model writes"
        );
        assert_eq!(r.method, Method::MeasuredBytes);
        assert!(!r.token_estimated);
        assert!(r.saved_cost_usd > 0.0);
    }

    #[test]
    fn writes_nothing_when_the_payload_did_not_shrink() {
        // "Ran but nothing shrank" and "grew" both mean no tokens were saved. A zero row would
        // dilute every average computed over this table.
        assert!(row(10_000, 10_000, calibrated()).is_none());
        assert!(row(4_000, 10_000, calibrated()).is_none());
        assert!(row(0, 0, calibrated()).is_none());
    }

    #[test]
    fn a_saving_too_small_to_reach_one_whole_token_is_not_a_row() {
        // 2 bytes at 4 chars/token is half a token. Rounding would claim a whole one.
        assert!(row(1_002, 1_000, CalibrationInputs::default()).is_none());
    }

    #[test]
    fn a_partial_token_is_truncated_rather_than_rounded_up() {
        // 7 bytes at 4 chars/token is 1.75 tokens. One is defensible; two is not.
        let r = row(1_007, 1_000, CalibrationInputs::default()).unwrap();
        assert_eq!(r.saved_input_tokens, 1);
    }

    #[test]
    fn an_uncalibrated_row_is_still_written_but_flagged() {
        // Dropping the saving because usage went unreported would under-report the window;
        // reporting it unflagged would overstate the confidence. Do both honestly.
        let r = row(10_000, 4_000, CalibrationInputs::default())
            .expect("an uncalibrated call still saved real bytes");

        assert_eq!(r.saved_input_tokens, 1_500); // 6000 / 4.0
        assert!(r.token_estimated);
    }

    #[test]
    fn the_session_id_is_left_for_the_read_path_to_resolve() {
        // Deliberate: this seam knows the flow id, not the contextId, and a per-call lookup on the
        // hot path to learn it would be the wrong trade.
        let r = row(10_000, 4_000, calibrated()).unwrap();
        assert_eq!(r.session_id, None);
        assert_eq!(r.flow_id.as_deref(), Some("trace-1"));
    }
}

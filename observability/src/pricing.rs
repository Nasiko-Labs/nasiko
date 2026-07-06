/// Per-token pricing table keyed by model name prefix.
///
/// Returns `(input_usd_per_token, output_usd_per_token)`.
/// Prices are per-token (not per-1k) to keep the arithmetic simple at the call site.
///
/// Sources: OpenAI, Anthropic, and Google published list prices as of 2025-07.
/// When a model is unknown we fall back to GPT-4o pricing so the estimate is
/// conservative rather than zero.
pub fn cost_per_token(model: &str) -> (f64, f64) {
    let m = model.to_lowercase();

    // ── OpenAI ────────────────────────────────────────────────────────────────
    if m.contains("gpt-4.1-nano") {
        return (0.000_000_1, 0.000_000_4);
    }
    if m.contains("gpt-4.1-mini") {
        return (0.000_000_4, 0.000_001_6);
    }
    if m.contains("gpt-4.1") {
        return (0.000_002, 0.000_008);
    }
    if m.contains("gpt-4o-mini") {
        return (0.000_000_15, 0.000_000_6);
    }
    if m.contains("gpt-4o") {
        return (0.000_002_5, 0.000_01);
    }
    if m.contains("gpt-4-turbo") || m.contains("gpt-4-1106") || m.contains("gpt-4-0125") {
        return (0.000_01, 0.000_03);
    }
    if m.contains("gpt-4") {
        return (0.000_03, 0.000_06);
    }
    if m.contains("gpt-3.5-turbo") || m.contains("gpt-3.5") {
        return (0.000_000_5, 0.000_001_5);
    }
    if m.contains("o3-mini") {
        return (0.000_001_1, 0.000_004_4);
    }
    if m.contains("o3") {
        return (0.000_01, 0.000_04);
    }
    if m.contains("o1-mini") {
        return (0.000_003, 0.000_012);
    }
    if m.contains("o1") {
        return (0.000_015, 0.000_06);
    }

    // ── Anthropic ─────────────────────────────────────────────────────────────
    if m.contains("claude-opus-4") || m.contains("claude-4-opus") {
        return (0.000_015, 0.000_075);
    }
    if m.contains("claude-sonnet-4") || m.contains("claude-4-sonnet") {
        return (0.000_003, 0.000_015);
    }
    if m.contains("claude-3-5-sonnet") || m.contains("claude-3.5-sonnet") {
        return (0.000_003, 0.000_015);
    }
    if m.contains("claude-3-5-haiku") || m.contains("claude-3.5-haiku") {
        return (0.000_000_8, 0.000_004);
    }
    if m.contains("claude-3-opus") {
        return (0.000_015, 0.000_075);
    }
    if m.contains("claude-3-sonnet") {
        return (0.000_003, 0.000_015);
    }
    if m.contains("claude-3-haiku") {
        return (0.000_000_25, 0.000_001_25);
    }
    if m.contains("claude") {
        return (0.000_003, 0.000_015); // conservative Claude fallback
    }

    // ── Google ────────────────────────────────────────────────────────────────
    if m.contains("gemini-2.5-pro") {
        return (0.000_001_25, 0.000_01);
    }
    if m.contains("gemini-2.5-flash") {
        return (0.000_000_15, 0.000_000_6);
    }
    if m.contains("gemini-2.0-flash") || m.contains("gemini-2.0") {
        return (0.000_000_1, 0.000_000_4);
    }
    if m.contains("gemini-1.5-pro") {
        return (0.000_001_25, 0.000_005);
    }
    if m.contains("gemini-1.5-flash") {
        return (0.000_000_075, 0.000_000_3);
    }
    if m.contains("gemini") {
        return (0.000_000_5, 0.000_001_5); // conservative Gemini fallback
    }

    // ── Meta / open-weight hosted ─────────────────────────────────────────────
    if m.contains("llama-3.3-70b") || m.contains("llama3.3-70b") {
        return (0.000_000_59, 0.000_000_79);
    }
    if m.contains("llama") || m.contains("mistral") || m.contains("mixtral") {
        return (0.000_000_2, 0.000_000_2); // typical hosted open-weight price
    }

    // ── Unknown: fall back to GPT-4o list price ───────────────────────────────
    (0.000_002_5, 0.000_01)
}

/// Compute USD cost from token counts and a model name.
#[inline]
pub fn estimate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let (in_rate, out_rate) = cost_per_token(model);
    (input_tokens as f64 * in_rate) + (output_tokens as f64 * out_rate)
}
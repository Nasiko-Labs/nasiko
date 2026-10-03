use nasiko_pricing::{PriceSource, PricingEngine, PromptConvention, RawUsage};
use sqlx::{PgPool, Row};

pub fn beneficial(stay: f64, switch: f64, margin: f64) -> bool {
    stay.is_finite()
        && switch.is_finite()
        && stay > 0.0
        && switch >= 0.0
        && margin.is_finite()
        && (0.0..1.0).contains(&margin)
        && switch < stay * (1.0 - margin)
}

pub async fn should_switch(
    db: &PgPool,
    pricing: &PricingEngine,
    owner: &str,
    agent: &str,
    conversation: &str,
    provider: &str,
    current: &str,
    candidate: &str,
    margin: f64,
) -> bool {
    let row = sqlx::query(
        r#"SELECT u.input_tokens, u.output_tokens,
        u.cache_read_input_tokens, u.cache_creation_input_tokens
        FROM token_usage u JOIN flows f ON f.flow_id = u.session_id
        WHERE u.user_id::text=$1 AND u.agent_id::text=$2 AND u.provider=$3 AND u.model=$4
        AND COALESCE(NULLIF(f.metadata->>'context_id',''), f.flow_id)=$5
        AND u.created_at > now() - interval '10 minutes'
        AND u.finish_reason IS NOT NULL
        AND u.metadata->'routing_evidence'->>'input_reported'='true'
        AND u.metadata->'routing_evidence'->>'output_reported'='true'
        AND u.metadata->'routing_evidence'->>'cache_read_reported'='true'
        AND u.metadata->'routing_evidence'->>'cache_creation_reported'='true'
        ORDER BY u.created_at DESC LIMIT 1"#,
    )
    .bind(owner)
    .bind(agent)
    .bind(provider)
    .bind(current)
    .bind(conversation)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    let Some(row) = row else {
        tracing::info!(
            reason = "missing_recent_usage",
            "cache switch policy keeps current model"
        );
        return false;
    };
    let read = |name| {
        row.try_get::<i32, _>(name)
            .ok()
            .filter(|n| *n >= 0)
            .map(|n| n as u64)
    };
    let (Some(input), Some(output), Some(cache_read), Some(cache_creation)) = (
        read("input_tokens"),
        read("output_tokens"),
        read("cache_read_input_tokens"),
        read("cache_creation_input_tokens"),
    ) else {
        return false;
    };
    let stay = pricing
        .price(
            Some(provider),
            current,
            RawUsage {
                input,
                output,
                cache_read: cache_read + cache_creation,
                cache_creation: 0,
                total: None,
            },
            PromptConvention::Exclusive,
            chrono::Utc::now(),
        )
        .await;
    let switch = pricing
        .price(
            Some(provider),
            candidate,
            RawUsage {
                input: input + cache_read + cache_creation,
                output,
                cache_read: 0,
                cache_creation: 0,
                total: None,
            },
            PromptConvention::Exclusive,
            chrono::Utc::now(),
        )
        .await;
    if !matches!(stay.quote.source, PriceSource::Exact)
        || !matches!(switch.quote.source, PriceSource::Exact)
        || ((cache_read + cache_creation > 0)
            && !matches!(stay.quote.cache_source, PriceSource::Exact))
    {
        return false;
    }
    tracing::info!(
        stay_usd = stay.cost.total_usd,
        switch_usd = switch.cost.total_usd,
        margin,
        "estimated next-call switching costs from previous observed usage"
    );
    let allowed = beneficial(stay.cost.total_usd, switch.cost.total_usd, margin);
    tracing::info!(switch_allowed = allowed, "cache switch policy decision");
    allowed
}

#[cfg(test)]
mod tests {
    #[test]
    fn requires_real_margin_and_rejects_unknown_costs() {
        assert!(super::beneficial(0.012, 0.008, 0.2));
        assert!(!super::beneficial(0.012, 0.015, 0.2));
        assert!(!super::beneficial(0.012, 0.011, 0.2));
        assert!(!super::beneficial(f64::NAN, 0.001, 0.2));
    }
}

//! Turns the brevity and minimal-code holdouts into measured effect factors.
//!
//! Neither layer's saving can be subtracted — nobody knows what the model would have written, or
//! how many tool calls it would have made, without them — so the dashboard reports both from
//! `optimization_effect_factors`, seeded with an assumption. This worker is what retires that
//! assumption: it compares the calls (brevity) or turns (minimal-code) that got the treatment
//! against the slice deliberately withheld from it (`brevity::Skipped::Holdout`,
//! `flows.metadata->'minimal_code'->>'skipped' = 'holdout'`) and writes the measured delta back
//! with `basis = 'fixture'`. The arithmetic is identical for both — [`pooled_factor`] doesn't know
//! or care which layer it's measuring — only where each layer's arms come from differs
//! ([`arms`] vs [`minimal_code_arms`]), because brevity's holdout decision lives on the
//! `token_usage` row it governs while minimal-code's lives one layer up, on the `flows` row for
//! the whole turn ([`minimal_code_arms`]'s doc comment has the detail).
//!
//! # Why a within-model comparison, pooled afterwards
//!
//! Output length varies far more between models than between arms. Pooling raw means across models
//! lets the arm ratio be driven by which models happened to land in which arm — Simpson's paradox,
//! and it would move the published figure for reasons that have nothing to do with the directive.
//! So the delta is computed per model and then averaged, weighted by each model's holdout sample
//! count.
//!
//! # Why it refuses to publish a thin estimate
//!
//! Below the sample floor the arm means are noise, and a noisy factor is worse than the honest
//! seed it would replace: the seed is at least labelled as an assumption, while a `fixture` figure
//! claims to be a measurement. So a layer under the floor is left exactly as it was.

use std::time::Duration;

use sqlx::PgPool;
use tokio::time::MissedTickBehavior;

/// One model's two arms over the window.
#[derive(sqlx::FromRow)]
pub(crate) struct ArmRow {
    model: String,
    applied_n: i64,
    applied_avg: Option<f64>,
    holdout_n: i64,
    holdout_avg: Option<f64>,
}

/// What the comparison concluded.
#[derive(Debug, PartialEq)]
pub(crate) struct Factor {
    /// Negative means the directive reduced output tokens.
    pub output_token_delta_pct: f64,
    /// Total holdout samples behind the figure.
    pub sample_count: i64,
    /// The models that contributed, for the row's `model` column.
    pub models: String,
}

/// Compute the pooled factor, or `None` when no model cleared the floor.
///
/// Pure, so the arithmetic is testable without a database — which matters more here than usual,
/// because this function decides a number that appears on a customer-facing dashboard.
pub(crate) fn pooled_factor(rows: &[ArmRow], min_samples: i64) -> Option<Factor> {
    let mut weighted_delta = 0.0;
    let mut weight_total = 0.0;
    let mut samples = 0i64;
    let mut models: Vec<&str> = Vec::new();

    for r in rows {
        let (Some(applied), Some(holdout)) = (r.applied_avg, r.holdout_avg) else {
            continue;
        };
        // Both arms need enough samples: a well-populated treated arm against six control calls is
        // precision on one side of a comparison that has none on the other.
        if r.applied_n < min_samples || r.holdout_n < min_samples || holdout <= 0.0 {
            continue;
        }
        let delta = (applied - holdout) / holdout * 100.0;
        let weight = r.holdout_n as f64;
        weighted_delta += delta * weight;
        weight_total += weight;
        samples += r.holdout_n;
        models.push(&r.model);
    }

    if weight_total == 0.0 {
        return None;
    }
    models.sort_unstable();
    Some(Factor {
        output_token_delta_pct: (weighted_delta / weight_total * 100.0).round() / 100.0,
        sample_count: samples,
        models: models.join(", "),
    })
}

/// One model's two arms for the minimal-code ladder, over the window.
///
/// Unlike brevity, the holdout decision isn't on the `token_usage` row itself — it's made at
/// `a2a_dispatch.rs`, one layer up from the LLM calls it governs, and recorded on the `flows` row
/// for that turn (`flows.metadata->'minimal_code'`). A coding turn can make several LLM calls
/// (tool loop), and every one of them inherits that turn's arm.
///
/// Aggregated per flow *first*, then averaged per model — not a flat per-call average. The
/// ladder's actual mechanism is fewer tool round-trips per turn, not shorter individual replies:
/// a treated turn might take one chunky call where an untreated turn took seven small ones. A
/// per-call average scores the treated turn as *worse* for being efficient; comparing each turn's
/// total against the other arm's turn totals is the comparison the ladder's claim is actually
/// about. (First shipped as a flat per-call average, which is what measured the ladder as making
/// output *longer* — it wasn't; it was making turns shorter in fewer, bigger calls.)
async fn minimal_code_arms(db: &PgPool, window_days: i64) -> Result<Vec<ArmRow>, sqlx::Error> {
    sqlx::query_as(
        r#"WITH per_flow AS (
             SELECT f.flow_id,
                    MAX(tu.model) AS model,
                    SUM(tu.output_tokens) AS total_output,
                    BOOL_OR(f.metadata->'minimal_code'->>'applied' = 'true') AS applied,
                    BOOL_OR(f.metadata->'minimal_code'->>'skipped' = 'holdout') AS is_holdout
             FROM token_usage tu
             JOIN flows f ON f.flow_id = tu.session_id
             WHERE tu.created_at > now() - make_interval(days => $1::int)
               AND tu.output_tokens > 0
               AND f.metadata ? 'minimal_code'
             GROUP BY f.flow_id
           )
           SELECT model,
                  COUNT(*) FILTER (WHERE applied)::BIGINT AS applied_n,
                  AVG(total_output) FILTER (WHERE applied)::FLOAT8 AS applied_avg,
                  COUNT(*) FILTER (WHERE is_holdout)::BIGINT AS holdout_n,
                  AVG(total_output) FILTER (WHERE is_holdout)::FLOAT8 AS holdout_avg
           FROM per_flow
           GROUP BY model"#,
    )
    .bind(window_days as i32)
    .fetch_all(db)
    .await
}

/// Same shape as [`refresh_once`], for the `minimal_code` row. Fail-soft: a failure leaves the
/// existing factor in place.
async fn refresh_once_minimal_code(
    db: &PgPool,
    min_samples: i64,
    window_days: i64,
) -> Result<(), String> {
    let rows = minimal_code_arms(db, window_days)
        .await
        .map_err(|e| e.to_string())?;
    let Some(factor) = pooled_factor(&rows, min_samples) else {
        tracing::debug!(
            target: "nasiko::savings::factors",
            "minimal-code holdout has not cleared the sample floor; the seed stands"
        );
        return Ok(());
    };

    sqlx::query(
        "UPDATE optimization_effect_factors
         SET output_token_delta_pct = $1,
             basis = 'fixture',
             sample_count = $2,
             model = $3,
             measured_at = now(),
             notes = $4
         WHERE layer = 'minimal_code'",
    )
    .bind(factor.output_token_delta_pct)
    .bind(factor.sample_count as i32)
    .bind(&factor.models)
    .bind(format!(
        "Measured against the withheld control arm over {window_days}d: {} holdout turns \
         (each turn's calls summed first, per the ladder's own claim) across {}.",
        factor.sample_count, factor.models
    ))
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;

    tracing::info!(
        target: "nasiko::savings::factors",
        delta_pct = factor.output_token_delta_pct,
        samples = factor.sample_count,
        models = %factor.models,
        "minimal-code factor measured from the holdout"
    );
    Ok(())
}

async fn arms(db: &PgPool, window_days: i64) -> Result<Vec<ArmRow>, sqlx::Error> {
    sqlx::query_as(
        r#"SELECT model,
                  COUNT(*) FILTER (WHERE metadata->'brevity'->>'applied' = 'true')::BIGINT AS applied_n,
                  AVG(output_tokens) FILTER (WHERE metadata->'brevity'->>'applied' = 'true')::FLOAT8 AS applied_avg,
                  COUNT(*) FILTER (WHERE metadata->'brevity'->>'skipped' = 'holdout')::BIGINT AS holdout_n,
                  AVG(output_tokens) FILTER (WHERE metadata->'brevity'->>'skipped' = 'holdout')::FLOAT8 AS holdout_avg
           FROM token_usage
           WHERE created_at > now() - make_interval(days => $1::int)
             AND output_tokens > 0
           GROUP BY model"#,
    )
    .bind(window_days as i32)
    .fetch_all(db)
    .await
}

/// One pass. Fail-soft: a failure leaves the existing factor in place.
pub(crate) async fn refresh_once(
    db: &PgPool,
    min_samples: i64,
    window_days: i64,
) -> Result<(), String> {
    let rows = arms(db, window_days).await.map_err(|e| e.to_string())?;
    let Some(factor) = pooled_factor(&rows, min_samples) else {
        tracing::debug!(
            target: "nasiko::savings::factors",
            "brevity holdout has not cleared the sample floor; the seed stands"
        );
        return Ok(());
    };

    // A positive delta means the directive made output *longer*. Publishing it is the point: that
    // is the §3.3 pathology the carve-outs exist to prevent, and it must surface as a number
    // rather than be quietly floored at zero.
    sqlx::query(
        "UPDATE optimization_effect_factors
         SET output_token_delta_pct = $1,
             basis = 'fixture',
             sample_count = $2,
             model = $3,
             measured_at = now(),
             notes = $4
         WHERE layer = 'brevity'",
    )
    .bind(factor.output_token_delta_pct)
    .bind(factor.sample_count as i32)
    .bind(&factor.models)
    .bind(format!(
        "Measured against the withheld control arm over {window_days}d: {} holdout calls across {}.",
        factor.sample_count, factor.models
    ))
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;

    tracing::info!(
        target: "nasiko::savings::factors",
        delta_pct = factor.output_token_delta_pct,
        samples = factor.sample_count,
        models = %factor.models,
        "brevity factor measured from the holdout"
    );
    Ok(())
}

/// Polling loop, same shape as `hours_meter` / `trace_materializer`: skip missed ticks, fail-soft
/// per pass, never panic.
pub async fn run(db: PgPool, interval: Duration, min_samples: i64, window_days: i64) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tick.tick().await; // skip the immediate first tick, as the other workers do

    loop {
        tick.tick().await;
        if let Err(e) = refresh_once(&db, min_samples, window_days).await {
            tracing::warn!(target: "nasiko::savings::factors", error = %e, "brevity factor refresh failed");
        }
        if let Err(e) = refresh_once_minimal_code(&db, min_samples, window_days).await {
            tracing::warn!(target: "nasiko::savings::factors", error = %e, "minimal-code factor refresh failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(model: &str, applied_n: i64, applied: f64, holdout_n: i64, holdout: f64) -> ArmRow {
        ArmRow {
            model: model.into(),
            applied_n,
            applied_avg: Some(applied),
            holdout_n,
            holdout_avg: Some(holdout),
        }
    }

    #[test]
    fn measures_the_reduction_against_the_untreated_arm() {
        // Treated wrote 650 where untreated wrote 1000: a 35% reduction.
        let f = pooled_factor(&[row("gpt-4o", 2000, 650.0, 2000, 1000.0)], 100).unwrap();
        assert_eq!(f.output_token_delta_pct, -35.0);
        assert_eq!(f.sample_count, 2000);
        assert_eq!(f.models, "gpt-4o");
    }

    #[test]
    fn a_layer_that_made_output_longer_reports_a_positive_delta() {
        // Terseness during tool use can cost more than it saves. Flooring this at zero would hide
        // exactly the failure the measurement exists to catch.
        let f = pooled_factor(&[row("gpt-4o", 2000, 1200.0, 2000, 1000.0)], 100).unwrap();
        assert_eq!(f.output_token_delta_pct, 20.0);
    }

    #[test]
    fn models_are_compared_within_themselves_then_pooled() {
        // Both models saw a 50% reduction, so the pooled answer is 50% — even though their
        // absolute lengths differ by 10x. Pooling raw means instead would return something else
        // entirely, driven by which model happened to dominate the sample.
        let f = pooled_factor(
            &[
                row("small", 1000, 50.0, 1000, 100.0),
                row("large", 1000, 500.0, 1000, 1000.0),
            ],
            100,
        )
        .unwrap();
        assert_eq!(f.output_token_delta_pct, -50.0);
        assert_eq!(f.sample_count, 2000);
        assert_eq!(f.models, "large, small");
    }

    #[test]
    fn weights_each_model_by_its_own_control_sample_count() {
        // 900 control calls at -40% and 100 at -10% pool to -37%, not the -25% an unweighted mean
        // would give.
        let f = pooled_factor(
            &[
                row("a", 900, 60.0, 900, 100.0),
                row("b", 100, 90.0, 100, 100.0),
            ],
            50,
        )
        .unwrap();
        assert_eq!(f.output_token_delta_pct, -37.0);
    }

    #[test]
    fn refuses_to_publish_below_the_sample_floor() {
        // A noisy `fixture` figure is worse than the seed it would replace: the seed is at least
        // labelled an assumption, while a fixture claims to be a measurement.
        assert!(pooled_factor(&[row("gpt-4o", 10, 650.0, 10, 1000.0)], 1600).is_none());
    }

    #[test]
    fn requires_both_arms_to_clear_the_floor() {
        // Precision on one side of a comparison that has none on the other is not a measurement.
        assert!(pooled_factor(&[row("gpt-4o", 5000, 650.0, 3, 1000.0)], 1600).is_none());
        assert!(pooled_factor(&[row("gpt-4o", 3, 650.0, 5000, 1000.0)], 1600).is_none());
    }

    #[test]
    fn a_model_with_only_one_arm_contributes_nothing() {
        let only_treated = ArmRow {
            model: "gpt-4o".into(),
            applied_n: 5000,
            applied_avg: Some(650.0),
            holdout_n: 0,
            holdout_avg: None,
        };
        assert!(pooled_factor(&[only_treated], 100).is_none());
    }

    #[test]
    fn a_zero_length_control_arm_is_skipped_rather_than_dividing_by_zero() {
        assert!(pooled_factor(&[row("gpt-4o", 2000, 650.0, 2000, 0.0)], 100).is_none());
    }

    #[test]
    fn one_qualifying_model_still_publishes_while_others_are_ignored() {
        let f = pooled_factor(
            &[
                row("qualifies", 2000, 500.0, 2000, 1000.0),
                row("too-thin", 5, 100.0, 5, 1000.0),
            ],
            1000,
        )
        .unwrap();
        assert_eq!(f.output_token_delta_pct, -50.0);
        assert_eq!(f.models, "qualifies");
    }
}

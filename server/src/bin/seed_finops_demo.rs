//! Synthetic FinOps/usage data seeder for local demos of the weave TokenOps
//! dashboard. Populates `trace_usage`, which both the fleet-wide FinOps endpoints
//! and the per-caller usage endpoints (`trace_spend`) read, with plausible
//! multi-agent, multi-week data — it is otherwise empty until real agent
//! traffic/materialization happens (see plan doc for the verified reason).
//!
//! Idempotent: re-running clears its own previously-seeded rows first
//! (tagged via an `agent_name`/`session_id` prefix), so this can be run
//! repeatedly without piling up duplicate data.
//!
//! Usage: `cargo run --bin seed-finops-demo` (reads `DATABASE_URL` from the
//! environment / `.env`, same as the main server binary).

use chrono::{Duration, Utc};
use rand::Rng;
use sqlx::PgPool;
use uuid::Uuid;

const SEED_PREFIX: &str = "demo-";
// A full year, not just 8-9 weeks: the calendar heatmap looks sparse/empty
// below a few months of history, and every FinOps endpoint's own real UI
// (month-by-month spend-calendar drilldown) needs more than one or two
// months to be worth paging through during manual testing.
const DAYS_BACK: i64 = 365;

/// (agent_name, provider, model, input_price_per_1m, output_price_per_1m,
/// weight). Provider/model pairs match real rows in `model_pricing`
/// (`oss/migrations/0006_seed_model_pricing.sql`) so a real cost lookup would
/// agree with the hand-computed `cost_usd` this seeder writes for
/// `trace_usage` (which has no cost trigger).
///
/// 24 agents, not 5: `get_finops_attributions`'s default "agent" view is one
/// row per agent (`service.rs::get_finops_attributions`) — 5 agents means
/// the attribution table can never show more than 5 rows, so pagination is
/// untestable. Several agents deliberately share the same model so
/// "usage by model" aggregates across multiple agents instead of mirroring
/// "usage by agent" 1:1. `weight` varies relative daily traffic (1 = light,
/// 3 = heavy) so the trend/heatmap/attribution views show a real
/// distribution instead of uniform bars.
const AGENTS: &[(&str, &str, &str, f64, f64, u32)] = &[
    (
        "demo-support-triage",
        "anthropic",
        "claude-3-5-sonnet",
        3.00,
        15.00,
        3,
    ),
    (
        "demo-support-escalation",
        "anthropic",
        "claude-3-5-sonnet",
        3.00,
        15.00,
        2,
    ),
    ("demo-code-review", "openai", "gpt-4o", 2.50, 10.00, 3),
    ("demo-code-refactor", "openai", "gpt-4o", 2.50, 10.00, 2),
    (
        "demo-pr-summarizer",
        "openai",
        "gpt-4.1-mini",
        0.40,
        1.60,
        2,
    ),
    (
        "demo-finops-analyst",
        "anthropic",
        "claude-3-5-haiku",
        0.80,
        4.00,
        2,
    ),
    (
        "demo-invoice-classifier",
        "anthropic",
        "claude-3-5-haiku",
        0.80,
        4.00,
        1,
    ),
    (
        "demo-rag-search",
        "google",
        "gemini-2.5-flash",
        0.15,
        0.60,
        3,
    ),
    ("demo-doc-qa", "google", "gemini-2.5-flash", 0.15, 0.60, 2),
    (
        "demo-image-caption",
        "google",
        "gemini-2.5-pro",
        1.25,
        10.00,
        1,
    ),
    (
        "demo-lead-classifier",
        "openai",
        "gpt-4o-mini",
        0.15,
        0.60,
        3,
    ),
    ("demo-email-router", "openai", "gpt-4o-mini", 0.15, 0.60, 2),
    (
        "demo-crm-enrichment",
        "openai",
        "gpt-4.1-nano",
        0.10,
        0.40,
        1,
    ),
    ("demo-sql-generator", "openai", "o3-mini", 1.10, 4.40, 1),
    (
        "demo-onboarding-guide",
        "anthropic",
        "claude-haiku-4",
        0.80,
        4.00,
        2,
    ),
    (
        "demo-release-notes",
        "anthropic",
        "claude-sonnet-4",
        3.00,
        15.00,
        1,
    ),
    (
        "demo-incident-summarizer",
        "anthropic",
        "claude-opus-4",
        15.00,
        75.00,
        1,
    ),
    (
        "demo-fast-classifier",
        "groq",
        "llama-3.1-8b-instant",
        0.05,
        0.08,
        3,
    ),
    (
        "demo-chatops-bot",
        "groq",
        "llama-3.3-70b-versatile",
        0.59,
        0.79,
        2,
    ),
    (
        "demo-cost-optimizer",
        "deepseek",
        "deepseek-chat",
        0.14,
        0.28,
        1,
    ),
    (
        "demo-deep-reasoner",
        "deepseek",
        "deepseek-reasoner",
        0.55,
        2.19,
        1,
    ),
    ("demo-log-triage", "openai", "gpt-3.5-turbo", 0.50, 1.50, 2),
    (
        "demo-migration-planner",
        "openai",
        "o1-mini",
        3.00,
        12.00,
        1,
    ),
    (
        "demo-forecast-analyst",
        "google",
        "gemini-1.5-pro",
        1.25,
        5.00,
        1,
    ),
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set (same as the main server binary)");
    let db = PgPool::connect(&database_url).await?;

    let user_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL \
         ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&db)
    .await?
    .ok_or_else(|| {
        anyhow::anyhow!(
            "no superuser found — create at least one user before seeding demo FinOps data"
        )
    })?;

    println!("Seeding synthetic FinOps/usage data for user {user_id}...");

    // Idempotent: clear this seeder's own previously-inserted rows first.
    sqlx::query("DELETE FROM trace_usage WHERE agent_name LIKE $1")
        .bind(format!("{SEED_PREFIX}%"))
        .execute(&db)
        .await?;

    // `get_finops_dashboard`/`get_finops_attributions` (agent view) enumerate
    // the real `agents` table and match `trace_usage.agent_name` against
    // `agents.name` — they do NOT aggregate `trace_usage` on its own. Without
    // a matching `agents` row, synthetic spend is invisible to those two
    // endpoints (it still shows up in spend-timeseries/spend-calendar, which
    // group directly by `agent_name`). `status = 'registered'` (the column
    // default) is deliberately inert — it does not trigger the reconciliation
    // worker's redeploy behavior the way `status = 'running'` would.
    for &(agent_name, _, _, _, _, _) in AGENTS {
        sqlx::query(
            "INSERT INTO agents (name, display_name, owner_id, status) \
             VALUES ($1, $1, $2, 'registered') \
             ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL DO NOTHING",
        )
        .bind(agent_name)
        .bind(user_id)
        .execute(&db)
        .await?;
    }

    let mut rng = rand::rng();
    let mut trace_rows = 0u32;

    let mut tx = db.begin().await?;
    let now = Utc::now();

    for day_offset in 0..DAYS_BACK {
        let day = now - Duration::days(day_offset);
        // Weekly cycle: weekdays busier than weekends, plus noise, so the
        // calendar heatmap and trend charts show real variation instead of a
        // flat line.
        let is_weekend = matches!(day.format("%u").to_string().as_str(), "6" | "7");
        let base_events_per_agent = if is_weekend { 2 } else { 8 };

        for &(agent_name, provider, model, input_price, output_price, weight) in AGENTS {
            let base = base_events_per_agent * weight;
            let events = rng.random_range(0..=base * 2).max(base.saturating_sub(4));

            for _ in 0..events {
                let hour = rng.random_range(0..24);
                let minute = rng.random_range(0..60);
                let started_at = day
                    .date_naive()
                    .and_hms_opt(hour, minute, 0)
                    .unwrap()
                    .and_utc();

                let input_tokens: i64 = rng.random_range(200..4_000);
                let output_tokens: i64 = rng.random_range(50..1_500);
                let cache_read_tokens: i64 = rng.random_range(0..800);
                let latency_ms: i64 = rng.random_range(300..6_000);
                let tool_calls: i64 = rng.random_range(0..4);

                let prompt_cost = (input_tokens as f64 / 1_000_000.0) * input_price;
                let completion_cost = (output_tokens as f64 / 1_000_000.0) * output_price;
                let cost = prompt_cost + completion_cost;

                let trace_id = format!("{SEED_PREFIX}{}", Uuid::new_v4());
                let session_id = format!("{SEED_PREFIX}{}", Uuid::new_v4());

                sqlx::query(
                    "INSERT INTO trace_usage \
                       (trace_id, agent_name, session_id, user_id, model, provider, \
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, \
                        cost_usd, prompt_cost_usd, completion_cost_usd, latency_ms, \
                        tool_call_count, started_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,0,$10,$11,$12,$13,$14,$15)",
                )
                .bind(&trace_id)
                .bind(agent_name)
                .bind(&session_id)
                .bind(user_id)
                .bind(model)
                .bind(provider)
                .bind(input_tokens)
                .bind(output_tokens)
                .bind(cache_read_tokens)
                .bind(cost)
                .bind(prompt_cost)
                .bind(completion_cost)
                .bind(latency_ms)
                .bind(tool_calls)
                .bind(started_at)
                .execute(&mut *tx)
                .await?;
                trace_rows += 1;
            }
        }
    }

    tx.commit().await?;

    println!(
        "Seeded {trace_rows} trace_usage rows across \
         {DAYS_BACK} days for {} synthetic agents.",
        AGENTS.len()
    );
    Ok(())
}

//! Composition root: read the port, wire the agent into an A2A server, start the scheduled
//! Slack digest, and serve. Everything of substance lives in the modules below.

use std::sync::Arc;

use a2a_server::{DefaultRequestHandler, InMemoryTaskStore, StaticAgentCard};
use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Utc};

mod agent;
mod github;
mod slack;
mod telemetry;
mod tools;

use agent::RepoWatchAgent;

const DEFAULT_PORT: u16 = 8000;

/// Defaults for the twice-daily Slack digest, overridable via `DIGEST_MORNING_TIME` /
/// `DIGEST_EVENING_TIME` (format `HH:MM`, interpreted in IST) — mainly so a real end-to-end
/// test doesn't require waiting for the actual time of day to roll around.
const DEFAULT_MORNING_TIME: &str = "07:30";
const DEFAULT_EVENING_TIME: &str = "19:30";

/// IST (Asia/Kolkata) has no DST, so a fixed UTC+5:30 offset is correct year-round — no
/// `chrono-tz` dependency needed.
const IST_OFFSET_SECONDS: i32 = 5 * 3600 + 30 * 60;

const SCHEDULED_DIGEST_PROMPT: &str = "What changed in the watch list in the last 12 hours?";

#[tokio::main]
async fn main() {
    // Load a local `.env` if present, so config (LLM keys, Slack, the digest trigger times)
    // can be edited there and picked up by `cargo run` — not just via the container's
    // `--env-file`. Real process env still wins; a missing file is not an error.
    dotenvy::dotenv().ok();

    telemetry::init();

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let agent = RepoWatchAgent::new();
    spawn_digest_scheduler(agent.clone());

    let handler = Arc::new(DefaultRequestHandler::new(agent, InMemoryTaskStore::new()));
    let card = Arc::new(StaticAgentCard::new(agent::agent_card(port)));

    let app = axum::Router::new()
        .merge(a2a_server::jsonrpc::jsonrpc_router(handler))
        .merge(a2a_server::agent_card::agent_card_router(card));

    tracing::info!("Repo Watch Agent listening on 0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind");
    axum::serve(listener, app).await.expect("server failed");
}

// ─── Scheduled Slack digest ───────────────────────────────────────────────────

/// Runs forever in the background: wakes at the next 07:30/19:30 IST trigger, generates a
/// digest via the LLM + GitHub tools (unchanged from the on-demand path), then posts it to
/// Slack deterministically — posting never depends on the model choosing to call a tool. A
/// failure (LLM error, Slack outage) is logged and never panics, so it can't take down the A2A
/// server running alongside it.
fn spawn_digest_scheduler(agent: RepoWatchAgent) {
    let morning = parse_trigger_time("DIGEST_MORNING_TIME", DEFAULT_MORNING_TIME);
    let evening = parse_trigger_time("DIGEST_EVENING_TIME", DEFAULT_EVENING_TIME);

    tokio::spawn(async move {
        loop {
            let wait = next_trigger_duration(Utc::now(), morning, evening);
            tracing::info!("next scheduled Slack digest in {:.0}s", wait.as_secs_f64());
            tokio::time::sleep(wait).await;

            match agent
                .run_digest(SCHEDULED_DIGEST_PROMPT.to_string(), None, None)
                .await
            {
                Ok(digest) => {
                    let comment = format!("Repo digest — {}", Utc::now().to_rfc3339());
                    if let Err(e) =
                        slack::post_markdown_file(&digest, "repo-digest.md", &comment).await
                    {
                        tracing::error!("scheduled Slack post failed: {e}");
                    }
                }
                Err(e) => tracing::error!("scheduled digest generation failed: {e}"),
            }
        }
    });
}

/// Reads an `HH:MM` trigger time from `env_var`, falling back to `default` if unset or
/// unparsable (logging a warning in the latter case so a typo doesn't fail silently).
fn parse_trigger_time(env_var: &str, default: &str) -> NaiveTime {
    let raw = std::env::var(env_var).unwrap_or_else(|_| default.to_string());
    NaiveTime::parse_from_str(&raw, "%H:%M").unwrap_or_else(|_| {
        tracing::warn!("{env_var}='{raw}' is not HH:MM; using default {default}");
        NaiveTime::parse_from_str(default, "%H:%M").expect("default trigger time is valid")
    })
}

/// How long to sleep from `now_utc` until the next `morning` or `evening` trigger, in IST. Pure
/// and side-effect-free so it's easy to unit-test across the day/midnight-rollover cases.
fn next_trigger_duration(
    now_utc: DateTime<Utc>,
    morning: NaiveTime,
    evening: NaiveTime,
) -> std::time::Duration {
    let ist = FixedOffset::east_opt(IST_OFFSET_SECONDS).expect("IST offset is a valid FixedOffset");
    let now_ist = now_utc.with_timezone(&ist);
    let today = now_ist.date_naive();

    // Both times for today and tomorrow, then take the earliest that's still ahead of now.
    // Using `min` over the full set (rather than `find` over an assumed-sorted list) keeps this
    // correct even if morning/evening are configured out of order.
    let tomorrow = today + chrono::Duration::days(1);
    let candidates = [
        today.and_time(morning),
        today.and_time(evening),
        tomorrow.and_time(morning),
        tomorrow.and_time(evening),
    ];
    let next = candidates
        .into_iter()
        .map(|naive| {
            ist.from_local_datetime(&naive)
                .single()
                .expect("a fixed UTC offset has no ambiguous local times")
        })
        .filter(|candidate| *candidate > now_ist)
        .min()
        .expect("tomorrow's triggers are always still ahead of now");

    (next - now_ist)
        .to_std()
        .expect("the chosen candidate was filtered to be strictly after now_ist")
}

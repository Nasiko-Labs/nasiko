//! Side-by-side comparison of the three context-selection strategies against
//! one seeded session, through the real `context_selection::fetch_for_user`
//! entry point.
//!
//! Needs infra + `DATABASE_URL`, and an `OPENAI_API_KEY` for the two
//! embedding-backed strategies, so it is `#[ignore]` by default:
//!
//!     cargo test -p nasiko-orchestrator --test strategy_comparison -- --ignored --nocapture

use nasiko_orchestrator::{ContextTiers, VectorStore, context_selection};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

/// A session whose turns are deliberately spread across unrelated topics, so
/// "most relevant to the query" and "most recent" are different answers and
/// the strategies visibly disagree.
const TURNS: &[(&str, &str)] = &[
    (
        "My postgres connection pool keeps exhausting under load.",
        "Raise max_connections and set a statement_timeout; pool exhaustion is usually a leaked transaction.",
    ),
    (
        "What's a good sourdough hydration for beginners?",
        "Start at 70% hydration — wetter doughs are harder to shape until you've built the technique.",
    ),
    (
        "How do I rotate the TLS cert on the ingress?",
        "Replace the secret and restart the ingress controller; cert-manager will re-issue if the annotation is present.",
    ),
    (
        "Recommend a hiking trail near Lisbon.",
        "Try the Sintra-Cascais park — the Praia da Ursa descent is short but steep.",
    ),
    (
        "The pool exhaustion is back after the deploy.",
        "Check for a connection leak in the new code path — an un-awaited transaction holds its connection until timeout.",
    ),
    (
        "What lens should I use for night photography?",
        "A fast prime, f/1.8 or wider; keep ISO under 3200 to limit noise.",
    ),
    (
        "How do I read the slow query log?",
        "Enable log_min_duration_statement and tail it; anything over 500ms is worth an index review.",
    ),
    (
        "Best way to reheat pizza?",
        "Cast iron on the stove with a lid — crisp base, melted top, about four minutes.",
    ),
];

/// The query under test: clearly about the database thread, which is turns
/// 0, 4 and 6 — none of which are the most recent.
const QUERY: &str = "Why is my database connection pool still running out?";

/// `TURNS` alone is only ~380 estimated tokens, which is under even the "low"
/// 500-token budget — every PACMS tier would keep the whole session and the
/// tiers would look identical. Repeat the topic mix, with each turn padded to
/// a realistic length, until the session is big enough that all three budgets
/// bind and the tiers actually separate.
const REPEATS: usize = 6;
const PAD: &str = " For context, this is the same environment as before: the staging cluster, \
     running the current release, with the usual traffic profile and no recent \
     infrastructure changes worth calling out.";

fn turns() -> Vec<(String, String)> {
    (0..REPEATS)
        .flat_map(|r| {
            TURNS
                .iter()
                .map(move |(u, a)| (format!("(round {r}) {u}{PAD}"), format!("{a}{PAD}")))
        })
        .collect()
}

async fn seed(pool: &PgPool, session_id: &str, user_id: Uuid) {
    sqlx::query("DELETE FROM chat_messages WHERE session_id = $1")
        .bind(session_id)
        .execute(pool)
        .await
        .expect("clear session");

    // chat_messages.session_id is FK'd to chat_sessions.
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, NULL, '/api/orchestrator/a2a', 'strategy comparison') \
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(session_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed session");

    // Explicit, increasing timestamps: the fetches order by `timestamp`, and
    // inserting in a loop can land several rows inside the same clock tick.
    let all = turns();
    for (i, (user, assistant)) in all.iter().enumerate() {
        for (role, content) in [("user", user), ("assistant", assistant)] {
            sqlx::query(
                "INSERT INTO chat_messages (session_id, role, content, timestamp) \
                 VALUES ($1, $2, $3, now() - make_interval(secs => $4))",
            )
            .bind(session_id)
            .bind(role)
            .bind(content)
            .bind((all.len() * 2 - i * 2) as f64)
            .execute(pool)
            .await
            .expect("seed message");
        }
    }
}

async fn set_prefs(pool: &PgPool, user_id: Uuid, strategy: &str, level: &str) {
    sqlx::query(&format!(
        "UPDATE users SET context_selection_strategy = '{strategy}', \
         pacms_budget_level = '{level}' WHERE id = $1"
    ))
    .bind(user_id)
    .execute(pool)
    .await
    .expect("set preferences");
}

#[tokio::test]
#[ignore = "needs DATABASE_URL + OPENAI_API_KEY"]
async fn compare_strategies_on_one_session() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let pool = PgPool::connect(&db_url).await.expect("connect");

    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users ORDER BY created_at LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("a user row must exist — log in once first");

    let session_id = "strategy-comparison";
    seed(&pool, session_id, user_id).await;

    let store = VectorStore::for_embedding(
        api_key,
        "https://api.openai.com".to_string(),
        "text-embedding-3-small".to_string(),
        Arc::new(dashmap::DashMap::new()),
    );

    let total_chars: usize = turns().iter().map(|(u, a)| u.len() + a.len()).sum();
    println!(
        "\nseeded {} turns ({} messages, ~{} estimated tokens); query: {QUERY:?}\n",
        turns().len(),
        turns().len() * 2,
        total_chars / 4,
    );

    for level in ["low", "medium", "high"] {
        for strategy in ["pacms", "topk", "lastk"] {
            set_prefs(&pool, user_id, strategy, level).await;
            let history = context_selection::fetch_for_user(
                &pool,
                user_id,
                session_id,
                &store,
                QUERY,
                &ContextTiers::default(),
            )
            .await;

            let tokens: usize = history.messages.iter().map(|m| m.content.len() / 4).sum();
            let db_hits = history
                .messages
                .iter()
                .filter(|m| {
                    let c = m.content.to_lowercase();
                    c.contains("pool")
                        || c.contains("postgres")
                        || c.contains("query log")
                        || c.contains("connection")
                        || c.contains("index review")
                })
                .count();
            println!(
                "── {strategy:<5} / {level:<6} → {:>3} messages, ~{:>5} tokens, {:>2} on-topic (database)",
                history.messages.len(),
                tokens,
                db_hits,
            );
        }
    }

    sqlx::query("DELETE FROM chat_messages WHERE session_id = $1")
        .bind(session_id)
        .execute(&pool)
        .await
        .ok();
}

/// `fetch_topk` used to `SELECT` a session's entire history with no `LIMIT`
/// and embed every pair of it. It now draws from the same `pool_size` window
/// `fetch_pacms` uses, so a long session cannot grow the query or the
/// per-request embedding cost without bound.
#[tokio::test]
#[ignore = "needs DATABASE_URL + OPENAI_API_KEY"]
async fn topk_is_bounded_by_pool_size() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let pool = PgPool::connect(&db_url).await.expect("connect");

    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users ORDER BY created_at LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("a user row must exist");

    let session_id = "topk-pool-bound";
    seed(&pool, session_id, user_id).await;

    let store = VectorStore::for_embedding(
        api_key,
        "https://api.openai.com".to_string(),
        "text-embedding-3-small".to_string(),
        Arc::new(dashmap::DashMap::new()),
    );

    // top_k far exceeds the pool, so the pool is the only thing limiting the
    // result: at most `POOL / 2` pairs, i.e. `POOL` messages. Unbounded, this
    // returned all 96 seeded messages.
    const POOL: usize = 10;
    let history = nasiko_orchestrator::SessionHistory::fetch_topk(
        session_id,
        &pool,
        QUERY,
        &store,
        999,
        POOL,
        &Default::default(),
    )
    .await;

    println!(
        "seeded {} messages, pool_size={POOL} → fetch_topk returned {}",
        turns().len() * 2,
        history.messages.len(),
    );
    assert!(
        history.messages.len() <= POOL,
        "fetch_topk returned {} messages from a {}-message session with pool_size={POOL} \
         — the window is not being applied",
        history.messages.len(),
        turns().len() * 2,
    );

    sqlx::query("DELETE FROM chat_messages WHERE session_id = $1")
        .bind(session_id)
        .execute(&pool)
        .await
        .ok();
}

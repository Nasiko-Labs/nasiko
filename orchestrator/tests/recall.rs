//! Recall of the three context-selection strategies over 10 labelled samples.
//!
//! Each sample is a session built from ten topics. One topic is the *gold*
//! topic: its three turns are what a perfect selector would return for that
//! sample's query. The other nine topics contribute one distractor turn each,
//! and the gold turns are placed early so recency alone cannot find them —
//! `lastk` has to fail for the metric to mean anything.
//!
//!     recall = |selected ∩ gold| / |gold|
//!
//! measured on the *user* half of each turn, and averaged over the ten
//! samples. Embeddings are deterministic for a fixed model, so the numbers
//! below are reproducible; a change in them is a real change in selection
//! behaviour, not flake.
//!
//! Needs infra + `DATABASE_URL` + `OPENAI_API_KEY`, so it is `#[ignore]`:
//!
//!     cargo test -p nasiko-orchestrator --test recall -- --ignored --nocapture

use nasiko_orchestrator::{ContextTiers, VectorStore, context_selection};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

/// Ten topics. Per topic: the query asked about it, and its three turns.
/// Vocabulary is deliberately disjoint across topics so relevance is not
/// ambiguous — a selector either found the topic's turns or it did not.
struct Topic {
    query: &'static str,
    turns: [(&'static str, &'static str); 3],
}

const TOPICS: [Topic; 10] = [
    Topic {
        query: "Why does my database connection pool keep running out?",
        turns: [
            (
                "The postgres connection pool is exhausting under load.",
                "Pool exhaustion is usually a leaked transaction holding its connection.",
            ),
            (
                "Pool exhaustion came back after the deploy.",
                "Look for an un-awaited transaction in the new code path.",
            ),
            (
                "How do I read the slow query log?",
                "Enable log_min_duration_statement; anything over 500ms deserves an index.",
            ),
        ],
    },
    Topic {
        query: "How should I proof sourdough in a cold kitchen?",
        turns: [
            (
                "What hydration should a beginner use for sourdough?",
                "Start near 70% — wetter doughs are hard to shape until your technique is there.",
            ),
            (
                "My sourdough starter smells like acetone.",
                "It is hungry; feed it twice a day at a 1:5:5 ratio for a few days.",
            ),
            (
                "How long should bulk fermentation run?",
                "Until the dough rises about 50% and domes — time is a poor proxy, watch the dough.",
            ),
        ],
    },
    Topic {
        query: "How do I renew the TLS certificate on the ingress?",
        turns: [
            (
                "How do I rotate the TLS cert on the ingress?",
                "Replace the secret and restart the controller; cert-manager reissues if annotated.",
            ),
            (
                "The cert-manager challenge is stuck pending.",
                "The ACME HTTP-01 challenge needs port 80 reachable from the internet.",
            ),
            (
                "Our certificate expired over the weekend.",
                "Shorten the renewal window; cert-manager renews at two-thirds of the lifetime.",
            ),
        ],
    },
    Topic {
        query: "Can you recommend a coastal hike near Lisbon?",
        turns: [
            (
                "Recommend a hiking trail near Lisbon.",
                "Sintra-Cascais park; the Praia da Ursa descent is short but steep.",
            ),
            (
                "Is the Rota Vicentina worth doing in spring?",
                "Yes — the Fishermen's Trail is best in April before the heat arrives.",
            ),
            (
                "What boots suit sandy coastal trails?",
                "Low-cut and breathable; ankle support matters less than drainage on sand.",
            ),
        ],
    },
    Topic {
        query: "What lens works best for astrophotography?",
        turns: [
            (
                "What lens should I use for night photography?",
                "A fast prime, f/1.8 or wider; keep ISO under 3200 to limit noise.",
            ),
            (
                "How do I avoid star trails in long exposures?",
                "Divide 500 by your focal length for the maximum shutter time in seconds.",
            ),
            (
                "Is a tracker worth it for the Milky Way?",
                "A star tracker buys minutes of exposure instead of seconds — transformative.",
            ),
        ],
    },
    Topic {
        query: "How do I stop my bread dough sticking to the banneton?",
        turns: [
            (
                "What is the best way to reheat leftover pizza?",
                "Cast iron on the stove with a lid — crisp base, melted top, four minutes.",
            ),
            (
                "How do I keep pasta from clumping after draining?",
                "Toss it with the sauce immediately, and save a cup of the starchy water.",
            ),
            (
                "Why does my risotto turn out gluey?",
                "Too much stirring at the end, and stock added faster than the rice absorbs.",
            ),
        ],
    },
    Topic {
        query: "How do I debug a Kubernetes pod stuck in CrashLoopBackOff?",
        turns: [
            (
                "My pod is in CrashLoopBackOff.",
                "Check `kubectl logs --previous` — the current container may be too young to log.",
            ),
            (
                "The liveness probe kills the pod during startup.",
                "Add a startupProbe so the liveness probe does not run until boot finishes.",
            ),
            (
                "OOMKilled shows in the pod events.",
                "Raise the memory limit, or find the allocation spike — limits are hard caps.",
            ),
        ],
    },
    Topic {
        query: "What is a sensible strategy for index funds?",
        turns: [
            (
                "Should I buy index funds or pick stocks?",
                "Broad index funds win on fees and diversification for almost everyone.",
            ),
            (
                "How often should I rebalance a portfolio?",
                "Once a year, or when an allocation drifts more than five points.",
            ),
            (
                "Are bond funds worth holding when rates rise?",
                "They cushion equity drawdowns; duration is the lever that matters.",
            ),
        ],
    },
    Topic {
        query: "How do I train for my first marathon?",
        turns: [
            (
                "How many weeks should marathon training take?",
                "Sixteen to twenty, building weekly mileage by no more than ten percent.",
            ),
            (
                "Should I run the full distance in training?",
                "No — the longest run is usually about twenty miles; race day carries the rest.",
            ),
            (
                "My knees hurt on long runs.",
                "Raise cadence toward 180 and check shoe wear before changing anything else.",
            ),
        ],
    },
    Topic {
        query: "How do I set up OAuth for a third-party API?",
        turns: [
            (
                "How does the OAuth authorization code flow work?",
                "The client swaps a short-lived code for tokens at the token endpoint.",
            ),
            (
                "Where should I store the refresh token?",
                "Server side and encrypted — never in browser storage.",
            ),
            (
                "The redirect URI keeps getting rejected.",
                "It must match the registered value exactly, including trailing slash.",
            ),
        ],
    },
];

/// The three turns of `gold` are what should be retrieved; every other topic
/// contributes its first turn as a distractor.
///
/// Gold turns go **first** so plain recency cannot reach them: with the medium
/// tier's five-message window, `lastk` sees only the tail, which is all
/// distractors by construction.
fn sample_turns(gold: usize) -> Vec<(&'static str, &'static str)> {
    let mut turns: Vec<(&'static str, &'static str)> = TOPICS[gold].turns.to_vec();
    for (i, t) in TOPICS.iter().enumerate() {
        if i != gold {
            turns.push(t.turns[0]);
        }
    }
    turns
}

/// The user-side text of each gold turn — what recall is measured against.
fn gold_texts(gold: usize) -> Vec<&'static str> {
    TOPICS[gold].turns.iter().map(|(u, _)| *u).collect()
}

async fn seed(pool: &PgPool, session_id: &str, user_id: Uuid, turns: &[(&str, &str)]) {
    sqlx::query("DELETE FROM chat_messages WHERE session_id = $1")
        .bind(session_id)
        .execute(pool)
        .await
        .expect("clear session");

    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, NULL, '/api/orchestrator/a2a', 'recall') \
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(session_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed session");

    // Explicit descending offsets: several inserts can otherwise land in the
    // same clock tick, and every fetch orders by `timestamp`.
    for (i, (user, assistant)) in turns.iter().enumerate() {
        for (role, content) in [("user", *user), ("assistant", *assistant)] {
            sqlx::query(
                "INSERT INTO chat_messages (session_id, role, content, timestamp) \
                 VALUES ($1, $2, $3, now() - make_interval(secs => $4))",
            )
            .bind(session_id)
            .bind(role)
            .bind(content)
            .bind((turns.len() * 2 - i * 2) as f64)
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

/// Mean recall across the ten samples, rounded to two decimals so the
/// assertion reads as a percentage rather than a float artefact.
async fn mean_recall(
    pool: &PgPool,
    user_id: Uuid,
    store: &VectorStore,
    strategy: &str,
    level: &str,
) -> f64 {
    set_prefs(pool, user_id, strategy, level).await;

    let mut total = 0.0;
    for (gold, topic) in TOPICS.iter().enumerate() {
        let session_id = format!("recall-sample-{gold}");
        seed(pool, &session_id, user_id, &sample_turns(gold)).await;

        let history = context_selection::fetch_for_user(
            pool,
            user_id,
            &session_id,
            store,
            topic.query,
            &ContextTiers::default(),
        )
        .await;

        let wanted = gold_texts(gold);
        let found = wanted
            .iter()
            .filter(|g| history.messages.iter().any(|m| m.content == **g))
            .count();
        total += found as f64 / wanted.len() as f64;

        sqlx::query("DELETE FROM chat_messages WHERE session_id = $1")
            .bind(&session_id)
            .execute(pool)
            .await
            .ok();
    }

    (total / TOPICS.len() as f64 * 100.0).round() / 100.0
}

#[tokio::test]
#[ignore = "needs DATABASE_URL + OPENAI_API_KEY"]
async fn recall_over_ten_samples() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
    let pool = PgPool::connect(&db_url).await.expect("connect");

    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users ORDER BY created_at LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("a user row must exist — log in once first");

    let store = VectorStore::for_embedding(
        api_key,
        "https://api.openai.com".to_string(),
        "text-embedding-3-small".to_string(),
        Arc::new(dashmap::DashMap::new()),
    );

    println!(
        "\n{} samples, {} gold turns each, {} turns per session\n",
        TOPICS.len(),
        TOPICS[0].turns.len(),
        sample_turns(0).len(),
    );
    println!(
        "{:<8} {:>8} {:>8} {:>8}",
        "strategy", "low", "medium", "high"
    );

    let mut got = Vec::new();
    for strategy in ["pacms", "topk", "lastk"] {
        let mut row = Vec::new();
        for level in ["low", "medium", "high"] {
            row.push(mean_recall(&pool, user_id, &store, strategy, level).await);
        }
        println!(
            "{:<8} {:>8.2} {:>8.2} {:>8.2}",
            strategy, row[0], row[1], row[2]
        );
        got.push((strategy, row));
    }

    // Measured values. Embeddings are deterministic for a fixed model, so
    // these are stable; if one moves, selection behaviour moved with it.
    let expected: [(&str, [f64; 3]); 3] = [
        (
            "pacms",
            [EXPECT_PACMS_LOW, EXPECT_PACMS_MED, EXPECT_PACMS_HIGH],
        ),
        ("topk", [EXPECT_TOPK_LOW, EXPECT_TOPK_MED, EXPECT_TOPK_HIGH]),
        (
            "lastk",
            [EXPECT_LASTK_LOW, EXPECT_LASTK_MED, EXPECT_LASTK_HIGH],
        ),
    ];

    for ((strategy, actual), (_, want)) in got.iter().zip(expected.iter()) {
        for (i, level) in ["low", "medium", "high"].iter().enumerate() {
            assert!(
                (actual[i] - want[i]).abs() < 1e-9,
                "{strategy}/{level} recall was {:.2}, expected {:.2}",
                actual[i],
                want[i],
            );
        }
    }
}

// Measured. Each number is explicable from the fixture, which is why they are
// pinned rather than bounded:
//
//   pacms  1.00 everywhere — even the low tier's 500-token budget is enough to
//          hold all three gold turns once relevance and coverage pick them out.
//   topk   0.30 at low: k=1 pair, so at most one of three gold turns can be
//          returned (ceiling 0.33), and nine of the ten samples ranked a gold
//          turn top-1 — 9/10 x 1/3 = 0.30. At medium (k=5) all three fit.
//   lastk  0.00 at low and medium: gold sits at the head of the session and
//          recency only ever sees the tail. At high (k=20 of 24 messages) the
//          window finally reaches back over one gold turn — 1/3 = 0.33.
//
// A change here is a change in selection behaviour, not flake.
const EXPECT_PACMS_LOW: f64 = 1.00;
const EXPECT_PACMS_MED: f64 = 1.00;
const EXPECT_PACMS_HIGH: f64 = 1.00;
const EXPECT_TOPK_LOW: f64 = 0.30;
const EXPECT_TOPK_MED: f64 = 1.00;
const EXPECT_TOPK_HIGH: f64 = 1.00;
const EXPECT_LASTK_LOW: f64 = 0.00;
const EXPECT_LASTK_MED: f64 = 0.00;
const EXPECT_LASTK_HIGH: f64 = 0.33;

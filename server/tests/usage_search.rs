//! `q` on GET /api/usage/by-model — the model-name search.
//!
//! The bug this file exists to prevent was a *contract* bug rather than a
//! crash: `/usage/by-model` accepted `q`, both frontends sent it, and the
//! data-source catalog advertised it to Weave as a searchable argument, but
//! the SQL never applied it. Every generated model search box typed into a
//! parameter the server threw away — the table never narrowed and nothing
//! errored. The route's own doc comment admitted it.
//!
//! A shape-only assertion would have passed throughout, so every test here
//! asserts on *which rows come back*, not on the envelope. Test 1 is the one
//! that fails without the predicate; the rest pin the semantics around it —
//! containment rather than equality or prefix, case-insensitivity matching
//! the `by-agent` sibling, pagination applied after the filter, and the
//! empty-`q` call the real frontends always make staying exactly as it was.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test usage_search -- --test-threads=1

mod common;

use common::TestServer;
use serde_json::Value;
use serial_test::serial;
use uuid::Uuid;

/// The corpus every test sees. Token totals are distinct and descending in
/// this order so `ORDER BY total_tokens DESC` fixes the page order without a
/// tiebreak, and `4o` appears mid-name in exactly one model so a containment
/// match is distinguishable from a prefix or an equality one.
const SEEDED: [(&str, &str, i32); 3] = [
    ("anthropic", "claude-sonnet-4-5", 900),
    ("anthropic", "claude-haiku-4-5", 600),
    ("openai", "gpt-4o-mini", 300),
];

/// One user owning one `trace_usage` row per seeded model. `started_at` is
/// `now()`, so every row falls inside the route's fixed 30-day look-back and
/// the window never enters into what these tests measure.
async fn seed(server: &TestServer) -> Uuid {
    let user_id: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind("usage-search")
            .bind("usage-search@test.local")
            .fetch_one(&server.db)
            .await
            .expect("seed user");

    for (provider, model, tokens) in SEEDED {
        sqlx::query(
            "INSERT INTO trace_usage \
             (trace_id, agent_name, user_id, provider, model, input_tokens, output_tokens, \
              cost_usd, latency_ms, started_at) \
             VALUES ($3, 'usage-search-agent', $1, $2, $3, $4, 0, 0.001, 100, now())",
        )
        .bind(user_id)
        .bind(provider)
        .bind(model)
        .bind(i64::from(tokens))
        .execute(&server.db)
        .await
        .expect("seed trace_usage");
    }

    user_id
}

/// The model names the route returns, in response order, plus the envelope's
/// `total`. `total` is returned rather than asserted here because it is the
/// page length and not a match count (`Paginated::new`), and one test exists
/// to pin exactly that.
async fn by_model(server: &TestServer, user_id: Uuid, query: &str) -> (Vec<String>, u64) {
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/usage/by-model{query}"))),
        &user_id.to_string(),
        "usage-search",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "GET /api/usage/by-model{query}");

    let body: Value = res.json().await.unwrap();
    let models = body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("no data array in {body}"))
        .iter()
        .map(|row| row["model"].as_str().expect("row.model").to_string())
        .collect();
    let total = body["total"].as_u64().expect("total");

    (models, total)
}

/// 1. The predicate exists at all. Without it this returns all three models.
#[tokio::test]
#[serial]
async fn q_narrows_by_model_instead_of_being_ignored() {
    let server = TestServer::start().await;
    let user_id = seed(&server).await;

    let (models, total) = by_model(&server, user_id, "?q=sonnet").await;
    assert_eq!(models, vec!["claude-sonnet-4-5"]);
    assert_eq!(total, 1);

    // A full name is just the degenerate substring, and must still work.
    let (models, _) = by_model(&server, user_id, "?q=claude-sonnet-4-5").await;
    assert_eq!(models, vec!["claude-sonnet-4-5"]);

    // Nothing matching is an empty 200, not a 404 and not an error.
    let (models, total) = by_model(&server, user_id, "?q=llama").await;
    assert!(models.is_empty(), "expected no rows, got {models:?}");
    assert_eq!(total, 0);

    server.cleanup().await;
}

/// 2. Containment, not equality and not a prefix. `4o` sits in the middle of
///    `gpt-4o-mini`, so a prefix implementation returns nothing here and an
///    equality one returns nothing for either term.
#[tokio::test]
#[serial]
async fn q_matches_a_substring_anywhere_in_the_model_name() {
    let server = TestServer::start().await;
    let user_id = seed(&server).await;

    let (models, _) = by_model(&server, user_id, "?q=4o").await;
    assert_eq!(models, vec!["gpt-4o-mini"]);

    let (models, _) = by_model(&server, user_id, "?q=haiku").await;
    assert_eq!(models, vec!["claude-haiku-4-5"]);

    server.cleanup().await;
}

/// 3. `ILIKE`, matching the `by-agent` convention this predicate was copied
///    from. Asserted as an equality between three spellings rather than
///    against a literal, so the test says "case does not matter" directly.
#[tokio::test]
#[serial]
async fn q_is_case_insensitive() {
    let server = TestServer::start().await;
    let user_id = seed(&server).await;

    let (lower, _) = by_model(&server, user_id, "?q=sonnet").await;
    let (upper, _) = by_model(&server, user_id, "?q=SONNET").await;
    let (mixed, _) = by_model(&server, user_id, "?q=Sonnet").await;

    assert_eq!(lower, vec!["claude-sonnet-4-5"]);
    assert_eq!(upper, lower);
    assert_eq!(mixed, lower);

    server.cleanup().await;
}

/// 4. `LIMIT`/`OFFSET` slice the *filtered* set, in `total_tokens DESC`
///    order, and `total` stays the page length throughout — it is not, and
///    must not become, a count of matches.
#[tokio::test]
#[serial]
async fn pagination_applies_after_the_filter() {
    let server = TestServer::start().await;
    let user_id = seed(&server).await;

    // `claude` matches two of the three: sonnet (900) then haiku (600).
    let (first, first_total) = by_model(&server, user_id, "?q=claude&limit=1&offset=0").await;
    let (second, second_total) = by_model(&server, user_id, "?q=claude&limit=1&offset=1").await;
    let (third, third_total) = by_model(&server, user_id, "?q=claude&limit=1&offset=2").await;

    assert_eq!(first, vec!["claude-sonnet-4-5"]);
    assert_eq!(second, vec!["claude-haiku-4-5"]);
    assert!(
        third.is_empty(),
        "expected an empty third page, got {third:?}"
    );

    assert_ne!(first, second, "pages must be disjoint");

    // `total` is `data.len()` on every page, the unfiltered model excluded.
    assert_eq!(first_total, first.len() as u64);
    assert_eq!(second_total, second.len() as u64);
    assert_eq!(third_total, third.len() as u64);
    assert_eq!(third_total, 0);

    server.cleanup().await;
}

/// 5. The call the real frontends actually make. Both serialize
///    `q: query || ''`, so `q` arrives as an empty string rather than absent;
///    that must keep matching everything, exactly as omitting it does. This
///    one passes before and after the fix — it is the guard that the change
///    stayed additive for existing callers, not evidence of the fix.
#[tokio::test]
#[serial]
async fn an_empty_q_matches_everything_just_like_no_q() {
    let server = TestServer::start().await;
    let user_id = seed(&server).await;

    let (empty, empty_total) = by_model(&server, user_id, "?q=").await;
    let (absent, absent_total) = by_model(&server, user_id, "").await;

    assert_eq!(empty, absent);
    assert_eq!(empty_total, absent_total);

    let mut seen = empty.clone();
    seen.sort();
    let mut expected: Vec<String> = SEEDED.iter().map(|(_, m, _)| (*m).to_string()).collect();
    expected.sort();
    assert_eq!(seen, expected, "every seeded model must still come back");

    server.cleanup().await;
}

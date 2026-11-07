//! Chat tiering end to end: messages of an idle session move to Lance, the
//! Postgres rows go, and history, paging, search and large (sidecar) bodies
//! still read back through the API and the event store.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test chat_tiering -- --test-threads=1
#![recursion_limit = "256"]

#[allow(dead_code)]
mod common;

use chrono::{DateTime, TimeDelta, Utc};
use nasiko_event_store::{EventStore, TieringConfig};
use serde_json::Value;
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

const SESSION: &str = "ses_tiering_test";

fn cold_dir() -> String {
    format!("/tmp/nasiko-cold-test-{}", Uuid::new_v4().simple())
}

fn store(db: &PgPool, cold_path: &str) -> EventStore {
    let cfg = TieringConfig {
        cold_path: cold_path.to_owned(),
        ..Default::default()
    };
    EventStore::new(db.clone(), cfg)
}

async fn seed_user(db: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash, role)
         VALUES ($1, $1 || '@test.local', 'x', 'member') RETURNING id",
    )
    .bind(format!("tier-{}", Uuid::new_v4()))
    .fetch_one(db)
    .await
    .unwrap()
}

async fn insert_msg(db: &PgPool, role: &str, content: &str, ts: DateTime<Utc>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO chat_messages (session_id, role, content, timestamp)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(SESSION)
    .bind(role)
    .bind(content)
    .bind(ts)
    .fetch_one(db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn idle_session_tiers_to_lance_and_still_reads_back() {
    let cold: String = cold_dir();
    let cold_cfg: String = cold.clone();
    let server = common::TestServer::start_with(move |c| c.cold_path = cold_cfg).await;
    let user: Uuid = seed_user(&server.db).await;
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 't')")
        .bind(SESSION)
        .bind(user)
        .execute(&server.db)
        .await
        .unwrap();

    // 6 visible messages + 1 hidden system row, all 40+ days old.
    let base: DateTime<Utc> = Utc::now() - TimeDelta::days(40);
    let big: String = "large body ".repeat(4000); // ~44 KB > 32 KB inline max
    let contents: Vec<String> = vec![
        "hello there".into(),
        "zebraquartz answer".into(),
        "internal continuation".into(), // system
        big.clone(),
        "fourth".into(),
        "fifth".into(),
        "sixth".into(),
    ];
    let mut expected: Vec<String> = Vec::new();
    for (i, c) in contents.iter().enumerate() {
        let role: &str = if i == 2 {
            "system"
        } else if i % 2 == 0 {
            "user"
        } else {
            "assistant"
        };
        insert_msg(&server.db, role, c, base + TimeDelta::seconds(i as i64)).await;
        if role != "system" {
            expected.push(c.clone());
        }
    }

    // TestServer's pool holds one connection; run_once needs a lock conn plus a work conn.
    let pool: PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&server.state.config.database_url)
        .await
        .unwrap();
    let report = store(&pool, &cold)
        .run_once(Utc::now())
        .await
        .unwrap()
        .expect("lock");
    assert!(report.rows >= 7, "chat rows tiered: {report:?}");

    let hot: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
        .bind(SESSION)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(hot, 0, "hot rows deleted");
    let cold_since: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT cold_since FROM chat_sessions WHERE session_id = $1")
            .bind(SESSION)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(cold_since.is_some(), "cold_since set");

    // Full history in one page.
    let url: String = server.url(&format!("/api/chat/sessions/{SESSION}/messages"));
    let resp = common::as_member(server.client.get(&url), &user.to_string(), "u")
        .query(&[("limit", "50")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let got: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        got, expected,
        "all non-system messages in order, large body intact"
    );

    // Cursor paging backwards, 2 at a time, reassembles the same conversation.
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut prev: Option<String> = None;
    loop {
        let mut q: Vec<(&str, String)> = vec![("limit", "2".into())];
        if let Some(p) = &prev {
            q.push(("prev_cursor", p.clone()));
        }
        let b: Value = common::as_member(server.client.get(&url), &user.to_string(), "u")
            .query(&q)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let rows: Vec<String> = b["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap().to_owned())
            .collect();
        if rows.is_empty() {
            break;
        }
        pages.push(rows);
        prev = b["prev_cursor"].as_str().map(str::to_owned);
        if !b["has_more"].as_bool().unwrap() || prev.is_none() {
            break;
        }
        assert!(pages.len() < 10, "paging terminates");
    }
    pages.reverse();
    let paged: Vec<String> = pages.concat();
    assert_eq!(
        paged, expected,
        "backward cursor paging covers everything once"
    );

    // Search finds cold inline and cold sidecar bodies.
    let s = store(&pool, &cold);
    let hits = s.search_messages(user, "zebraquartz", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].content, "zebraquartz answer");
    let big_hits = s.search_messages(user, "large body", 10).await.unwrap();
    assert_eq!(big_hits.len(), 1, "sidecar body searchable");
    assert_eq!(big_hits[0].content, big);

    let all = s.load_session(SESSION).await.unwrap();
    assert_eq!(all.len(), 7);

    pool.close().await;
    server.cleanup().await;
    let _ = std::fs::remove_dir_all(&cold);
}

/// Deleting a tiered chat removes its cold copies (messages and sidecar bodies), and a
/// session re-created under the same id never shows the deleted conversation.
#[tokio::test]
#[serial]
async fn deleting_a_tiered_chat_erases_its_cold_messages() {
    use nasiko_event_store::{chat_messages, cold};

    let cold_root: String = cold_dir();
    let cold_cfg: String = cold_root.clone();
    let server = common::TestServer::start_with(move |c| c.cold_path = cold_cfg).await;
    let user: Uuid = seed_user(&server.db).await;
    let create_session = || {
        sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 't')")
            .bind(SESSION)
            .bind(user)
            .execute(&server.db)
    };
    create_session().await.unwrap();
    let base: DateTime<Utc> = Utc::now() - TimeDelta::days(40);
    insert_msg(&server.db, "user", "deleted question", base).await;
    let big: String = "deleted large body ".repeat(3000); // sidecar body
    insert_msg(&server.db, "assistant", &big, base + TimeDelta::seconds(1)).await;

    let pool: PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&server.state.config.database_url)
        .await
        .unwrap();
    let es: EventStore = store(&pool, &cold_root);
    es.run_once(Utc::now()).await.unwrap().expect("lock");
    let in_session: String = format!("session_id = {}", cold::lit(SESSION));
    let main: String = cold::dataset_uri(&cold_root, chat_messages::TABLE);
    let bodies: String = cold::dataset_uri(&cold_root, chat_messages::BODIES);
    assert_eq!(cold::count(&main, &in_session).await.unwrap(), 2, "tiered");
    assert_eq!(
        cold::count(&bodies, &in_session).await.unwrap(),
        1,
        "sidecar body"
    );

    let url: String = server.url(&format!("/api/chat/sessions/{SESSION}"));
    let resp = common::as_member(server.client.delete(&url), &user.to_string(), "u")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // The route erases in the background; finish it here if that hasn't yet. The
    // TestServer pool has one connection, so that task can hold the tiering lock until
    // its second acquire times out (30 s).
    for _ in 0..400 {
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM session_erasure_requests WHERE session_id = $1",
        )
        .bind(SESSION)
        .fetch_one(&pool)
        .await
        .unwrap();
        if pending == 0 {
            break;
        }
        let _ = es.erase_session(SESSION).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        cold::count(&main, &in_session).await.unwrap(),
        0,
        "cold messages erased"
    );
    assert_eq!(
        cold::count(&bodies, &in_session).await.unwrap(),
        0,
        "cold bodies erased"
    );

    // Same id again, tiered later: only the new conversation reads back.
    create_session().await.unwrap();
    insert_msg(&server.db, "user", "fresh start", Utc::now()).await;
    sqlx::query("UPDATE chat_sessions SET cold_since = now() WHERE session_id = $1")
        .bind(SESSION)
        .execute(&pool)
        .await
        .unwrap();
    let rows = es.load_session(SESSION).await.unwrap();
    let contents: Vec<&str> = rows.iter().map(|r| r.content.as_str()).collect();
    assert_eq!(contents, vec!["fresh start"]);

    pool.close().await;
    server.cleanup().await;
    let _ = std::fs::remove_dir_all(&cold_root);
}

/// Fetch every page of `url` following one cursor direction, joined in reading order.
async fn walk(
    server: &common::TestServer,
    user: Uuid,
    url: &str,
    first: Vec<(&'static str, String)>,
    cursor: &'static str,
) -> Vec<String> {
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut q: Vec<(&str, String)> = first;
    loop {
        let b: Value = common::as_member(server.client.get(url), &user.to_string(), "u")
            .query(&q)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let rows: Vec<String> = b["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap().to_owned())
            .collect();
        if rows.is_empty() {
            break;
        }
        pages.push(rows);
        let next: Option<String> = b[cursor].as_str().map(str::to_owned);
        match next {
            Some(c) if b["has_more"].as_bool().unwrap() => {
                q = vec![("limit", "2".into()), (cursor, c)];
            }
            _ => break,
        }
        assert!(pages.len() < 20, "paging terminates");
    }
    if cursor == "prev_cursor" {
        pages.reverse();
    }
    pages.concat()
}

/// A tiered session that gained new hot messages pages across the hot/cold
/// boundary in both directions, each tier read one bounded page at a time, with
/// the cold sidecar body hydrated, and `get_session` keeps its oldest-first cap.
#[tokio::test]
#[serial]
async fn paging_crosses_the_hot_cold_boundary_both_ways() {
    let cold: String = cold_dir();
    let cold_cfg: String = cold.clone();
    let server = common::TestServer::start_with(move |c| c.cold_path = cold_cfg).await;
    let user: Uuid = seed_user(&server.db).await;
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 't')")
        .bind(SESSION)
        .bind(user)
        .execute(&server.db)
        .await
        .unwrap();

    let base: DateTime<Utc> = Utc::now() - TimeDelta::days(40);
    let big: String = "large body ".repeat(4000); // sidecar body
    let old: Vec<(&str, String)> = vec![
        ("user", "c1".into()),
        ("assistant", big.clone()),
        ("system", "hidden".into()),
        ("user", "c3".into()),
        ("assistant", "c4".into()),
    ];
    for (i, (role, c)) in old.iter().enumerate() {
        insert_msg(&server.db, role, c, base + TimeDelta::seconds(i as i64)).await;
    }
    let pool: PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&server.state.config.database_url)
        .await
        .unwrap();
    store(&pool, &cold)
        .run_once(Utc::now())
        .await
        .unwrap()
        .expect("lock");
    // New turns after the session went cold stay hot.
    let now: DateTime<Utc> = Utc::now();
    for (i, c) in ["h1", "h2", "h3"].iter().enumerate() {
        let role: &str = if i % 2 == 0 { "user" } else { "assistant" };
        insert_msg(&server.db, role, c, now + TimeDelta::seconds(i as i64)).await;
    }
    let hot: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
        .bind(SESSION)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(hot, 3, "only the new turns are hot");

    let expected: Vec<String> = vec![
        "c1".into(),
        big.clone(),
        "c3".into(),
        "c4".into(),
        "h1".into(),
        "h2".into(),
        "h3".into(),
    ];
    let url: String = server.url(&format!("/api/chat/sessions/{SESSION}/messages"));
    let backward: Vec<String> = walk(
        &server,
        user,
        &url,
        vec![("limit", "2".into())],
        "prev_cursor",
    )
    .await;
    assert_eq!(
        backward, expected,
        "newest-first paging covers both tiers once"
    );
    let from_start: String = (base - TimeDelta::days(1)).to_rfc3339();
    let forward: Vec<String> = walk(
        &server,
        user,
        &url,
        vec![("limit", "2".into()), ("after", from_start)],
        "next_cursor",
    )
    .await;
    assert_eq!(
        forward, expected,
        "oldest-first paging covers both tiers once"
    );

    // get_session: oldest rows first, system rows included, capped at `limit`.
    let detail: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/chat/sessions/{SESSION}"))),
        &user.to_string(),
        "u",
    )
    .query(&[("limit", "4")])
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let firsts: Vec<String> = detail["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        firsts,
        vec!["c1".to_string(), big, "hidden".into(), "c3".into()]
    );

    pool.close().await;
    server.cleanup().await;
    let _ = std::fs::remove_dir_all(&cold);
}

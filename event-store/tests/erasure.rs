#![recursion_limit = "256"]
//! User erasure end to end: hot + tiered (Lance) rows of one user go, another
//! user's rows stay, and a pending (lock-busy or failed) request is retried by
//! the tiering run.
//!
//!   cargo test -p nasiko-event-store --test erasure -- --test-threads=1

use chrono::Utc;
use nasiko_event_store::{EventStore, TieringConfig, cold};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

const TIERING_LOCK: i64 = 0x0074_6965_7269_6e67;

async fn fresh_db() -> PgPool {
    let admin: String = std::env::var("TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into());
    let name: String = format!("erasure_{}", Uuid::new_v4().simple());
    let mut c: PgConnection = PgConnection::connect(&admin).await.expect("postgres up?");
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&mut c)
        .await
        .unwrap();
    let base: &str = admin.rsplit_once('/').unwrap().0;
    let pool: PgPool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("{base}/{name}"))
        .await
        .unwrap();
    sqlx::migrate!("../migrations").run(&pool).await.unwrap();
    pool
}

async fn user(db: &PgPool) -> Uuid {
    let id: Uuid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(id.to_string())
        .bind(format!("{id}@t"))
        .execute(db)
        .await
        .unwrap();
    id
}

async fn trace(db: &PgPool, user: Uuid, days_ago: i32, source: &str) {
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, user_id, model, input_tokens,
             output_tokens, cost_usd, started_at, source)
         VALUES ($1, 'a', $2, 'm', 10, 5, 0.01, now() - make_interval(days => $3), $4)",
    )
    .bind(format!("t-{}", Uuid::new_v4()))
    .bind(user)
    .bind(days_ago)
    .bind(source)
    .execute(db)
    .await
    .unwrap();
}

/// Hot (recent) and cold-bound (old) rows in trace_usage.
async fn seed(db: &PgPool, u: Uuid) {
    trace(db, u, 100, "agent").await;
    trace(db, u, 100, "coding").await;
    trace(db, u, 1, "acp").await;
}

fn store(db: &PgPool, dir: &tempfile::TempDir) -> EventStore {
    let cfg = TieringConfig {
        cold_path: dir.path().to_str().unwrap().into(),
        batch_rows: 2,
        ..Default::default()
    };
    EventStore::new(db.clone(), cfg)
}

async fn n(db: &PgPool, sql: &str, u: Uuid) -> i64 {
    sqlx::query_scalar(sql).bind(u).fetch_one(db).await.unwrap()
}

/// (hot rows, rollup rows, cold rows) for `u` in trace_usage.
async fn footprint(db: &PgPool, dir: &tempfile::TempDir, u: Uuid) -> (i64, i64, usize) {
    let hot: i64 = n(db, "SELECT count(*) FROM trace_usage WHERE user_id = $1", u).await;
    let rollup: i64 = n(
        db,
        "SELECT count(*) FROM trace_usage_daily WHERE user_id = $1",
        u,
    )
    .await;
    let root: &str = dir.path().to_str().unwrap();
    let f: String = format!("user_id = '{u}'");
    let cold_rows: usize = cold::count(cold::dataset_uri(root, "trace_usage").as_str(), &f)
        .await
        .unwrap();
    (hot, rollup, cold_rows)
}

async fn pending(db: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM erasure_requests")
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn erases_hot_and_cold_and_keeps_other_users() {
    let db: PgPool = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (user(&db).await, user(&db).await);
    seed(&db, a).await;
    seed(&db, b).await;
    let s: EventStore = store(&db, &dir);
    s.run_once(Utc::now()).await.unwrap().expect("lock");
    let (hot, rollup, cold_a) = footprint(&db, &dir, a).await;
    assert_eq!((hot, cold_a), (1, 2), "seeded hot + tiered");
    assert!(rollup > 0);

    assert!(s.erase_user(a).await.unwrap());
    assert_eq!(footprint(&db, &dir, a).await, (0, 0, 0), "user a gone");
    let (hot_b, rollup_b, cold_b) = footprint(&db, &dir, b).await;
    assert_eq!((hot_b, cold_b), (1, 2), "user b intact");
    assert!(rollup_b > 0);
    assert_eq!(pending(&db).await, 0, "request cleared");
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_erasure_is_retried_by_tiering() {
    let db: PgPool = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (user(&db).await, user(&db).await, user(&db).await);
    for u in [a, b, c] {
        seed(&db, u).await;
    }
    let s: EventStore = store(&db, &dir);
    s.run_once(Utc::now()).await.unwrap().expect("lock");

    // Lock busy (another replica tiering): erase_user leaves the request pending.
    let mut holder: PgConnection = db.acquire().await.unwrap().detach();
    let _: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(TIERING_LOCK)
        .fetch_one(&mut holder)
        .await
        .unwrap();
    assert!(!s.erase_user(a).await.unwrap(), "lock busy -> pending");
    assert_eq!(pending(&db).await, 1);
    assert_eq!(
        footprint(&db, &dir, a).await.2,
        2,
        "cold untouched while pending"
    );
    holder.close().await.unwrap();

    // A previously failed attempt (as left by a cold-storage error).
    sqlx::query(
        "INSERT INTO erasure_requests (user_id, attempts, last_error, last_attempt_at)
         VALUES ($1, 1, 'cold erase trace_usage: boom', now())",
    )
    .bind(b)
    .execute(&db)
    .await
    .unwrap();
    assert_eq!(pending(&db).await, 2);

    let r = s.run_once(Utc::now()).await.unwrap().expect("lock");
    assert_eq!(r.erased, 2, "{r:?}");
    assert_eq!(pending(&db).await, 0);
    assert_eq!(footprint(&db, &dir, a).await, (0, 0, 0));
    assert_eq!(footprint(&db, &dir, b).await, (0, 0, 0));
    let (hot_c, _, cold_c) = footprint(&db, &dir, c).await;
    assert_eq!((hot_c, cold_c), (1, 2), "user c intact");
}

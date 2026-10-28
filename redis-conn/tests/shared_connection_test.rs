//! Needs a real Redis (`just infra`). `TEST_REDIS_URL` defaults to db 15, the same keyspace the
//! other integration tests use, so a developer's own server (db 0) is never touched.

use nasiko_redis_conn::connection;

fn client() -> redis::Client {
    let url =
        std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379/15".into());
    redis::Client::open(url).expect("redis client")
}

async fn client_id(conn: &mut impl redis::aio::ConnectionLike) -> i64 {
    redis::cmd("CLIENT")
        .arg("ID")
        .query_async(conn)
        .await
        .expect("CLIENT ID")
}

#[tokio::test]
async fn every_call_returns_the_same_underlying_connection() {
    let client = client();

    let mut ids = Vec::new();
    for _ in 0..20 {
        let mut conn = connection(&client).await.expect("shared connection");
        ids.push(client_id(&mut conn).await);
    }

    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "twenty calls must share one connection, saw ids {ids:?}"
    );

    // Control: the call this replaces really does open a new connection each time.
    let mut fresh = client.get_multiplexed_async_connection().await.unwrap();
    assert_ne!(client_id(&mut fresh).await, ids[0]);
}

#[tokio::test]
async fn concurrent_first_use_still_creates_one_connection() {
    let client = client();

    let ids = futures_ids(&client).await;

    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "concurrent callers must share one connection, saw ids {ids:?}"
    );
}

async fn futures_ids(client: &redis::Client) -> Vec<i64> {
    let tasks: Vec<_> = (0..32)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move {
                let mut conn = connection(&client).await.expect("shared connection");
                client_id(&mut conn).await
            })
        })
        .collect();
    let mut ids = Vec::new();
    for task in tasks {
        ids.push(task.await.unwrap());
    }
    ids
}

#[tokio::test]
async fn commands_work_through_the_shared_connection() {
    let client = client();
    let mut conn = connection(&client).await.expect("shared connection");

    let reply: String = redis::cmd("PING").query_async(&mut conn).await.unwrap();

    assert_eq!(reply, "PONG");
}

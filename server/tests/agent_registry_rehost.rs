mod common;

use nasiko_server::agents::rehost::rewrite_references;
use serial_test::serial;
use uuid::Uuid;

async fn owner(server: &common::TestServer) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(format!("rehost-{}", Uuid::new_v4().simple()))
        .bind(format!("rh-{}@test.local", Uuid::new_v4().simple()))
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn agent(server: &common::TestServer, owner: Uuid, image: &str, deleted: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, deleted_at) \
         VALUES ($1, $2, $3, 'running', CASE WHEN $4 THEN now() END) RETURNING id",
    )
    .bind(format!("a-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .bind(image)
    .bind(deleted)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn image(server: &common::TestServer, id: Uuid) -> String {
    sqlx::query_scalar("SELECT image FROM agents WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn queued(server: &common::TestServer, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM agent_registry_rehosts WHERE agent_id = $1)")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn references_on_a_former_host_move_to_the_current_one_and_live_agents_are_queued() {
    let server = common::TestServer::start().await;
    let owner = owner(&server).await;
    let old = "free.old.example";
    let new = "free.app.nasiko.com";
    let moved = agent(&server, owner, "free.old.example/u-abc/agent:1", false).await;
    let deleted = agent(&server, owner, "free.old.example/u-abc/gone:1", true).await;
    let lookalike = agent(&server, owner, "free.old.example.evil.io/u-abc/x:1", false).await;
    let external = agent(&server, owner, "docker.io/library/nginx:1", false).await;
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag) VALUES ($1, '1.0.0', $2)",
    )
    .bind(moved)
    .bind("free.old.example/u-abc/agent:1")
    .execute(&server.db)
    .await
    .unwrap();

    let first = rewrite_references(&server.db, new, &[old.to_string()])
        .await
        .unwrap();

    assert_eq!(first, 1, "only the live agent on the former host is queued");
    assert_eq!(
        image(&server, moved).await,
        "free.app.nasiko.com/u-abc/agent:1"
    );
    assert_eq!(
        image(&server, deleted).await,
        "free.app.nasiko.com/u-abc/gone:1"
    );
    assert_eq!(
        image(&server, lookalike).await,
        "free.old.example.evil.io/u-abc/x:1"
    );
    assert_eq!(image(&server, external).await, "docker.io/library/nginx:1");
    assert!(queued(&server, moved).await);
    assert!(!queued(&server, deleted).await);
    let version: String =
        sqlx::query_scalar("SELECT image_tag FROM agent_versions WHERE agent_id = $1")
            .bind(moved)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(version, "free.app.nasiko.com/u-abc/agent:1");

    let again = rewrite_references(&server.db, new, &[old.to_string()])
        .await
        .unwrap();
    assert_eq!(again, 0, "a second start finds nothing on the former host");
}

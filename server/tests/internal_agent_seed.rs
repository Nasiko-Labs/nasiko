//! NAS-1136: the internal-agent seeder (used for Weave) found its row by name
//! only. Names are unique per owner, so it adopted any user's agent with the
//! same name, or a soft-deleted one, and redeployed it with the platform image
//! and settings under that user's ownership.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test internal_agent_seed -- --test-threads=1
mod common;

use std::collections::HashMap;

use nasiko_server::seed::{InternalAgentSeed, seed_internal_agent};
use serial_test::serial;
use uuid::Uuid;

const NAME: &str = "dummy-internal-agent";

async fn seed_user(server: &common::TestServer, username: &str, superuser: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser, is_active) \
         VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@local.test"))
    .bind(superuser)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed(server: &common::TestServer) {
    seed_internal_agent(
        &server.state,
        InternalAgentSeed {
            name: NAME.to_string(),
            image: "nasiko/dummy-internal:1".to_string(),
            force_pull: false,
            env: HashMap::from([("DUMMY_SETTING".to_string(), "on".to_string())]),
            public: true,
            writable_path: None,
        },
    )
    .await;
}

/// (owner, image, is_internal, is_public, deleted)
async fn rows(server: &common::TestServer) -> Vec<(Uuid, Option<String>, bool, bool, bool)> {
    sqlx::query_as(
        "SELECT owner_id, image, is_internal, is_public, deleted_at IS NOT NULL \
         FROM agents WHERE name = $1 ORDER BY created_at",
    )
    .bind(NAME)
    .fetch_all(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn the_seed_does_not_adopt_a_users_agent_with_the_same_name() {
    let server = common::TestServer::start().await;
    let root = seed_user(&server, "dummy-root", true).await;
    let user = seed_user(&server, "dummy-user", false).await;
    let users_agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, is_public) \
         VALUES ($1, $2, 'user/own-image:1', 'running', false) RETURNING id",
    )
    .bind(NAME)
    .bind(user)
    .fetch_one(&server.db)
    .await
    .unwrap();

    seed(&server).await;

    let found = rows(&server).await;
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        found[0],
        (
            user,
            Some("user/own-image:1".to_string()),
            false,
            false,
            false
        ),
        "the user's agent was changed"
    );
    assert_eq!(
        found[1],
        (
            root,
            Some("nasiko/dummy-internal:1".to_string()),
            true,
            true,
            false
        )
    );

    let untouched_env: bool =
        sqlx::query_scalar("SELECT secrets_env = '{}'::jsonb FROM agents WHERE id = $1")
            .bind(users_agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(
        untouched_env,
        "platform settings were written to the user's agent"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_seed_reuses_its_own_row_but_not_a_deleted_one() {
    let server = common::TestServer::start().await;
    let root = seed_user(&server, "dummy-root", true).await;
    sqlx::query(
        "INSERT INTO agents (name, owner_id, image, status, is_internal, deleted_at) \
         VALUES ($1, $2, 'nasiko/dummy-internal:0', 'stopped', true, now())",
    )
    .bind(NAME)
    .bind(root)
    .execute(&server.db)
    .await
    .unwrap();

    seed(&server).await;
    seed(&server).await;

    let found = rows(&server).await;
    let live: Vec<_> = found.iter().filter(|r| !r.4).collect();
    assert_eq!(live.len(), 1, "{found:?}");
    assert!(live[0].2, "the live row is not internal");
    assert_eq!(
        found.len(),
        2,
        "the deleted row was revived or duplicated: {found:?}"
    );

    server.cleanup().await;
}

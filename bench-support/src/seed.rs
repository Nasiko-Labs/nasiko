//! Seeds users + agents for the in-process criterion harness.
//!
//! Adapted from `oss/server/src/bin/bench_seed.rs`'s `seed_users`/`seed_agents`,
//! with one addition: `bench_seed` only inserts DB rows, so a
//! `SimulatedRuntime` never learns about the seeded agent ids and every
//! proxied request falls back to the stored `agents.url` column instead of
//! the live `ContainerRuntime::endpoint()` lookup. Since this harness and the
//! server it benchmarks share one process, `seed()` also calls
//! `runtime.deploy()` per agent so the benchmark exercises the live-lookup
//! code path in `oss/server/src/agent_proxy.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use nasiko_auth::Identity;
use nasiko_auth::jwt::encode_jwt;
use nasiko_runtime::{ContainerId, ContainerRuntime, DeploymentSpec};
use sqlx::PgPool;
use uuid::Uuid;

pub struct ManifestUser {
    pub id: Uuid,
    pub username: String,
    pub token: String,
}

pub struct ManifestAgent {
    pub id: Uuid,
    pub name: String,
}

pub struct Manifest {
    pub users: Vec<ManifestUser>,
    pub agents: Vec<ManifestAgent>,
}

/// Seed `users` users and `agents` agents into `pool`, minting a bearer JWT
/// per user (bypassing bcrypt/login) and deploying each agent into `runtime`
/// so live endpoint resolution succeeds. `sim_agent_url` becomes each seeded
/// agent's stored `agents.url` fallback value.
pub async fn seed(
    pool: &PgPool,
    users: u32,
    agents: u32,
    sim_agent_url: &str,
    jwt_secret: &str,
    runtime: &Arc<dyn ContainerRuntime>,
) -> Manifest {
    assert!(
        users >= 1,
        "seed() requires at least one user (agents need an owner)"
    );

    let mut out_users = Vec::with_capacity(users as usize);
    for i in 0..users {
        let username = format!("bench_user_{i}");
        let email = format!("{username}@bench.local");

        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (username, email, role) VALUES ($1, $2, 'member') RETURNING id",
        )
        .bind(&username)
        .bind(&email)
        .fetch_one(pool)
        .await
        .expect("seed: insert user");

        let identity = Identity {
            user_id: user_id.to_string(),
            username: username.clone(),
            is_superuser: false,
        };
        let token = encode_jwt(jwt_secret, 24 * 60 * 60, &identity).expect("seed: mint jwt");

        out_users.push(ManifestUser {
            id: user_id,
            username,
            token,
        });
    }

    let owner_id = out_users[0].id;

    let mut out_agents = Vec::with_capacity(agents as usize);
    for i in 0..agents {
        let name = format!("bench-agent-{i}");

        let agent_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agents (name, owner_id, url, status, is_public)
             VALUES ($1, $2, $3, 'running', true) RETURNING id",
        )
        .bind(&name)
        .bind(owner_id)
        .bind(sim_agent_url)
        .fetch_one(pool)
        .await
        .expect("seed: insert agent");

        sqlx::query("INSERT INTO agent_grants (agent_id, grant_type, grantee_id) VALUES ($1, 'public', '*')")
            .bind(agent_id)
            .execute(pool)
            .await
            .expect("seed: insert public grant");

        let spec = DeploymentSpec {
            container_id: ContainerId::from_uuid(agent_id),
            name: name.clone(),
            image: "bench/sim-agent:latest".into(),
            min_replicas: 1,
            max_replicas: 1,
            env_vars: HashMap::new(),
            ports: vec![8000],
            resources: None,
            image_pull_secret_name: None,
            image_pull_credential_seed: None,
        };
        runtime
            .deploy(&spec)
            .await
            .expect("seed: deploy into SimulatedRuntime");

        out_agents.push(ManifestAgent { id: agent_id, name });
    }

    Manifest {
        users: out_users,
        agents: out_agents,
    }
}

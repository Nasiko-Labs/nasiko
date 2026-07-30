//! Load generator for the control-plane benchmark.
//!
//! Reads the manifest written by `bench_seed` (pre-signed JWTs + seeded
//! agent IDs) so no virtual user ever calls `/api/auth/login` in its hot
//! path, then drives four scenarios against a running `nasiko-server`
//! (paired with `SimulatedRuntime` + `simulated-agent` so responses are
//! real network round-trips without Docker/K8s/LLM cost):
//!
//! - `catalog_list`   — GET  /api/agents               (pure DB read)
//! - `agent_proxy`    — POST /api/agents/{id}          (direct proxy → simulated-agent)
//! - `orchestrator`   — POST /api/orchestrator/a2a     (full routing pipeline)
//! - `agent_crud`     — POST/GET/PUT/DELETE /api/agents[/{id}] (registry management)
//!
//! `agent_crud` creates and tears down its own throwaway agent per call — unlike
//! the other three transactions, it never touches the shared seeded agents (which
//! `catalog_list`/`agent_proxy_chat` depend on staying alive for the whole run),
//! and needs no pre-sized disposable pool the way `oss/server/benches/control_plane.rs`'s
//! `delete` benchmark does, since a live Goose run has no fixed iteration count to
//! size a pool against up front.
//!
//! Manifest path is read from `BENCH_MANIFEST` (default `bench-manifest.json`).
//! All other load parameters (users, run-time, host) are goose's own CLI
//! flags — see `--help`.
use std::sync::Arc;

use goose::prelude::*;
use nasiko_types::a2a::build_send_request;
use rand::Rng;
use rand::seq::IndexedRandom;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Clone)]
struct ManifestUser {
    token: String,
}

#[derive(Deserialize, Clone)]
struct ManifestAgent {
    id: String,
}

#[derive(Deserialize, Clone)]
struct Manifest {
    users: Vec<ManifestUser>,
    agents: Vec<ManifestAgent>,
}

/// Per-virtual-user session state: one bearer token, held for the whole run
/// so login/token-mint cost never appears in the hot path.
#[derive(Clone)]
struct Session {
    token: String,
    agents: Arc<Vec<ManifestAgent>>,
}

fn load_manifest() -> Manifest {
    let path = std::env::var("BENCH_MANIFEST").unwrap_or_else(|_| "bench-manifest.json".into());
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read manifest {path:?}: {e} — run bench_seed first"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("failed to parse manifest {path:?}: {e}"))
}

async fn setup_session(user: &mut GooseUser) -> TransactionResult {
    let manifest: Arc<Manifest> = user
        .get_session_data::<Arc<Manifest>>()
        .cloned()
        .unwrap_or_else(|| Arc::new(load_manifest()));

    let token = manifest
        .users
        .choose(&mut rand::rng())
        .map(|u| u.token.clone())
        .expect("manifest must have at least one user");

    user.set_session_data(Session {
        token,
        agents: Arc::new(manifest.agents.clone()),
    });
    Ok(())
}

fn auth_header<'a>(user: &'a GooseUser) -> &'a str {
    &user.get_session_data_unchecked::<Session>().token
}

async fn catalog_list(user: &mut GooseUser) -> TransactionResult {
    let token = auth_header(user).to_owned();
    let request_builder = user
        .get_request_builder(&GooseMethod::Get, "/api/agents")?
        .header("Authorization", format!("Bearer {token}"));
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;
    Ok(())
}

async fn agent_proxy_chat(user: &mut GooseUser) -> TransactionResult {
    let session = user.get_session_data_unchecked::<Session>().clone();
    let Some(agent) = session.agents.choose(&mut rand::rng()) else {
        return Ok(());
    };

    let body = build_send_request("What is the weather like today?", None);
    let path = format!("/api/agents/{}", agent.id);
    let request_builder = user
        .get_request_builder(&GooseMethod::Post, &path)?
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&body);
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;
    Ok(())
}

/// Exercises the agent-registry management API (as opposed to the chat/routing
/// paths the other three transactions cover): create a throwaway agent owned by
/// this virtual user, read it back, update it, then delete it — one full
/// create→get→update→delete cycle per call, entirely self-contained so it never
/// risks mutating the shared seeded agents the other transactions rely on.
async fn agent_crud(user: &mut GooseUser) -> TransactionResult {
    let token = auth_header(user).to_owned();
    let suffix: u64 = rand::rng().random();
    let name = format!("bench-crud-{suffix:x}");

    // CREATE
    let request_builder = user
        .get_request_builder(&GooseMethod::Post, "/api/agents")?
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": name }));
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .build();
    let created = user.request(goose_request).await?;
    let Ok(response) = created.response else {
        return Ok(());
    };
    let Ok(body) = response.json::<Value>().await else {
        return Ok(());
    };
    let Some(agent_id) = body.get("id").and_then(Value::as_str) else {
        // Create failed (e.g. name conflict) — nothing to read/update/delete.
        return Ok(());
    };

    // GET
    let request_builder = user
        .get_request_builder(&GooseMethod::Get, &format!("/api/agents/{agent_id}"))?
        .header("Authorization", format!("Bearer {token}"));
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;

    // UPDATE
    let request_builder = user
        .get_request_builder(&GooseMethod::Put, &format!("/api/agents/{agent_id}"))?
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({ "description": "bench crud update" }));
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;

    // DELETE
    let request_builder = user
        .get_request_builder(&GooseMethod::Delete, &format!("/api/agents/{agent_id}"))?
        .header("Authorization", format!("Bearer {token}"));
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;

    Ok(())
}

async fn orchestrator_a2a(user: &mut GooseUser) -> TransactionResult {
    let token = auth_header(user).to_owned();
    let body = build_send_request("Summarize the latest agent activity for me.", None);
    let request_builder = user
        .get_request_builder(&GooseMethod::Post, "/api/orchestrator/a2a")?
        .header("Authorization", format!("Bearer {token}"))
        .json(&body);
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    GooseAttack::initialize()?
        .register_scenario(
            scenario!("ControlPlaneBench")
                .register_transaction(transaction!(setup_session).set_on_start())
                .register_transaction(transaction!(catalog_list).set_weight(3)?)
                .register_transaction(transaction!(agent_proxy_chat).set_weight(5)?)
                .register_transaction(transaction!(orchestrator_a2a).set_weight(1)?)
                // Weighted low — CRUD is an occasional admin action, not
                // high-frequency chat/routing traffic (see module docs).
                .register_transaction(transaction!(agent_crud).set_weight(1)?),
        )
        .execute()
        .await?;
    Ok(())
}

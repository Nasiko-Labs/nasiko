//! Load generator for the control-plane benchmark.
//!
//! Reads the manifest written by `bench_seed` (pre-signed JWTs + seeded
//! agent IDs) so no virtual user ever calls `/api/auth/login` in its hot
//! path, then drives three scenarios against a running `nasiko-server`
//! (paired with `SimulatedRuntime` + `simulated-agent` so responses are
//! real network round-trips without Docker/K8s/LLM cost):
//!
//! - `catalog_list`   — GET  /api/catalog/agents      (pure DB read)
//! - `agent_proxy`    — POST /api/agents/{id}          (direct proxy → simulated-agent)
//! - `orchestrator`   — POST /api/orchestrator/a2a     (full routing pipeline)
//!
//! Manifest path is read from `BENCH_MANIFEST` (default `bench-manifest.json`).
//! All other load parameters (users, run-time, host) are goose's own CLI
//! flags — see `--help`.
use std::sync::Arc;

use goose::prelude::*;
use nasiko_types::a2a::build_send_request;
use rand::seq::IndexedRandom;
use serde::Deserialize;

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
        .get_request_builder(&GooseMethod::Get, "/api/catalog/agents")?
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
                .register_transaction(transaction!(orchestrator_a2a).set_weight(1)?),
        )
        .execute()
        .await?;
    Ok(())
}

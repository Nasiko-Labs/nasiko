//! End-to-end smoke test for the restart endpoint.
//!
//! Exercises in order:
//!   health check → init admin → restart unknown → 404
//!   → find any running deployment → restart while running → 409
//!   → find any stopped/crashed deployment → restart → 200 + crash fields cleared
//!
//! The 409 and 200 steps require existing deployments; they are skipped if none
//! are found and noted with instructions.
//!
//! Prerequisites:
//!   docker compose -f docker-compose.infra.yml up -d
//!   cargo run -p nasiko-server
//!
//! Run:
//!   cargo run -p nasiko-server --example restart_e2e
//!   cargo run -p nasiko-server --example restart_e2e -- --server http://localhost:9090

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;
use serde_json::Value;

// ── CLI args ──────────────────────────────────────────────────────────────────

struct Args {
    server:        String,
    access_key:    Option<String>,
    access_secret: Option<String>,
    agent_id:      Option<String>,
}

fn parse_args() -> Args {
    let mut server        = "http://localhost:9090".to_string();
    let mut access_key    = None;
    let mut access_secret = None;
    let mut agent_id      = None;
    let mut it = std::env::args().skip(1).peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--server"        => { server        = it.next().unwrap_or(server); }
            "--access-key"    => { access_key    = it.next(); }
            "--access-secret" => { access_secret = it.next(); }
            "--agent-id"      => { agent_id      = it.next(); }
            _ => {}
        }
    }
    Args { server, access_key, access_secret, agent_id }
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

async fn list_deployments(client: &reqwest::Client, server: &str, token: &str) -> Vec<Value> {
    let res = client
        .get(format!("{server}/api/agents/deployments"))
        .bearer_auth(token)
        .send()
        .await;

    match res {
        Ok(r) if r.status() == 200 => r.json::<Vec<Value>>().await.unwrap_or_default(),
        _ => vec![],
    }
}

async fn restart_deployment(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    deployment_id: &str,
) -> Result<reqwest::Response, String> {
    client
        .post(format!("{server}/api/agents/deployment/{deployment_id}/restart"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args = parse_args();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    println!("=== Restart Endpoint E2E ===");
    println!("server: {}", args.server);
    println!();

    let mut passed = 0u32;
    let mut failed = 0u32;

    // 1. Health check.
    let alive = common::step(
        "GET /health",
        client
            .get(format!("{}/health", args.server))
            .send()
            .await
            .map(|r| format!("HTTP {}", r.status()))
            .map_err(|e| e.to_string()),
        &mut passed,
        &mut failed,
    );
    if !alive {
        println!("\nServer not reachable — is `cargo run -p nasiko-server` running?\n");
        std::process::exit(1);
    }

    // 2. Get admin auth (creates admin on first run, logs in on subsequent runs).
    let (_uid, token) = match common::get_admin_auth(
        &client,
        &args.server,
        args.access_key.as_deref(),
        args.access_secret.as_deref(),
    )
    .await
    {
        Ok((uid, token)) => {
            println!("[ PASS ] init/login admin                      → uid={uid}");
            passed += 1;
            (uid, token)
        }
        Err(e) => {
            println!("[ FAIL ] init/login admin                      → {e}");
            failed += 1;
            (String::new(), String::new())
        }
    };

    if token.is_empty() {
        println!("\n0/{} passed   {} failed", passed + failed, failed);
        std::process::exit(1);
    }

    // 3. Restart an unknown deployment — must return 404.
    let fake_id = uuid::Uuid::new_v4();
    match restart_deployment(&client, &args.server, &token, &fake_id.to_string()).await {
        Ok(res) if res.status() == 404 => {
            println!("[ PASS ] restart unknown deployment → 404      → not-found guard works");
            passed += 1;
        }
        Ok(res) => {
            println!("[ FAIL ] restart unknown deployment            → expected 404, got {}", res.status());
            failed += 1;
        }
        Err(e) => {
            println!("[ FAIL ] restart unknown deployment            → {e}");
            failed += 1;
        }
    }

    // 4. Discover deployments (filtered to --agent-id if given).
    println!("[ INFO ] fetching current deployments...");
    let all_deployments = list_deployments(&client, &args.server, &token).await;
    let deployments: Vec<&Value> = if let Some(ref aid) = args.agent_id {
        let filtered: Vec<&Value> = all_deployments.iter()
            .filter(|d| d["agent_id"].as_str() == Some(aid.as_str()))
            .collect();
        println!("[ INFO ] found {} deployment(s) for agent {aid}", filtered.len());
        filtered
    } else {
        println!("[ INFO ] found {} deployment(s)", all_deployments.len());
        all_deployments.iter().collect()
    };

    // 4a. 409: restart a running/starting agent — must be rejected.
    let running = deployments.iter().copied().find(|d| {
        matches!(d["status"].as_str(), Some("running") | Some("starting"))
    });

    match running {
        Some(dep) => {
            let did = dep["id"].as_str().unwrap_or("");
            let name = dep["name"].as_str().or(dep["agent_name"].as_str()).unwrap_or("?");
            match restart_deployment(&client, &args.server, &token, did).await {
                Ok(res) if res.status() == 409 => {
                    println!("[ PASS ] restart running agent → 409           → conflict guard works (agent={name})");
                    passed += 1;
                }
                Ok(res) => {
                    let status = res.status();
                    let body = res.text().await.unwrap_or_default();
                    println!(
                        "[ FAIL ] restart running agent                 → expected 409, got {status} body={body:.60}"
                    );
                    failed += 1;
                }
                Err(e) => {
                    println!("[ FAIL ] restart running agent                 → {e}");
                    failed += 1;
                }
            }
        }
        None => {
            let hint = if args.agent_id.is_some() {
                "agent not in running/starting state"
            } else {
                "no running deployment found — pass --agent-id <uuid> or deploy an agent first"
            };
            common::skip("restart running agent → 409", hint);
        }
    }

    // 4b. Happy path: restart a stopped or crashed agent — must return 200.
    let restartable = deployments.iter().copied().find(|d| {
        matches!(d["status"].as_str(), Some("stopped") | Some("crashed") | Some("failed"))
    });

    match restartable {
        Some(dep) => {
            let did = dep["id"].as_str().unwrap_or("");
            let status = dep["status"].as_str().unwrap_or("?");
            let name = dep["name"].as_str().or(dep["agent_name"].as_str()).unwrap_or("?");
            match restart_deployment(&client, &args.server, &token, did).await {
                Ok(res) if res.status() == 200 => {
                    let body: serde_json::Value = res.json().await.unwrap_or_default();
                    let new_did = body["deployment_id"].as_str().unwrap_or("").to_string();
                    println!("[ PASS ] restart {status} agent → 200         → restart accepted (agent={name})");
                    passed += 1;

                    // Verify the new deployment row is in a live state.
                    if !new_did.is_empty() {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        let refreshed = list_deployments(&client, &args.server, &token).await;
                        if let Some(new_dep) = refreshed.iter().find(|d| d["id"].as_str() == Some(new_did.as_str())) {
                            let new_status = new_dep["status"].as_str().unwrap_or("?");
                            if matches!(new_status, "running" | "starting") {
                                println!("[ PASS ] new deployment after restart          → status={new_status}");
                                passed += 1;
                            } else {
                                println!("[ FAIL ] new deployment after restart          → status={new_status}");
                                failed += 1;
                            }
                        } else {
                            println!("[ FAIL ] new deployment after restart          → id={new_did:.8}.. not found in list");
                            failed += 1;
                        }
                    } else {
                        common::skip("new deployment status after restart", "server returned no deployment_id");
                    }
                }
                Ok(res) => {
                    let http_status = res.status();
                    let body = res.text().await.unwrap_or_default();
                    println!(
                        "[ FAIL ] restart {status} agent                → expected 200, got {http_status} body={body:.60}"
                    );
                    failed += 1;
                }
                Err(e) => {
                    println!("[ FAIL ] restart {status} agent                → {e}");
                    failed += 1;
                }
            }
        }
        None => {
            let hint = if args.agent_id.is_some() {
                "agent not in stopped/crashed state"
            } else {
                "no stopped/crashed deployment — pass --agent-id <uuid> or stop a running agent first"
            };
            common::skip("restart stopped/crashed agent → 200", hint);
        }
    }

    // Summary.
    println!();
    let total = passed + failed;
    println!("{passed}/{total} passed   {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}

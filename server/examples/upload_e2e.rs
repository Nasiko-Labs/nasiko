//! End-to-end smoke test for the upload-and-deploy pipeline.
//!
//! Exercises in order:
//!   health check → init admin → upload valid zip → agent upsert → zip guards
//!   (no Dockerfile, path traversal, 1000 files at-limit, 1001 files over-limit) → poll build status
//!
//! Prerequisites:
//!   docker compose -f docker-compose.infra.yml up -d
//!   cargo run -p nasiko-server
//!
//! Run:
//!   cargo run -p nasiko-server --example upload_e2e -- --server http://localhost:8080
//!
//! First run (no admin yet): credentials are printed — save them.
//! Subsequent runs: pass the saved credentials:
//!   cargo run -p nasiko-server --example upload_e2e -- \
//!     --server http://localhost:8080 \
//!     --access-key <key> --access-secret <secret>
//!
//!   --test-large   include the 101 MiB upload test (slow)
//!   --no-poll      skip polling build status to terminal

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

// ── CLI args ──────────────────────────────────────────────────────────────────

struct Args {
    server: String,
    access_key: Option<String>,
    access_secret: Option<String>,
    test_large: bool,
    no_poll: bool,
}

fn parse_args() -> Args {
    let mut server = "http://localhost:9090".to_string();
    let mut access_key = None;
    let mut access_secret = None;
    let mut test_large = false;
    let mut no_poll = false;
    let mut it = std::env::args().skip(1).peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--server" => {
                server = it.next().unwrap_or(server);
            }
            "--access-key" => {
                access_key = it.next();
            }
            "--access-secret" => {
                access_secret = it.next();
            }
            "--test-large" => {
                test_large = true;
            }
            "--no-poll" => {
                no_poll = true;
            }
            _ => {}
        }
    }
    Args {
        server,
        access_key,
        access_secret,
        test_large,
        no_poll,
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args = parse_args();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    println!("=== Upload Pipeline E2E ===");
    println!("server: {}", args.server);
    println!();

    let mut passed = 0u32;
    let mut failed = 0u32;

    // 1. Health check — bail early if server is not up.
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

    // 2. Get admin user_id + JWT token (creates admin on first run, logs in on
    //    subsequent runs). Every upload call below needs the actual bearer
    //    token, not just the user_id — get_admin_uid() discards the token and
    //    cannot be used here.
    let (_uid, token) = match common::get_admin_auth(
        &client,
        &args.server,
        args.access_key.as_deref(),
        args.access_secret.as_deref(),
    )
    .await
    {
        Ok((id, tok)) => {
            println!("[ PASS ] init/login admin                      → uid={id}");
            passed += 1;
            (id, tok)
        }
        Err(e) => {
            println!("[ FAIL ] init/login admin                      → {e}");
            failed += 1;
            (String::new(), String::new())
        }
    };

    if token.is_empty() {
        println!("\nCannot proceed without an admin user.\n");
        println!("0/{} passed   {} failed", passed + failed, failed);
        std::process::exit(1);
    }

    // 3. Upload a valid agent zip — expect 202 + queued build.
    let (first_agent_id, first_build_id) = {
        match common::upload(
            &client,
            &args.server,
            &token,
            "e2e-upload-agent",
            common::make_valid_zip(),
        )
        .await
        {
            Ok(res) if res.status() == 202 => {
                let body: serde_json::Value = res.json().await.unwrap_or_default();
                let aid = body["agent_id"].as_str().unwrap_or("").to_string();
                let bid = body["build_id"].as_str().unwrap_or("").to_string();
                let status = body["status"].as_str().unwrap_or("");
                println!(
                    "[ PASS ] upload valid zip                      → 202 status={status} agent={:.8}.. build={:.8}..",
                    aid, bid
                );
                passed += 1;
                (aid, bid)
            }
            Ok(res) => {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                println!("[ FAIL ] upload valid zip                      → {status}: {body:.120}");
                failed += 1;
                (String::new(), String::new())
            }
            Err(e) => {
                println!("[ FAIL ] upload valid zip                      → {e}");
                failed += 1;
                (String::new(), String::new())
            }
        }
    };

    // 4. Upload same name again — expect same agent_id (owner-scoped upsert).
    if !first_agent_id.is_empty() {
        match common::upload(
            &client,
            &args.server,
            &token,
            "e2e-upload-agent",
            common::make_valid_zip(),
        )
        .await
        {
            Ok(res) if res.status() == 202 => {
                let body: serde_json::Value = res.json().await.unwrap_or_default();
                let aid = body["agent_id"].as_str().unwrap_or("");
                let bid = body["build_id"].as_str().unwrap_or("");
                if aid == first_agent_id && bid != first_build_id {
                    println!(
                        "[ PASS ] upload same name → agent upsert       → same agent_id, new build_id"
                    );
                    passed += 1;
                } else if aid != first_agent_id {
                    println!(
                        "[ FAIL ] upload same name → agent upsert       → different agent_id: {aid:.8}.."
                    );
                    failed += 1;
                } else {
                    println!(
                        "[ FAIL ] upload same name → agent upsert       → build_id not unique"
                    );
                    failed += 1;
                }
            }
            Ok(res) => {
                println!(
                    "[ FAIL ] upload same name → agent upsert       → HTTP {}",
                    res.status()
                );
                failed += 1;
            }
            Err(e) => {
                println!("[ FAIL ] upload same name → agent upsert       → {e}");
                failed += 1;
            }
        }
    } else {
        common::skip("upload same name → agent upsert", "prior upload failed");
    }

    // 5. Upload without Dockerfile — expect 400 with "Dockerfile" in body.
    match common::upload(
        &client,
        &args.server,
        &token,
        "e2e-no-dockerfile",
        common::make_zip(&[("README.md", b"no dockerfile here")]),
    )
    .await
    {
        Ok(res) if res.status() == 400 => {
            let body = res.text().await.unwrap_or_default();
            if body.to_lowercase().contains("dockerfile") {
                println!("[ PASS ] upload no Dockerfile → 400            → {body:.80}");
                passed += 1;
            } else {
                println!(
                    "[ FAIL ] upload no Dockerfile → 400 but 'Dockerfile' missing in: {body:.80}"
                );
                failed += 1;
            }
        }
        Ok(res) => {
            println!(
                "[ FAIL ] upload no Dockerfile                  → expected 400, got {}",
                res.status()
            );
            failed += 1;
        }
        Err(e) => {
            println!("[ FAIL ] upload no Dockerfile                  → {e}");
            failed += 1;
        }
    }

    // 6. Upload with path traversal entry — expect 400.
    match common::upload(
        &client,
        &args.server,
        &token,
        "e2e-traversal",
        common::make_traversal_zip(),
    )
    .await
    {
        Ok(res) if res.status() == 400 => {
            println!("[ PASS ] upload path traversal → 400           → traversal guard fired");
            passed += 1;
        }
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            println!(
                "[ FAIL ] upload path traversal                 → expected 400, got {status} body={body:.80}"
            );
            failed += 1;
        }
        Err(e) => {
            println!("[ FAIL ] upload path traversal                 → {e}");
            failed += 1;
        }
    }

    // 7a. Upload exactly 1000 files — must be accepted (at the limit, not over).
    //     2 required files (Dockerfile + main.py) + 998 padding = 1000 total.
    //     archive.len() == 1000; guard fires on > 1000, so this passes.
    //     We verify body has real agent_id + build_id from the DB, not a static stub.
    println!("[ INFO ] building 1000-file zip (at limit)...");
    match common::upload(
        &client,
        &args.server,
        &token,
        "e2e-at-limit-files",
        common::make_many_files_zip(998),
    )
    .await
    {
        Ok(res) if res.status() == 202 => {
            let body: serde_json::Value = res.json().await.unwrap_or_default();
            let aid = body["agent_id"].as_str().unwrap_or("");
            let bid = body["build_id"].as_str().unwrap_or("");
            if !aid.is_empty() && !bid.is_empty() {
                println!(
                    "[ PASS ] upload 1000 files → 202               → agent={:.8}.. build={:.8}..",
                    aid, bid
                );
                passed += 1;
            } else {
                println!(
                    "[ FAIL ] upload 1000 files → 202 but body missing agent_id/build_id: {body}"
                );
                failed += 1;
            }
        }
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            println!(
                "[ FAIL ] upload 1000 files                     → expected 202, got {status}: {body:.80}"
            );
            failed += 1;
        }
        Err(e) => {
            println!("[ FAIL ] upload 1000 files                     → {e}");
            failed += 1;
        }
    }

    // 7b. Upload with 1001 files — expect 400 (one over MAX_ZIP_FILES = 1000).
    //     2 required files (Dockerfile + main.py) + 999 padding = 1001 total.
    println!("[ INFO ] building 1001-file zip (over limit)...");
    match common::upload(
        &client,
        &args.server,
        &token,
        "e2e-over-limit-files",
        common::make_many_files_zip(999),
    )
    .await
    {
        Ok(res) if res.status() == 400 => {
            let body = res.text().await.unwrap_or_default();
            println!("[ PASS ] upload 1001 files → 400               → {body:.80}");
            passed += 1;
        }
        Ok(res) => {
            println!(
                "[ FAIL ] upload 1001 files                     → expected 400, got {}",
                res.status()
            );
            failed += 1;
        }
        Err(e) => {
            println!("[ FAIL ] upload 1001 files                     → {e}");
            failed += 1;
        }
    }

    // 8. Upload > 100 MiB — expect 413. Opt-in because generating + transmitting 101 MiB is slow.
    if args.test_large {
        println!("[ INFO ] building 101 MiB zip (stored, no compression)...");
        let large = common::make_oversize_zip();
        let size_mib = large.len() / (1024 * 1024);
        let big_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap();
        match common::upload(&big_client, &args.server, &token, "e2e-oversize", large).await {
            Ok(res) if matches!(res.status().as_u16(), 400 | 413) => {
                println!(
                    "[ PASS ] upload {size_mib} MiB → {}              → size guard fired",
                    res.status()
                );
                passed += 1;
            }
            Ok(res) => {
                println!(
                    "[ FAIL ] upload {size_mib} MiB                      → expected 400 or 413, got {}",
                    res.status()
                );
                failed += 1;
            }
            Err(e) => {
                println!("[ FAIL ] upload {size_mib} MiB                      → {e}");
                failed += 1;
            }
        }
    } else {
        common::skip("upload >100 MiB → 413", "pass --test-large to enable");
    }

    // 9. Poll build status until terminal (the first valid upload).
    //    In dev the Docker build will fail (no real base image) — 'failed' is a valid terminal state.
    if !first_build_id.is_empty() && !args.no_poll {
        println!(
            "[ INFO ] polling build/{:.8}.. (up to 90s, Docker build expected to fail fast)...",
            first_build_id
        );
        match common::poll_build_status(&client, &args.server, &token, &first_build_id, 90).await {
            Ok(status) => {
                println!("[ PASS ] build reaches terminal state          → status={status}");
                passed += 1;
            }
            Err(e) => {
                println!("[ FAIL ] build reaches terminal state          → {e}");
                failed += 1;
            }
        }
    } else if !first_build_id.is_empty() {
        common::skip("poll build status", "--no-poll set");
    }

    // Summary.
    println!();
    let total = passed + failed;
    println!("{passed}/{total} passed   {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}

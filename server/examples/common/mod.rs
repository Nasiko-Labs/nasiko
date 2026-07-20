#![allow(dead_code)]

use std::io::Write;
use std::time::Duration;

// ── Output helpers ────────────────────────────────────────────────────────────

/// Print a PASS/FAIL line and increment the counter. Returns true on pass.
pub fn step(
    label: &str,
    result: Result<String, String>,
    passed: &mut u32,
    failed: &mut u32,
) -> bool {
    match result {
        Ok(msg) => {
            println!("[ PASS ] {label:<38} → {msg}");
            *passed += 1;
            true
        }
        Err(e) => {
            println!("[ FAIL ] {label:<38} → {e}");
            *failed += 1;
            false
        }
    }
}

pub fn skip(label: &str, reason: &str) {
    println!("[ SKIP ] {label:<38}   ({reason})");
}

// ── Zip builders ──────────────────────────────────────────────────────────────

/// Build an in-memory zip from `(path, bytes)` pairs.
pub fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }
    cursor.into_inner()
}

/// A structurally valid agent zip — passes all server-side validation.
/// Uses a nonexistent base image so Docker fails quickly in dev without pulling.
pub fn make_valid_zip() -> Vec<u8> {
    make_zip(&[
        (
            "Dockerfile",
            b"FROM python:3.11-slim\nCMD [\"python\", \"main.py\"]",
        ),
        ("main.py", b"print('hello from nasiko e2e')"),
    ])
}

/// Zip with a path-traversal entry (`../evil.txt`).
pub fn make_traversal_zip() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("Dockerfile", opts).unwrap();
        zw.write_all(b"FROM python:3.11-slim\nCMD [\"python\", \"main.py\"]")
            .unwrap();
        zw.start_file("main.py", opts).unwrap();
        zw.write_all(b"print('hi')").unwrap();
        zw.start_file("../evil.txt", opts).unwrap();
        zw.write_all(b"traversal payload").unwrap();
        zw.finish().unwrap();
    }
    cursor.into_inner()
}

/// Zip with `n` extra padding files on top of the required Dockerfile + main.py.
pub fn make_many_files_zip(extra: usize) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("Dockerfile", opts).unwrap();
        zw.write_all(b"FROM python:3.11-slim\nCMD [\"python\", \"main.py\"]")
            .unwrap();
        zw.start_file("main.py", opts).unwrap();
        zw.write_all(b"print('hi')").unwrap();
        for i in 0..extra {
            zw.start_file(format!("pad_{i:04}.txt"), opts).unwrap();
            zw.write_all(b"x").unwrap();
        }
        zw.finish().unwrap();
    }
    cursor.into_inner()
}

/// 101 MiB stored (no compression) — the zip file itself exceeds the 100 MiB upload limit.
///
/// Slow to generate (~1s) and to upload over loopback. Enable via `--test-large`.
pub fn make_oversize_zip() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("big.bin", opts).unwrap();
        let chunk = vec![0u8; 1024 * 1024]; // 1 MiB
        for _ in 0..101usize {
            zw.write_all(&chunk).unwrap();
        }
        zw.finish().unwrap();
    }
    cursor.into_inner()
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

/// Get the admin user_id and JWT token.
///
/// Priority:
/// 1. `initialize-admin` — creates admin on first run, prints credentials to save.
/// 2. `login` fallback — when admin already exists, uses access_key + access_secret.
///
/// Returns `(user_id, token)`.
pub async fn get_admin_auth(
    client: &reqwest::Client,
    server: &str,
    access_key: Option<&str>,
    access_secret: Option<&str>,
) -> Result<(String, String), String> {
    let res = client
        .post(format!("{server}/api/auth/initialize-admin"))
        .json(&serde_json::json!({"username": "admin", "email": "admin@e2e.local"}))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = res.status();
    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

    if status.is_success() {
        // First time — print credentials so the user can save them.
        if let (Some(key), Some(secret)) =
            (body["access_key"].as_str(), body["access_secret"].as_str())
        {
            println!("[ INFO ] Admin created. Save these credentials for future runs:");
            println!("[ INFO ]   --access-key {key}");
            println!("[ INFO ]   --access-secret {secret}");
        }
        let uid = body["user_id"]
            .as_str()
            .ok_or_else(|| format!("no user_id in initialize-admin response: {body}"))?
            .to_string();
        let token = body["token"]
            .as_str()
            .ok_or_else(|| format!("no token in initialize-admin response: {body}"))?
            .to_string();
        return Ok((uid, token));
    }

    // 409 = admin already exists — fall back to login.
    if status == 409 {
        let (key, secret) = match (access_key, access_secret) {
            (Some(k), Some(s)) => (k, s),
            _ => {
                return Err(
                    "admin already exists. Options:\n  --access-key <key> --access-secret <secret>   (from first-run output)"
                        .to_string(),
                );
            }
        };
        let login_res = client
            .post(format!("{server}/api/auth/login"))
            .json(&serde_json::json!({"access_key": key, "access_secret": secret}))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let login_body: serde_json::Value = login_res.json().await.map_err(|e| e.to_string())?;
        let uid = login_body["user_id"]
            .as_str()
            .ok_or_else(|| format!("login failed: {login_body}"))?
            .to_string();
        let token = login_body["token"]
            .as_str()
            .ok_or_else(|| format!("no token in login response: {login_body}"))?
            .to_string();
        return Ok((uid, token));
    }

    Err(format!("initialize-admin returned {status}: {body}"))
}

/// Backwards-compat alias that returns only the user_id.
pub async fn get_admin_uid(
    client: &reqwest::Client,
    server: &str,
    user_id: Option<&str>,
    access_key: Option<&str>,
    access_secret: Option<&str>,
) -> Result<String, String> {
    if let Some(uid) = user_id {
        return Ok(uid.to_string());
    }
    get_admin_auth(client, server, access_key, access_secret)
        .await
        .map(|(uid, _)| uid)
}

/// POST a multipart upload-and-deploy request.
pub async fn upload(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    name: &str,
    zip: Vec<u8>,
) -> Result<reqwest::Response, String> {
    let form = reqwest::multipart::Form::new()
        .text("name", name.to_string())
        .text("version_tag", "v1")
        .part(
            "source",
            reqwest::multipart::Part::bytes(zip).file_name("agent.zip"),
        );
    client
        .post(format!("{server}/api/agents/upload"))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())
}

/// Poll `GET /api/builds/{build_id}` until status is terminal or timeout.
pub async fn poll_build_status(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    build_id: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let res = client
            .get(format!("{server}/api/builds/{build_id}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if res.status() == 200 {
            let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
            if let Some(s) = body["status"].as_str()
                && (s == "success" || s == "failed")
            {
                return Ok(s.to_string());
            }
        }

        if std::time::Instant::now() >= deadline {
            return Err(format!("timed out after {timeout_secs}s"));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

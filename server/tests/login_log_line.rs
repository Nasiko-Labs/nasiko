//! The password login logs one line per attempt under `nasiko::auth`, and that line carries the
//! outcome, the reason and the account's id, but never the name that was typed (people type
//! passwords into that field), not even hashed, and never a password.
//!
//! Requires infra (Postgres, Redis, S3):
//!   cargo test -p nasiko-server --test login_log_line -- --test-threads=1
mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::as_superuser;
use serde_json::{Value, json};
use serial_test::serial;
use sha2::{Digest, Sha256};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    /// The `nasiko::auth` login lines written since the last `clear`.
    fn login_lines(&self) -> Vec<String> {
        self.text()
            .lines()
            .filter(|line| line.contains("nasiko::auth"))
            .map(str::to_owned)
            .collect()
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

async fn create_user(server: &common::TestServer, admin_id: &str, name: &str) -> Value {
    as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": name, "email": format!("{name}@test.local")}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn the_oss_log_line_never_holds_a_password_or_the_name_typed() {
    let logs = Captured::default();
    // Scoped to this thread, which also runs the server on a `#[tokio::test]` runtime.
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish(),
    );

    let server = common::TestServer::start().await;
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let admin_id = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, admin_id, "alice").await;
    let gone = create_user(&server, admin_id, "gone").await;
    let res = as_superuser(
        server
            .client
            .put(server.url(&format!("/api/users/{}", gone["id"].as_str().unwrap()))),
        admin_id,
        "admin",
    )
    .json(&json!({"is_active": false}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let alice_id = alice["id"].as_str().unwrap();
    let alice_key = alice["access_key"].as_str().unwrap();
    let alice_secret = alice["access_secret"].as_str().unwrap();
    let gone_id = gone["id"].as_str().unwrap();
    let gone_secret = gone["access_secret"].as_str().unwrap();

    let typed_name = "typed-name-5c1d0e";
    let typed_password = "typed-password-91ab7f";
    logs.clear();

    // Each attempt, signed in by the service the login route calls. A name that is an access key
    // is as secret as a password, so the account is reached by its key.
    let attempts: [(&str, &str); 7] = [
        (typed_name, typed_password),
        (alice_key, typed_password),
        (alice_key, alice_secret),
        ("gone", gone_secret),
        (alice_key, typed_password),
        (alice_key, typed_password),
        (alice_key, typed_password),
    ];
    for (name, password) in attempts {
        let _ = server.state.auth.authenticate(name, password).await;
    }

    let lines = logs.login_lines();
    assert_eq!(
        lines.len(),
        attempts.len(),
        "one line per attempt, not more and not fewer:\n{}",
        logs.text()
    );
    let has = |line: &String, parts: &[&str]| parts.iter().all(|part| line.contains(part));
    // The unknown name is flagged, and carries no account.
    assert!(has(
        &lines[0],
        &[
            "outcome=\"refused\"",
            "reason=\"unknown_name\"",
            "unknown_name=true"
        ]
    ));
    assert!(!lines[0].contains("user_id"));
    // A known account: its id, never `unknown_name`.
    assert!(has(
        &lines[1],
        &["reason=\"wrong_password\"", "unknown_name=false", alice_id]
    ));
    assert!(has(
        &lines[2],
        &["outcome=\"ok\"", "unknown_name=false", alice_id]
    ));
    assert!(has(&lines[3], &["reason=\"deactivated\"", gone_id]));
    // Three wrong passwords in a row (the right one in between reset the count): the third locks.
    assert!(has(&lines[4], &["reason=\"wrong_password\""]));
    assert!(has(&lines[5], &["reason=\"wrong_password\""]));
    assert!(has(&lines[6], &["reason=\"locked\"", alice_id]));

    // Nothing typed reaches the log: no password, no name, no hash of the name.
    let written = logs.text();
    for secret in [
        typed_name,
        typed_password,
        alice_key,
        alice_secret,
        gone_secret,
    ] {
        assert!(!written.contains(secret), "{secret} was logged:\n{written}");
    }
    for typed in [typed_name, alice_key] {
        let digest: String = Sha256::digest(typed.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert!(
            !written.contains(&digest),
            "a hash of the name {typed} was logged"
        );
    }

    server.cleanup().await;
}

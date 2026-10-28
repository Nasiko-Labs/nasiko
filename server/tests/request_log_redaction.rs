//! NAS-1135: the request span records the URI, and the SSO handoff carries a
//! session JWT in `?token=`. The token must not reach the logs. This drives the
//! real app (its `TraceLayer` included) and inspects what the log subscriber
//! actually wrote.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test request_log_redaction -- --test-threads=1
mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use serial_test::serial;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

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

#[tokio::test]
#[serial]
async fn the_sso_session_token_is_not_logged() {
    let logs = Captured::default();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(logs.clone())
        .try_init()
        .expect("this test owns the global subscriber");

    let server = common::TestServer::start().await;
    let secret = "eyJhbGciOiJIUzI1NiJ9.dummy-session-payload.dummy-signature";

    // An invalid token is rejected with a `warn!` inside the request span, so
    // the span's `uri` field is written next to it.
    server
        .client
        .get(server.url(&format!("/api/auth/sso/session?token={secret}")))
        .send()
        .await
        .unwrap();

    let written = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        written.contains("SSO session handoff rejected"),
        "the request was not logged at all, so the test proves nothing:\n{written}"
    );
    assert!(
        written.contains("/api/auth/sso/session?token=[REDACTED]"),
        "the span URI was not redacted:\n{written}"
    );
    assert!(
        !written.contains("dummy-session-payload"),
        "token leaked:\n{written}"
    );

    server.cleanup().await;
}

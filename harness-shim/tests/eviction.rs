//! SIGTERM handling of the real binary: eviction in handshake mode, a clean exit in env mode.

mod common;

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use common::*;
use serde_json::json;

/// The shim binary, killed on drop so a failed assertion leaves nothing behind.
struct Binary(Child);

impl Drop for Binary {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Binary {
    fn sigterm(&self) {
        self.signal(libc::SIGTERM);
    }

    fn signal(&self, signal: libc::c_int) {
        // SAFETY: plain signal delivery to the child process this test spawned.
        unsafe { libc::kill(self.0.id() as libc::pid_t, signal) };
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(started.elapsed() < DEADLINE, "the shim did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Starts the binary on an ephemeral port and returns once it logs where it listens. That line is
/// written after the socket is bound and the signal handlers are installed, so a signal sent
/// afterwards is always handled. The environment is cleared so a developer's `WORKSPACE_REPO`,
/// `HARNESS_SHIM_TOKEN` or `HTTPS_PROXY` cannot change what is under test.
async fn spawn(mode: &str, extra_env: &[(&str, &str)]) -> (Binary, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_harness-shim"))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("HARNESS_SHIM_MODE", mode)
        .env("HARNESS_SHIM_LISTEN", "127.0.0.1:0")
        .envs(extra_env.iter().copied())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let binary = Binary(child);
    let (tx, rx) = std::sync::mpsc::channel();
    // Keeps draining stderr after the address line so the shim never blocks on a full pipe.
    std::thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !sent && line.contains("harness shim listening") {
                sent = tx.send(listen_addr(&line)).is_ok();
            }
        }
    });
    let addr = tokio::task::spawn_blocking(move || rx.recv_timeout(DEADLINE))
        .await
        .unwrap()
        .expect("the shim never logged its address");
    (binary, addr)
}

/// The `127.0.0.1:<port>` in a log line (which may carry colour codes around it).
fn listen_addr(line: &str) -> SocketAddr {
    let start = line.find("127.0.0.1:").expect("an address in the line");
    let rest = &line[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ':'))
        .unwrap_or(rest.len());
    rest[..end].parse().unwrap()
}

async fn wait_evicting(addr: SocketAddr, bearer: Option<&str>) {
    let started = Instant::now();
    while status(addr, bearer).await["evicting"] != true {
        assert!(started.elapsed() < DEADLINE, "evicting never became true");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn sigterm_after_configure_only_flags_eviction_and_keeps_relaying() {
    let (mut binary, addr) = spawn("handshake", &[]).await;
    let body = configure_body("t1", json!({"HARNESS_CMD": "cat"}), "h1");
    assert_eq!(configure(addr, None, &body).await, 204);
    wait_for_child(addr, "t1", "running").await;

    binary.sigterm();
    wait_evicting(addr, Some("t1")).await;
    assert!(binary.0.try_wait().unwrap().is_none(), "the shim exited");
    let stats = http(addr, "GET", "/control/stats", Some("t1"), "").await.1;
    assert_eq!(stats["evicting"], true);
    // A client attaching during eviction is still served.
    assert_echoes(addr, "t1", r#"{"ping":1}"#).await;
}

#[tokio::test]
async fn sigterm_before_configure_flags_eviction_and_the_shim_still_configures() {
    let (mut binary, addr) = spawn("handshake", &[]).await;
    binary.sigterm();
    wait_evicting(addr, None).await;
    assert!(binary.0.try_wait().unwrap().is_none(), "the shim exited");

    let body = configure_body("t1", json!({"HARNESS_CMD": "cat"}), "h1");
    assert_eq!(configure(addr, None, &body).await, 204);
    wait_for_child(addr, "t1", "running").await;
    assert_echoes(addr, "t1", r#"{"ping":1}"#).await;
}

#[tokio::test]
async fn sigterm_in_env_mode_still_exits_cleanly() {
    let (mut binary, _addr) = spawn("env", &[("HARNESS_CMD", "cat")]).await;
    binary.sigterm();
    assert_eq!(binary.wait_exit().code(), Some(0));
}

#[tokio::test]
async fn sigint_still_exits_a_handshake_shim() {
    let (mut binary, _addr) = spawn("handshake", &[]).await;
    binary.signal(libc::SIGINT);
    assert_eq!(binary.wait_exit().code(), Some(0));
}

//! `DockerBatchRunner` against a live Docker daemon (ignored by default):
//!   cargo test -p nasiko-runtime --test docker_batch -- --ignored
#![cfg(feature = "docker")]

use std::time::Duration;

use nasiko_runtime::{
    BatchFile, BatchJob, BatchLimits, BatchNetwork, BatchRunner, BatchStop, DockerBatchConfig,
    DockerBatchRunner, DockerRuntime, DockerRuntimeConfig,
};

async fn runner() -> DockerBatchRunner {
    let runtime = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("docker daemon");
    DockerBatchRunner::new(runtime, DockerBatchConfig::default())
}

fn job(name: &str, script: &str, wall: Duration) -> BatchJob {
    BatchJob {
        name: name.into(),
        image: "alpine:3.21".into(),
        command: vec!["sh".into(), "-c".into(), script.into()],
        env: vec![("GREETING".into(), "hello".into())],
        inputs: vec![BatchFile {
            path: "task/prompt.md".into(),
            contents: b"fix it".to_vec(),
        }],
        collect_outputs: true,
        network: BatchNetwork::None,
        limits: BatchLimits {
            wall,
            cpus: 1.0,
            memory_mib: 256,
            pids: 128,
            disk_mib: 512,
        },
        label: name.into(),
        before_stop: Some("echo saved > /eval/out/saved.txt".into()),
    }
}

#[tokio::test]
#[ignore = "needs a Docker daemon"]
async fn a_job_reads_its_inputs_writes_outputs_and_has_no_network() {
    let r = runner().await;
    r.build_image(&[], "alpine:3.21").await.ok();
    let script = "cat /eval/in/task/prompt.md > /eval/out/copy.txt; \
                  echo \"$GREETING\" > /eval/out/env.txt; \
                  (wget -q -T 3 -O- http://example.com >/dev/null 2>&1 && echo online || echo offline) > /eval/out/net.txt; \
                  echo done; exit 3";
    let outcome = r
        .run(job("batch-test-io", script, Duration::from_secs(60)))
        .await
        .expect("runs");
    assert_eq!(outcome.exit_code, Some(3));
    assert_eq!(outcome.stopped, None);
    assert!(outcome.logs.contains("done"), "{}", outcome.logs);
    assert_eq!(outcome.output_text("copy.txt").as_deref(), Some("fix it"));
    assert_eq!(outcome.output_text("env.txt").as_deref(), Some("hello\n"));
    assert_eq!(outcome.output_text("net.txt").as_deref(), Some("offline\n"));
}

/// A one-route HTTP server on the host standing in for the model router.
async fn fake_router() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let body = if request.starts_with("GET /v1/models") {
                    "router-ok"
                } else {
                    "wrong-path"
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    port
}

#[tokio::test]
#[ignore = "needs a Docker daemon (Docker Desktop: host.docker.internal)"]
async fn a_router_only_job_reaches_the_router_and_nothing_else() {
    let port = fake_router().await;
    let runtime = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("docker daemon");
    let r = DockerBatchRunner::new(
        runtime,
        DockerBatchConfig {
            router_upstream: Some(format!("http://host.docker.internal:{port}")),
            ..DockerBatchConfig::default()
        },
    );
    let script = "sleep 1; wget -q -T 5 -O /eval/out/router.txt \"$NASIKO_ROUTER_URL/models\"; \
                  (wget -q -T 3 -O- http://example.com >/dev/null 2>&1 && echo online || echo offline) > /eval/out/net.txt; \
                  (wget -q -T 3 -O- \"${NASIKO_ROUTER_URL%/v1}/api/agents\" >/dev/null 2>&1 && echo open || echo refused) > /eval/out/other.txt";
    let mut j = job("batch-test-router", script, Duration::from_secs(60));
    j.network = BatchNetwork::RouterOnly;
    let outcome = r.run(j).await.expect("runs");
    assert_eq!(outcome.exit_code, Some(0), "{}", outcome.logs);
    assert_eq!(
        outcome.output_text("router.txt").as_deref(),
        Some("router-ok")
    );
    assert_eq!(outcome.output_text("net.txt").as_deref(), Some("offline\n"));
    assert_eq!(
        outcome.output_text("other.txt").as_deref(),
        Some("refused\n")
    );
    assert_eq!(
        r.cleanup(Some("batch-test-router")).await.expect("cleans"),
        0
    );
}

#[tokio::test]
#[ignore = "needs a Docker daemon"]
async fn a_job_past_its_wall_clock_is_stopped_and_keeps_its_work() {
    let r = runner().await;
    let outcome = r
        .run(job("batch-test-slow", "sleep 600", Duration::from_secs(3)))
        .await
        .expect("runs");
    assert_eq!(outcome.stopped, Some(BatchStop::TimeLimit));
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.output_text("saved.txt").as_deref(), Some("saved\n"));
    assert_eq!(r.cleanup(Some("batch-test-slow")).await.expect("cleans"), 0);
}

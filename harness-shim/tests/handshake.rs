//! In-process tests of handshake mode: the shim serves first and is configured over HTTP.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::*;
use harness_shim::handshake::Mode;
use harness_shim::server::{Config, run};
use serde_json::json;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

struct Shim {
    addr: SocketAddr,
    done: JoinHandle<anyhow::Result<i32>>,
}

async fn start_in(mode: Mode, mount: std::path::PathBuf) -> Shim {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        buffer_lines: 100,
        argv: (mode == Mode::Env).then(|| vec!["cat".to_owned()]),
        token: None,
        workspace: None,
        workspace_mount: mount,
        files: vec![],
        mode,
    };
    let (tx, rx) = oneshot::channel();
    let done = tokio::spawn(run(config, Some(tx)));
    Shim {
        addr: rx.await.unwrap(),
        done,
    }
}

async fn start(mode: Mode) -> SocketAddr {
    start_in(mode, "/workspace".into()).await.addr
}

/// Writes `body` as an executable-by-`sh` script (a configured command is whitespace-split, so a
/// script file stands in for a quoted `sh -c`) and returns the command that runs it.
fn script(dir: &tempfile::TempDir, name: &str, body: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    format!("sh {}", path.display())
}

#[tokio::test]
async fn handshake_shim_is_healthy_before_configure_and_spawns_after() {
    let addr = start(Mode::Handshake).await;

    let (code, status) = http(addr, "GET", "/control/status", None, "").await;
    assert_eq!(code, 200);
    assert_eq!(status["mode"], "handshake");
    assert_eq!(status["configured"], false);
    assert_eq!(status["child"], "waiting");
    let (code, health) = http(addr, "GET", "/healthz", None, "").await;
    assert_eq!(code, 200);
    assert_eq!(health["child"], "waiting");

    let body = configure_body("t1", json!({"HARNESS_CMD": "cat", "X": "1"}), "h1");
    assert_eq!(configure(addr, None, &body).await, 204);

    let status = wait_for_child(addr, "t1", "running").await;
    assert_eq!(status["configured"], true);
    assert_eq!(status["env_hash"], "h1");
    assert_eq!(http(addr, "GET", "/control/status", None, "").await.0, 401);
    assert_eq!(configure(addr, None, &body).await, 401);
    assert_eq!(configure(addr, Some("t1"), &body).await, 204);
}

#[tokio::test]
async fn configure_rejects_bad_bodies() {
    let addr = start(Mode::Handshake).await;
    assert_eq!(configure(addr, None, "not json").await, 400);
    let no_command = configure_body("t", json!({"X": "1"}), "h");
    assert_eq!(configure(addr, None, &no_command).await, 400);
    // Rejected calls claim no bearer.
    let (_, status) = http(addr, "GET", "/control/status", None, "").await;
    assert_eq!(status["configured"], false);
}

#[tokio::test]
async fn reconfigure_with_a_new_hash_takes_effect_on_restart() {
    let addr = start(Mode::Handshake).await;
    // The command is whitespace-split, so a script file stands in for a quoted `sh -c`.
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("harness.sh");
    std::fs::write(&script, "echo x=$X\ncat\n").unwrap();
    let cmd = format!("sh {}", script.display());
    let cmd = cmd.as_str();
    let first = configure_body("t1", json!({"HARNESS_CMD": cmd, "X": "1"}), "h1");
    assert_eq!(configure(addr, None, &first).await, 204);
    wait_for_child(addr, "t1", "running").await;
    assert_eq!(first_line(addr, "t1").await.as_deref(), Some("x=1"));

    let second = configure_body("t1", json!({"HARNESS_CMD": cmd, "X": "2"}), "h2");
    assert_eq!(configure(addr, Some("t1"), &second).await, 204);
    let before = status(addr, Some("t1")).await;
    assert_eq!(before["env_hash"], "h2");
    // The running child is left alone until the restart: it still runs the old configuration.
    assert_eq!(before["running_env_hash"], "h1");
    assert_eq!(before["child"], "running");
    let generation = before["child_generation"].as_u64().unwrap();

    // The restart answers only once the new child is up, so an immediate `/acp` sees it.
    let (code, body) = http(addr, "POST", "/control/restart", Some("t1"), "").await;
    assert_eq!(code, 200);
    assert_eq!(body["child_generation"], generation + 1);
    let after = status(addr, Some("t1")).await;
    assert_eq!(after["running_env_hash"], "h2");
    assert_eq!(after["child_generation"], generation + 1);
    assert_eq!(first_line(addr, "t1").await.as_deref(), Some("x=2"));
}

#[tokio::test]
async fn stats_report_processes_and_cpu() {
    let addr = start(Mode::Handshake).await;
    let body = configure_body("t1", json!({"HARNESS_CMD": "sleep 30"}), "h1");
    assert_eq!(configure(addr, None, &body).await, 204);
    wait_for_child(addr, "t1", "running").await;

    let (code, first) = http(addr, "GET", "/control/stats", Some("t1"), "").await;
    assert_eq!(code, 200);
    assert_eq!(first["cpu_millicores"], 0);
    assert_eq!(first["evicting"], false);
    assert!(first["sampled_at"].is_string());
    let (_, second) = http(addr, "GET", "/control/stats", Some("t1"), "").await;
    assert!(second["memory_bytes"].is_u64());
    if cfg!(target_os = "linux") {
        assert!(second["processes"].as_u64().unwrap() >= 1);
        assert!(second["memory_bytes"].as_u64().unwrap() > 0);
    } else {
        // Unknown is `null`, never 0.
        assert!(second["processes"].is_null());
    }
}

#[tokio::test]
async fn env_mode_refuses_configure() {
    let addr = start(Mode::Env).await;
    let body = configure_body("t1", json!({"HARNESS_CMD": "cat"}), "h1");
    assert_eq!(configure(addr, None, &body).await, 409);
    let (_, status) = http(addr, "GET", "/control/status", None, "").await;
    assert_eq!(status["mode"], "env");
}

#[tokio::test]
async fn secret_keys_in_the_configure_env_never_reach_the_child() {
    let addr = start(Mode::Handshake).await;
    let dir = tempfile::tempdir().unwrap();
    let cmd = script(
        &dir,
        "harness.sh",
        "echo \"t=${HARNESS_SHIM_TOKEN-unset} w=${WORKSPACE_REPO_TOKEN-unset} x=$X\"\ncat\n",
    );
    let env = json!({
        "HARNESS_CMD": cmd,
        "HARNESS_SHIM_TOKEN": "leak-1",
        "WORKSPACE_REPO_TOKEN": "leak-2",
        "X": "1",
    });
    assert_eq!(
        configure(addr, None, &configure_body("t1", env, "h1")).await,
        204
    );
    wait_for_child(addr, "t1", "running").await;
    assert_eq!(
        first_line(addr, "t1").await.as_deref(),
        Some("t=unset w=unset x=1")
    );
}

#[tokio::test]
async fn two_first_configures_cannot_both_claim_the_bearer() {
    let addr = start(Mode::Handshake).await;
    let a = configure_body("tok-a", json!({"HARNESS_CMD": "cat"}), "h1");
    let b = configure_body("tok-b", json!({"HARNESS_CMD": "cat"}), "h2");
    let (first, second) = tokio::join!(configure(addr, None, &a), configure(addr, None, &b));
    let mut codes = [first, second];
    codes.sort_unstable();
    assert_eq!(codes, [204, 401]);
}

#[tokio::test]
async fn a_configure_with_another_token_is_accepted_but_does_not_rotate_the_bearer() {
    let addr = start(Mode::Handshake).await;
    let first = configure_body("t1", json!({"HARNESS_CMD": "cat"}), "h1");
    assert_eq!(configure(addr, None, &first).await, 204);
    let rotated = configure_body("t2", json!({"HARNESS_CMD": "cat"}), "h2");
    assert_eq!(configure(addr, Some("t1"), &rotated).await, 204);
    assert_eq!(status(addr, Some("t1")).await["env_hash"], "h2");
    assert_eq!(
        http(addr, "GET", "/control/status", Some("t2"), "").await.0,
        401
    );
}

#[tokio::test]
async fn a_workspace_block_is_cloned_before_the_first_spawn_and_a_failure_is_announced() {
    let mount = tempfile::tempdir().unwrap();
    let shim = start_in(Mode::Handshake, mount.path().to_owned()).await;
    let addr = shim.addr;
    let body = json!({
        "token": "t1",
        // `file` is not an allowed clone protocol outside the shim's own unit tests.
        "env": {"HARNESS_CMD": "cat"},
        "workspace": {"repo": "file:///no/such/repo", "token": null, "timeout_secs": 30},
        "env_hash": "h1",
    })
    .to_string();
    assert_eq!(configure(addr, None, &body).await, 204);
    wait_for_child(addr, "t1", "running").await;

    let mut client = connect_acp(addr, "t1").await;
    let prompt = r#"{"jsonrpc":"2.0","id":7,"method":"session/prompt","params":{"sessionId":"s-1","prompt":[]}}"#;
    futures::SinkExt::send(&mut client, prompt.into())
        .await
        .unwrap();
    loop {
        let frame = next_text(&mut client).await.expect("socket stayed open");
        if frame.contains("clone failed") {
            assert!(frame.contains(r#""sessionId":"s-1""#), "{frame}");
            break;
        }
    }
}

#[tokio::test]
async fn a_reconfigure_during_a_long_clone_is_honoured_by_the_first_spawn() {
    // A TLS "server" that accepts and never answers: the clone hangs until its timeout.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_addr = silent.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            held.push(stream);
        }
    });
    let mount = tempfile::tempdir().unwrap();
    let shim = start_in(Mode::Handshake, mount.path().to_owned()).await;
    let addr = shim.addr;
    let dir = tempfile::tempdir().unwrap();
    let cmd = script(&dir, "harness.sh", "echo x=$X\ncat\n");
    let configure_with = |x: &str, hash: &str| {
        json!({
            "token": "t1",
            "env": {"HARNESS_CMD": cmd, "X": x},
            "workspace": {
                "repo": format!("https://{silent_addr}/repo.git"),
                "token": null,
                "timeout_secs": 6,
            },
            "env_hash": hash,
        })
        .to_string()
    };
    assert_eq!(configure(addr, None, &configure_with("1", "h1")).await, 204);
    // The clone is hanging: no child yet.
    assert_eq!(status(addr, Some("t1")).await["child"], "waiting");
    assert_eq!(
        configure(addr, Some("t1"), &configure_with("2", "h2")).await,
        204
    );

    // The reconfigure landed while the clone was still hanging; no timing arithmetic needed.
    assert_eq!(status(addr, Some("t1")).await["child"], "waiting");
    wait_for_child(addr, "t1", "running").await;
    assert_eq!(first_line(addr, "t1").await.as_deref(), Some("x=2"));
}

#[tokio::test]
async fn a_child_that_exits_leaves_the_shim_serving_until_the_next_configure() {
    let shim = start(Mode::Handshake).await;
    let addr = shim;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    // The first run exits at once; later runs stay up.
    let cmd = script(
        &dir,
        "harness.sh",
        &format!(
            "if [ -f {m} ]; then echo again; cat; else touch {m}; fi\n",
            m = marker.display()
        ),
    );
    let body = configure_body("t1", json!({"HARNESS_CMD": cmd}), "h1");
    assert_eq!(configure(addr, None, &body).await, 204);
    wait_for_child(addr, "t1", "exited").await;
    // The actor is alive: the probe stays 200 even though the child is gone.
    let (code, health) = http(addr, "GET", "/healthz", None, "").await;
    assert_eq!((code, &health["child"]), (200, &json!("exited")));

    let gone = status(addr, Some("t1")).await;
    assert!(gone["running_env_hash"].is_null());
    // The same hash still respawns a child that is gone, and that counts as a new spawn.
    assert_eq!(configure(addr, Some("t1"), &body).await, 204);
    let up = wait_for_child(addr, "t1", "running").await;
    assert_eq!(up["running_env_hash"], "h1");
    assert_eq!(
        up["child_generation"].as_u64().unwrap(),
        gone["child_generation"].as_u64().unwrap() + 1
    );
    assert_eq!(first_line(addr, "t1").await.as_deref(), Some("again"));
}

#[tokio::test]
async fn a_failed_spawn_does_not_exit_the_shim() {
    let shim = start_in(Mode::Handshake, "/workspace".into()).await;
    let addr = shim.addr;
    let bad = configure_body("t1", json!({"HARNESS_CMD": "/no/such/binary"}), "h1");
    assert_eq!(configure(addr, None, &bad).await, 204);
    wait_for_child(addr, "t1", "exited").await;
    assert!(!shim.done.is_finished());

    let good = configure_body("t1", json!({"HARNESS_CMD": "cat"}), "h2");
    assert_eq!(configure(addr, Some("t1"), &good).await, 204);
    wait_for_child(addr, "t1", "running").await;
    assert_echoes(addr, "t1", r#"{"ping":1}"#).await;
}

#[tokio::test]
async fn env_mode_exits_when_its_child_does() {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        buffer_lines: 100,
        argv: Some(vec!["true".to_owned()]),
        token: None,
        workspace: None,
        workspace_mount: "/workspace".into(),
        files: vec![],
        mode: Mode::Env,
    };
    let (tx, rx) = oneshot::channel();
    let done = tokio::spawn(run(config, Some(tx)));
    let _ = rx.await;
    let code = tokio::time::timeout(DEADLINE, done)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(code, 0);
}

#[tokio::test]
async fn a_failed_restart_answers_500_and_leaves_the_shim_serving() {
    let shim = start_in(Mode::Handshake, "/workspace".into()).await;
    let addr = shim.addr;
    assert_eq!(
        configure(
            addr,
            None,
            &configure_body("t1", json!({"HARNESS_CMD": "cat"}), "h1")
        )
        .await,
        204
    );
    wait_for_child(addr, "t1", "running").await;
    let bad = configure_body("t1", json!({"HARNESS_CMD": "/no/such/binary"}), "h2");
    assert_eq!(configure(addr, Some("t1"), &bad).await, 204);

    let (code, body) = http(addr, "POST", "/control/restart", Some("t1"), "").await;
    assert_eq!(code, 500);
    assert!(body["error"].is_string());
    assert_eq!(status(addr, Some("t1")).await["child"], "exited");
    assert!(!shim.done.is_finished());
    // Nothing to replace before the first spawn.
    let fresh = start(Mode::Handshake).await;
    assert_eq!(
        http(fresh, "POST", "/control/restart", None, "").await.0,
        409
    );
}

#[tokio::test]
async fn a_configure_that_finds_the_child_running_does_not_respawn_it_later() {
    let addr = start(Mode::Handshake).await;
    // Exits as soon as it reads a line.
    let dir = tempfile::tempdir().unwrap();
    let cmd = script(&dir, "harness.sh", "read line\n");
    let body = configure_body("t1", json!({"HARNESS_CMD": cmd}), "h1");
    assert_eq!(configure(addr, None, &body).await, 204);
    wait_for_child(addr, "t1", "running").await;
    // Same hash while running: no-op, and it must not leave a pending respawn behind.
    assert_eq!(configure(addr, Some("t1"), &body).await, 204);

    let mut client = connect_acp(addr, "t1").await;
    futures::SinkExt::send(&mut client, r#"{"x":1}"#.into())
        .await
        .unwrap();
    wait_for_child(addr, "t1", "exited").await;
    let generation = status(addr, Some("t1")).await["child_generation"].clone();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let now = status(addr, Some("t1")).await;
    assert_eq!(now["child"], "exited");
    assert_eq!(now["child_generation"], generation);
}

#[tokio::test]
async fn configured_files_are_written_before_each_spawn() {
    let addr = start(Mode::Handshake).await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("home/.codex/config.toml");
    let cmd = script(
        &dir,
        "harness.sh",
        &format!(
            "echo \"f=${{HARNESS_FILES-unset}} c=$(cat {} | tr -d '\\n')\"\ncat\n",
            config.display()
        ),
    );
    let body = |hash: &str, content: &str| {
        json!({
            "token": "t1",
            "env": {"HARNESS_CMD": cmd},
            "workspace": null,
            "files": [{"path": config.display().to_string(), "content": content}],
            "env_hash": hash,
        })
        .to_string()
    };
    assert_eq!(
        configure(addr, None, &body("h1", "model = \"a\"\n")).await,
        204
    );
    wait_for_child(addr, "t1", "running").await;
    assert_eq!(
        first_line(addr, "t1").await.as_deref(),
        Some("f=unset c=model = \"a\"")
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&config).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    // A reconfigure's files are written by the restart that applies it.
    assert_eq!(
        configure(addr, Some("t1"), &body("h2", "model = \"b\"\n")).await,
        204
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "model = \"a\"\n");
    let (code, _) = http(addr, "POST", "/control/restart", Some("t1"), "").await;
    assert_eq!(code, 200);
    assert_eq!(
        first_line(addr, "t1").await.as_deref(),
        Some("f=unset c=model = \"b\"")
    );
}

#[tokio::test]
async fn env_mode_writes_its_files_before_the_child_starts() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("settings.json");
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        buffer_lines: 100,
        argv: Some(vec![
            "sh".into(),
            "-c".into(),
            format!("cat {}; cat", target.display()),
        ]),
        token: Some("t".into()),
        workspace: None,
        workspace_mount: "/workspace".into(),
        files: vec![harness_shim::files::FileSpec {
            path: target.display().to_string(),
            content: "{\"ok\":true}\n".into(),
            mode: Some(0o644),
        }],
        mode: Mode::Env,
    };
    let (tx, rx) = oneshot::channel();
    let _done = tokio::spawn(run(config, Some(tx)));
    let addr = rx.await.unwrap();
    wait_for_child(addr, "t", "running").await;
    assert_eq!(
        first_line(addr, "t").await.as_deref(),
        Some("{\"ok\":true}")
    );
}

#[tokio::test]
async fn tilde_expansion_and_status_report_the_configured_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let shim = start_in(Mode::Handshake, dir.path().to_path_buf()).await;
    let addr = shim.addr;
    let cmd = script(
        &dir,
        "h.sh",
        "echo \"c=$CODEX_HOME f=$(cat \"$CODEX_HOME/config.toml\")\"\ncat\n",
    );
    let body = json!({
        "token": "t1",
        "env": {"HARNESS_CMD": cmd, "HOME": home.display().to_string(), "CODEX_HOME": "~/.codex"},
        "workspace": null,
        "files": [{"path": "~/.codex/config.toml", "content": "ok"}],
        "env_hash": "h1",
    })
    .to_string();
    assert_eq!(configure(addr, None, &body).await, 204);
    wait_for_child(addr, "t1", "running").await;
    assert_eq!(
        first_line(addr, "t1").await.as_deref(),
        Some(&*format!("c={}/.codex f=ok", home.display()))
    );
    let st = status(addr, Some("t1")).await;
    assert_eq!(st["workspace"], dir.path().display().to_string());
}

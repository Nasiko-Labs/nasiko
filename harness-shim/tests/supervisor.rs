//! In-process tests of `harness-supervisor` driving the real `harness-shim` binary.
//!
//! Each test uses its own fixed ports (the platform, not the supervisor, picks a session's port),
//! in a range of its own so tests running in parallel never collide.

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{configure_body, http, wait_for_child};
use harness_shim::supervisor::SupervisorConfig;
use serde_json::{Value, json};

const TOKEN: &str = "sup-token";

/// A running supervisor, its workspace root and its scratch root.
struct Pod {
    addr: SocketAddr,
    /// The sessions' root, one level inside `outer`, so a path that escaped it would still land
    /// in this test's own directory.
    root: PathBuf,
    /// The sessions' scratch root, beside `root`.
    scratch: PathBuf,
    outer: tempfile::TempDir,
}

fn real_shim() -> PathBuf {
    env!("CARGO_BIN_EXE_harness-shim").into()
}

async fn start(max: usize) -> Pod {
    start_with(max, real_shim()).await
}

fn config(max: usize, shim_binary: PathBuf, workspace_root: PathBuf) -> SupervisorConfig {
    SupervisorConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        token: TOKEN.into(),
        shim_binary,
        workspace_root,
        scratch_root: None,
        max_sessions: max,
    }
}

async fn start_with(max: usize, shim_binary: PathBuf) -> Pod {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    let scratch = outer.path().join("scratch");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    let config = SupervisorConfig {
        scratch_root: Some(scratch.clone()),
        ..config(max, shim_binary, root.clone())
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(harness_shim::supervisor::run(config, Some(tx)));
    Pod {
        addr: rx.await.unwrap(),
        root,
        scratch,
        outer,
    }
}

/// `PUT /sessions/{id}` with the bearer.
async fn put(sup: SocketAddr, id: &str, port: u16) -> (u16, Value) {
    put_body(sup, id, json!({"port": port})).await
}

async fn put_body(sup: SocketAddr, id: &str, body: Value) -> (u16, Value) {
    let path = format!("/sessions/{id}");
    http(sup, "PUT", &path, Some(TOKEN), &body.to_string()).await
}

async fn delete(sup: SocketAddr, path: &str) -> u16 {
    http(sup, "DELETE", path, Some(TOKEN), "").await.0
}

async fn get(sup: SocketAddr, path: &str) -> (u16, Value) {
    http(sup, "GET", path, Some(TOKEN), "").await
}

fn shim(port: u16) -> SocketAddr {
    (Ipv4Addr::LOCALHOST, port).into()
}

/// Whether anything accepts a connection on `port`.
async fn answers(port: u16) -> bool {
    let connect = tokio::net::TcpStream::connect(shim(port));
    matches!(
        tokio::time::timeout(Duration::from_secs(1), connect).await,
        Ok(Ok(_))
    )
}

fn session<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {list}"))
}

fn assert_running_with_stats(entry: &Value) {
    assert_eq!(entry["state"], "running", "{entry}");
    assert!(entry["stats"]["cpu_millicores"].is_u64(), "{entry}");
    assert!(entry["stats"]["memory_bytes"].is_u64(), "{entry}");
    assert!(entry["stats"]["sampled_at"].is_string(), "{entry}");
    // `/proc` exists only on Linux; elsewhere the process count is unknown, never 0.
    #[cfg(target_os = "linux")]
    assert!(
        entry["stats"]["processes"].as_u64().unwrap() >= 1,
        "{entry}"
    );
    #[cfg(not(target_os = "linux"))]
    assert!(entry["stats"]["processes"].is_null(), "{entry}");
}

fn ends_with_segment(path: &Value, segment: &str) -> bool {
    path.as_str()
        .is_some_and(|p| Path::new(p).file_name() == Some(segment.as_ref()))
}

/// Points `link` at `target`, replacing what it pointed at.
fn repoint(link: &Path, target: &Path) {
    let _ = std::fs::remove_file(link);
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[tokio::test]
async fn put_spawns_a_shim_and_is_idempotent() {
    let pod = start(8).await;

    let (code, placed) = put(pod.addr, "s-a", 23301).await;
    assert_eq!(code, 200, "{placed}");
    assert_eq!(placed["port"], 23301);
    assert!(placed["generation"].as_u64().unwrap() >= 1);

    let (code, _) = http(shim(23301), "GET", "/healthz", None, "").await;
    assert_eq!(code, 200);
    let (_, status) = http(shim(23301), "GET", "/control/status", None, "").await;
    assert_eq!(status["mode"], "handshake");
    // The session's directory is named by its id.
    assert!(ends_with_segment(&status["workspace"], "s-a"), "{status}");

    let (code, again) = put(pod.addr, "s-a", 23301).await;
    assert_eq!(code, 200);
    assert_eq!(again, placed);
    assert!(pod.root.join("s-a/home").is_dir());
}

#[tokio::test]
async fn two_sessions_run_on_their_told_ports() {
    let pod = start(8).await;
    for (id, port) in [("s-a", 23311), ("s-b", 23312)] {
        assert_eq!(put(pod.addr, id, port).await.0, 200);
        // A harness child gives each session's tree a process to count.
        let body = configure_body("shim-token", json!({"HARNESS_CMD": "cat"}), "h1");
        let (code, _) = http(shim(port), "POST", "/control/configure", None, &body).await;
        assert_eq!(code, 204);
        wait_for_child(shim(port), "shim-token", "running").await;
    }

    let (code, list) = get(pod.addr, "/sessions").await;
    assert_eq!(code, 200, "{list}");
    assert_eq!(list.as_array().unwrap().len(), 2);
    for (id, port) in [("s-a", 23311), ("s-b", 23312)] {
        let entry = session(&list, id);
        assert_eq!(entry["port"], port);
        assert!(entry["started_at"].is_string());
        assert_running_with_stats(entry);
    }

    let (code, one) = get(pod.addr, "/sessions/s-b").await;
    assert_eq!(code, 200);
    assert_eq!(one["id"], "s-b");
    assert_running_with_stats(&one);
    assert_eq!(get(pod.addr, "/sessions/s-z").await.0, 404);
}

#[tokio::test]
async fn delete_stops_and_purge_removes_the_directory() {
    let pod = start(8).await;
    let (_, first) = put(pod.addr, "s-a", 23321).await;

    assert_eq!(delete(pod.addr, "/sessions/s-a").await, 204);
    assert!(!answers(23321).await, "the stopped shim still answers");
    assert!(pod.root.join("s-a").is_dir());
    let (code, stopped) = get(pod.addr, "/sessions/s-a").await;
    assert_eq!(code, 200);
    assert_eq!(stopped["state"], "exited");
    assert!(stopped["stats"].is_null());

    // The next PUT respawns the stopped session in its directory.
    let (code, respawned) = put(pod.addr, "s-a", 23321).await;
    assert_eq!(code, 200, "{respawned}");
    assert!(respawned["generation"].as_u64() > first["generation"].as_u64());
    assert!(answers(23321).await);

    assert_eq!(delete(pod.addr, "/sessions/s-a?purge=true").await, 204);
    assert!(!answers(23321).await);
    assert!(!pod.root.join("s-a").exists());
    assert_eq!(get(pod.addr, "/sessions/s-a").await.0, 404);
    assert_eq!(delete(pod.addr, "/sessions/s-a").await, 204);
    assert_eq!(delete(pod.addr, "/sessions/s-a?purge=true").await, 204);
}

#[tokio::test]
async fn a_session_gets_its_own_scratch_directory_until_purged() {
    let pod = start(8).await;
    put(pod.addr, "s-a", 23401).await;
    put(pod.addr, "s-b", 23402).await;
    assert!(pod.scratch.join("s-a").is_dir());
    assert!(pod.scratch.join("s-b").is_dir());
    // Its TMPDIR, which must exist before anything writes a temporary file.
    assert!(pod.scratch.join("s-a/tmp").is_dir());
    std::fs::write(pod.scratch.join("s-a/cache"), "x").unwrap();

    // A suspend keeps it; the purge takes it, and only its own.
    assert_eq!(delete(pod.addr, "/sessions/s-a").await, 204);
    assert!(pod.scratch.join("s-a/cache").is_file());
    assert_eq!(delete(pod.addr, "/sessions/s-a?purge=true").await, 204);
    assert!(!pod.scratch.join("s-a").exists());
    assert!(pod.scratch.join("s-b").is_dir());
}

#[tokio::test]
async fn a_scratch_root_that_is_not_a_directory_is_a_startup_error() {
    let outer = tempfile::tempdir().unwrap();
    let config = SupervisorConfig {
        scratch_root: Some(outer.path().join("absent")),
        ..config(8, real_shim(), outer.path().into())
    };
    let err = harness_shim::supervisor::run(config, None)
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("HARNESS_SCRATCH_ROOT"),
        "{err:#}"
    );
}

#[tokio::test]
async fn purging_an_unknown_session_removes_its_directory() {
    let pod = start(8).await;
    // Left behind by a supervisor that restarted and so forgot the session.
    let left = pod.root.join("s-old");
    std::fs::create_dir_all(left.join("home")).unwrap();
    std::fs::write(left.join("home/notes"), "x").unwrap();
    let scratch = pod.scratch.join("s-old");
    std::fs::create_dir_all(scratch.join("npm")).unwrap();

    assert_eq!(delete(pod.addr, "/sessions/s-old?purge=true").await, 204);
    assert!(!left.exists());
    assert!(!scratch.exists());
}

#[tokio::test]
async fn a_traversal_id_is_refused_before_any_path_is_built() {
    let pod = start(8).await;
    let outside = pod.outer.path().join("x");
    std::fs::create_dir(&outside).unwrap();

    for path in [
        "/sessions/..%2Fx?purge=true",
        "/sessions/..%2Fx",
        "/sessions/..",
    ] {
        assert_eq!(delete(pod.addr, path).await, 400, "{path}");
    }
    assert!(outside.is_dir(), "a purge escaped the workspace root");
}

#[tokio::test]
async fn conflicts_answer_409() {
    let pod = start(1).await;
    assert_eq!(put(pod.addr, "s-a", 23331).await.0, 200);
    let (code, full) = put(pod.addr, "s-b", 23332).await;
    assert_eq!(code, 409, "{full}");
    assert!(full["error"].is_string());
    // The same session on a different port means the caller's placement is wrong.
    assert_eq!(put(pod.addr, "s-a", 23333).await.0, 409);
    assert_eq!(delete(pod.addr, "/sessions/s-a").await, 204);
    assert_eq!(put(pod.addr, "s-b", 23332).await.0, 200);

    let pod = start(2).await;
    assert_eq!(put(pod.addr, "s-b", 23334).await.0, 200);
    assert_eq!(put(pod.addr, "s-c", 23334).await.0, 409);
}

#[tokio::test]
async fn a_shim_that_fails_to_start_is_torn_down_and_the_next_put_retries() {
    let bin = tempfile::tempdir().unwrap();
    let shim_binary = bin.path().join("shim");
    repoint(&shim_binary, Path::new("/usr/bin/false"));
    let pod = start_with(8, shim_binary.clone()).await;

    let (code, failed) = put(pod.addr, "s-a", 23361).await;
    assert_eq!(code, 500, "{failed}");
    assert!(failed["error"].is_string());
    let (_, entry) = get(pod.addr, "/sessions/s-a").await;
    assert_eq!(entry["state"], "exited", "{entry}");
    let (_, health) = http(pod.addr, "GET", "/healthz", None, "").await;
    assert_eq!(health["sessions"], 0);

    repoint(&shim_binary, Path::new(env!("CARGO_BIN_EXE_harness-shim")));
    let (code, placed) = put(pod.addr, "s-a", 23361).await;
    assert_eq!(code, 200, "{placed}");
    assert!(placed["generation"].as_u64() > entry["generation"].as_u64());
    assert!(answers(23361).await);
}

#[tokio::test]
async fn a_shim_token_is_required_by_the_shim_from_the_start() {
    let pod = start(8).await;
    let body = json!({"port": 23371, "shim_token": "session-bearer"});
    assert_eq!(put_body(pod.addr, "s-a", body).await.0, 200);

    let (code, _) = http(shim(23371), "GET", "/control/status", None, "").await;
    assert_eq!(code, 401);
    let bearer = Some("session-bearer");
    let (code, status) = http(shim(23371), "GET", "/control/status", bearer, "").await;
    assert_eq!(code, 200, "{status}");
}

#[tokio::test]
async fn the_bearer_is_required_everywhere_but_healthz() {
    let pod = start(8).await;
    let body = json!({"port": 23341}).to_string();
    for bearer in [None, Some("wrong")] {
        let (code, _) = http(pod.addr, "PUT", "/sessions/s-a", bearer, &body).await;
        assert_eq!(code, 401);
        assert_eq!(http(pod.addr, "GET", "/sessions", bearer, "").await.0, 401);
        assert_eq!(
            http(pod.addr, "GET", "/sessions/s-a", bearer, "").await.0,
            401
        );
        let deleted = http(pod.addr, "DELETE", "/sessions/s-a", bearer, "").await;
        assert_eq!(deleted.0, 401);
    }
    assert!(!answers(23341).await, "an unauthorized PUT spawned a shim");

    let (code, health) = http(pod.addr, "GET", "/healthz", None, "").await;
    assert_eq!(code, 200);
    assert_eq!(health["sessions"], 0);
    assert_eq!(put(pod.addr, "s-a", 23341).await.0, 200);
    let (_, health) = http(pod.addr, "GET", "/healthz", None, "").await;
    assert_eq!(health["sessions"], 1);
}

#[tokio::test]
async fn a_bad_id_port_or_body_is_refused() {
    let pod = start(8).await;
    for id in ["..%2Fx", "..", "s%20a"] {
        let (code, refused) = put(pod.addr, id, 23351).await;
        assert_eq!(code, 400, "id {id}: {refused}");
        assert!(refused["error"].is_string());
    }
    for port in [80, pod.addr.port()] {
        assert_eq!(put(pod.addr, "s-a", port).await.0, 400, "port {port}");
    }
    for body in [
        json!({"port": 23351, "shim_token": ""}),
        json!({"shim_token": "t"}),
        json!({"port": 70000}),
    ] {
        assert_eq!(
            put_body(pod.addr, "s-a", body.clone()).await.0,
            400,
            "{body}"
        );
    }
    let (code, _) = http(pod.addr, "PUT", "/sessions/s-a", Some(TOKEN), "not json").await;
    assert_eq!(code, 400);
    assert!(!answers(23351).await);
    assert_eq!(std::fs::read_dir(&pod.root).unwrap().count(), 0);
}

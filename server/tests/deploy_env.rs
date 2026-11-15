//! The platform's deploy-time env (`agents::deploy_env`) reaches every deploy path.
//!
//! Each test drives one real path against the harness's recording `FakeRuntime` and asserts on the
//! spec the runtime was handed: the LLM router wiring, `MCP_GATEWAY_TOKEN`, and the state API
//! address and credential. Not driven here, because each needs a real network or Docker daemon:
//! the GitHub clone worker arm (a live GitHub tarball download; it calls the same
//! `deploy_env::build` as the legacy clone arm tested below) and the registry import
//! (a live registry's manifest).
//!
//! Requires infra (Postgres, Redis, S3 - see TEST_PG_URL / TEST_REDIS_URL / TEST_S3_ENDPOINT):
//!   cargo test -p nasiko-server --test deploy_env -- --test-threads=1

mod common;

use std::collections::HashMap;
use std::time::Duration;

use nasiko_runtime::{ContainerId, DeploymentSpec};
use nasiko_server::agents::upload::BuildJobPayload;
use nasiko_server::seed::{InternalAgentSeed, seed_agents_if_configured, seed_internal_agent};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const GATEWAY_URL: &str = "http://cp.test:8080/api/mcp";
const STATE_URL: &str = "http://cp.test:8080/api/agent-state";
const LLM_BASE: &str = "http://llm.test:8080";

// ─── Fixtures ───────────────────────────────────────────────────────────────

struct Fixture {
    server: common::TestServer,
    admin_id: Uuid,
}

async fn start_with(
    gateway_url: Option<&'static str>,
    platform_openai_key: Option<&'static str>,
) -> Fixture {
    // SAFETY: tests run serially (#[serial]), so no concurrent env mutation.
    unsafe {
        std::env::set_var("AGENT_JWT_SECRET", "deploy-env-test-secret-0123456789");
        std::env::set_var("LLM_GATEWAY_BASE_URL", LLM_BASE);
    }
    start_configured(move |config| {
        config.mcp_gateway_public_url = gateway_url.map(str::to_owned);
        config.openai_api_key = platform_openai_key.map(str::to_owned);
        config.openai_base_url = platform_openai_key.map(|_| "https://api.openai.test/v1".into());
    })
    .await
}

async fn start_configured(configure: impl FnOnce(&mut nasiko_config::Config)) -> Fixture {
    let server = common::TestServer::start_with(configure).await;
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
    let admin_id = admin["user_id"].as_str().unwrap().parse().unwrap();
    Fixture { server, admin_id }
}

async fn start() -> Fixture {
    start_with(Some(GATEWAY_URL), None).await
}

const PLATFORM_KEY: &str = "platform-openai-key";

/// A server that holds the platform's OpenAI key and has no LLM gateway configured: the gateway
/// would overwrite `OPENAI_API_KEY` on every deploy and hide whether the platform key was handed
/// out at all.
async fn start_with_platform_key(fallback: bool) -> Fixture {
    // SAFETY: tests run serially (#[serial]), so no concurrent env mutation.
    unsafe {
        std::env::remove_var("LLM_GATEWAY_BASE_URL");
    }
    start_configured(move |config| {
        config.agent_platform_key_fallback = fallback;
        config.openai_api_key = Some(PLATFORM_KEY.to_owned());
    })
    .await
}

/// The platform key is in the deploy env exactly when the fallback is on.
fn assert_platform_key(spec: &DeploymentSpec, fallback: bool) {
    assert_eq!(
        spec.env_vars.get("OPENAI_API_KEY").map(String::as_str),
        fallback.then_some(PLATFORM_KEY),
        "AGENT_PLATFORM_KEY_FALLBACK={fallback}"
    );
}

impl Fixture {
    fn uid(&self) -> String {
        self.admin_id.to_string()
    }

    async fn agent_row(&self, name: &str, image: Option<&str>, status: &str) -> Uuid {
        sqlx::query_scalar(
            // Resident: these tests are about the env a deployed workload gets, and a parkable
            // agent has none.
            "INSERT INTO agents (name, owner_id, image, status, version, parking_policy) \
             VALUES ($1, $2, $3, $4, '1.0.0', 'resident') RETURNING id",
        )
        .bind(name)
        .bind(self.admin_id)
        .bind(image)
        .bind(status)
        .fetch_one(&self.server.db)
        .await
        .expect("seed agent")
    }

    fn specs_for(&self, agent_id: Uuid) -> Vec<DeploymentSpec> {
        let container = ContainerId::from_uuid(agent_id);
        self.server
            .runtime
            .deployed_specs()
            .into_iter()
            .filter(|spec| spec.container_id == container)
            .collect()
    }

    /// Waits for a deploy of `agent_id` beyond the `seen` already recorded; returns the newest.
    async fn spec_after(&self, agent_id: Uuid, seen: usize) -> DeploymentSpec {
        for _ in 0..200 {
            let specs = self.specs_for(agent_id);
            if specs.len() > seen {
                return specs.last().unwrap().clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("agent {agent_id} was never deployed (after {seen} earlier deploys)");
    }

    async fn first_spec(&self, agent_id: Uuid) -> DeploymentSpec {
        self.spec_after(agent_id, 0).await
    }

    async fn agent_id_by_name(&self, name: &str) -> Uuid {
        sqlx::query_scalar("SELECT id FROM agents WHERE name = $1")
            .bind(name)
            .fetch_one(&self.server.db)
            .await
            .expect("agent by name")
    }

    async fn create_agent(&self, name: &str, version: &str) -> Uuid {
        let res = common::as_superuser(
            self.server.client.post(self.server.url("/api/agents")),
            &self.uid(),
            "admin",
        )
        .json(&json!({"name": name, "version": version}))
        .send()
        .await
        .unwrap();
        assert_eq!(res.status(), 201);
        let body: Value = res.json().await.unwrap();
        body["id"].as_str().unwrap().parse().unwrap()
    }

    async fn state_mode(&self, agent_id: Uuid) -> String {
        sqlx::query_scalar("SELECT state_mode FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&self.server.db)
            .await
            .unwrap()
    }

    async fn register_with(&self, token: &str, capabilities: Value) -> reqwest::StatusCode {
        self.server
            .client
            .post(self.server.url("/api/agent-state/register"))
            .bearer_auth(token)
            .json(&json!({"capabilities": capabilities, "client": "deploy-env-test"}))
            .send()
            .await
            .unwrap()
            .status()
    }
}

fn dockerfile_zip() -> Vec<u8> {
    common::make_zip(&[
        (
            "Dockerfile",
            b"FROM python:3.11-slim\nCMD [\"python\", \"main.py\"]",
        ),
        ("main.py", b"print('hello')"),
    ])
}

/// What every deploy path must hand the runtime.
fn assert_platform_env(spec: &DeploymentSpec) {
    assert_credentials(spec);
    assert_llm_wiring(spec);
}

/// The gateway and state credentials and the state API address.
fn assert_credentials(spec: &DeploymentSpec) {
    let env = &spec.env_vars;
    let gateway = env.get("MCP_GATEWAY_TOKEN").expect("MCP_GATEWAY_TOKEN");
    assert!(gateway.starts_with("ngt_"), "gateway credential: {gateway}");
    let state_token = env.get("NASIKO_AGENT_TOKEN").expect("NASIKO_AGENT_TOKEN");
    assert!(state_token.starts_with("nst_"), "state credential");
    assert_eq!(
        env.get("NASIKO_STATE_URL").map(String::as_str),
        Some(STATE_URL)
    );
}

/// The LLM router wiring: its base URL and the agent-identity JWT as the key.
fn assert_llm_wiring(spec: &DeploymentSpec) {
    let env = &spec.env_vars;
    assert_eq!(
        env.get("OPENAI_BASE_URL").map(String::as_str),
        Some("http://llm.test:8080/v1"),
        "LLM router base URL"
    );
    let llm_key = env.get("OPENAI_API_KEY").expect("OPENAI_API_KEY");
    assert_eq!(llm_key.matches('.').count(), 2, "the router wiring's JWT");
}

// ─── Every deploy path ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn startup_reconcile_deploy_carries_the_platform_env() {
    let f = start().await;
    let agent_id = f
        .agent_row("reconciled-agent", Some("nasiko/echo:1.0.0"), "running")
        .await;
    f.server.runtime.set_status_follows_deploys(true);

    f.server.state.init().await;

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn seed_agent_deploy_carries_the_platform_env() {
    let f = start().await;
    // A seed image is mirrored from its source registry first. This test has no registry, so it
    // points at an unreachable one and starts from an earlier mirrored copy, which a failed mirror
    // falls back to; the redeploy of that copy is the deploy judged here. `SEED_FORCE_PULL` makes
    // it redeploy although the copy is unchanged and the fake runtime reports it running.
    let agent_id = f.create_agent("seeded-echo", "1.0.0").await;
    sqlx::query("UPDATE agents SET image = 'localhost:5000/nasiko/seeded-echo:v1' WHERE id = $1")
        .bind(agent_id)
        .execute(&f.server.db)
        .await
        .unwrap();
    // SAFETY: serial tests.
    unsafe {
        std::env::set_var("SEED_AGENTS", "127.0.0.1:9/nasiko/seeded-echo:v1");
        std::env::set_var("SEED_FORCE_PULL", "1");
    }

    // The seed routine then waits for the agent card from a container that is not there; the
    // deploy it is judged on has long been recorded by the time that wait is cut off.
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        seed_agents_if_configured(&f.server.state),
    )
    .await;
    unsafe {
        std::env::remove_var("SEED_AGENTS");
        std::env::remove_var("SEED_FORCE_PULL");
    }

    let spec = f.first_spec(agent_id).await;
    assert_platform_env(&spec);
    // A seed is a platform agent: kept resident, so it deploys an always-running workload.
    assert!(!spec.parkable);
    let policy: String = sqlx::query_scalar("SELECT parking_policy FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&f.server.db)
        .await
        .unwrap();
    assert_eq!(policy, "resident");
    f.server.cleanup().await;
}

/// Runs the seed routine over `images` once, as the server's startup does.
async fn seed_once(f: &Fixture, images: &str) {
    // SAFETY: serial tests.
    unsafe { std::env::set_var("SEED_AGENTS", images) };
    // As above: cut off the agent-card wait once the deploy is recorded.
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        seed_agents_if_configured(&f.server.state),
    )
    .await;
    unsafe { std::env::remove_var("SEED_AGENTS") };
}

/// `SEED_AGENT_SETTINGS` gives a seed agent its own CPU and memory on its deploy, a running seed
/// agent is redeployed only when its settings change, and the stored values are what every later
/// deploy reads.
#[tokio::test]
#[serial]
async fn seed_agent_settings_reach_the_deploy_and_only_a_change_redeploys() {
    let settings = |memory: &str| {
        nasiko_config::parse_seed_agent_settings(Some(&format!(
            r#"{{"seeded-echo": {{"cpu_limit_milli": 1000, "cpu_request_milli": 250,
                                 "memory_limit": "{memory}"}}}}"#
        )))
        .unwrap()
    };
    let mut f = start_configured(|config| config.seed_agent_settings = settings("1536Mi")).await;
    // As in the test above: a running agent with an earlier mirrored copy, which a failed mirror
    // falls back to. Without SEED_FORCE_PULL only a settings change redeploys it.
    let agent_id = f.create_agent("seeded-echo", "1.0.0").await;
    sqlx::query("UPDATE agents SET image = 'localhost:5000/nasiko/seeded-echo:v1' WHERE id = $1")
        .bind(agent_id)
        .execute(&f.server.db)
        .await
        .unwrap();
    let images = "127.0.0.1:9/nasiko/seeded-echo:v1";

    seed_once(&f, images).await;
    let limits = f.first_spec(agent_id).await.resources.expect("resources");
    assert_eq!(
        (
            limits.memory.as_str(),
            limits.cpu_milli,
            limits.cpu_request_milli
        ),
        ("1536Mi", 1000, Some(250))
    );
    let stored: (Option<i32>, Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT cpu_limit_milli, cpu_request_milli, memory_limit_mib FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    assert_eq!(stored, (Some(1000), Some(250), Some(1536)));

    seed_once(&f, images).await;
    assert_eq!(
        f.specs_for(agent_id).len(),
        1,
        "unchanged settings, no redeploy"
    );

    f.server.state.config = std::sync::Arc::new(nasiko_config::Config {
        seed_agent_settings: settings("2Gi"),
        ..(*f.server.state.config).clone()
    });
    seed_once(&f, images).await;
    let limits = f
        .spec_after(agent_id, 1)
        .await
        .resources
        .expect("resources");
    assert_eq!(limits.memory, "2048Mi");
    f.server.cleanup().await;
}

/// An internal agent's model access is its own `env`; the LLM router (configured in this
/// fixture) must not take it over, but the credentials still arrive.
#[tokio::test]
#[serial]
async fn internal_agent_seed_deploy_carries_the_credentials_and_keeps_its_direct_llm_env() {
    let f = start().await;

    seed_internal_agent(
        &f.server.state,
        InternalAgentSeed {
            name: "internal-helper".into(),
            image: "nasiko/helper:1".into(),
            force_pull: false,
            env: HashMap::from([
                ("EDITION_SETTING".into(), "kept".into()),
                ("OPENAI_API_KEY".into(), "direct-key".into()),
                ("OPENAI_BASE_URL".into(), "http://direct.test/v1".into()),
            ]),
            public: false,
            writable_path: None,
        },
    )
    .await;

    let agent_id = f.agent_id_by_name("internal-helper").await;
    let spec = f.first_spec(agent_id).await;
    assert_credentials(&spec);
    assert_eq!(spec.env_vars["EDITION_SETTING"], "kept");
    assert_eq!(spec.env_vars["OPENAI_API_KEY"], "direct-key");
    assert_eq!(spec.env_vars["OPENAI_BASE_URL"], "http://direct.test/v1");
    assert!(!spec.writable);
    assert!(!spec.parkable, "an internal agent is resident");
    f.server.cleanup().await;
}

/// An internal agent that writes files gets its volume where it asked, and the
/// row records it so a later redeploy (the internal-agent watch) mounts the same.
#[tokio::test]
#[serial]
async fn internal_agent_seed_with_a_writable_path_mounts_it_and_records_it() {
    let f = start().await;

    seed_internal_agent(
        &f.server.state,
        InternalAgentSeed {
            name: "internal-writer".into(),
            image: "nasiko/writer:1".into(),
            force_pull: false,
            env: HashMap::new(),
            public: false,
            writable_path: Some("/data".into()),
        },
    )
    .await;

    let agent_id = f.agent_id_by_name("internal-writer").await;
    let spec = f.first_spec(agent_id).await;
    assert!(spec.writable);
    assert_eq!(spec.writable_path.as_deref(), Some("/data"));
    let row: (bool, Option<String>) =
        sqlx::query_as("SELECT writable, writable_path FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&f.server.state.db)
            .await
            .unwrap();
    assert_eq!(row, (true, Some("/data".to_string())));
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn ad_hoc_deploy_of_a_registered_agent_carries_the_platform_env() {
    let f = start().await;
    let agent_id = f.create_agent("adhoc-agent", "1.0.0").await;

    let res = common::as_superuser(
        f.server.client.post(f.server.url("/api/containers")),
        &f.uid(),
        "admin",
    )
    .json(&json!({"image": "nasiko/adhoc:1", "name": "adhoc-agent"}))
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "deploy: {}", res.status());

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn admin_redeploy_carries_the_platform_env() {
    let f = start().await;
    let agent_id = f
        .agent_row("redeployed-agent", Some("nasiko/redeploy:1"), "running")
        .await;

    let res = common::as_superuser(
        f.server
            .client
            .post(f.server.url("/api/containers/redeployed-agent/restart")),
        &f.uid(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "restart: {}", res.status());

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

/// A stopped deployment row for `agent_id`, ready to be restarted.
async fn stopped_deployment(f: &Fixture, agent_id: Uuid) -> Uuid {
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status) \
         VALUES ($1, '1.0.0', 'nasiko/restart:1', 'success') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO agent_deployments (agent_id, build_id, owner_id, status) \
         VALUES ($1, $2, $3, 'stopped') RETURNING id",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(f.admin_id)
    .fetch_one(&f.server.db)
    .await
    .unwrap()
}

async fn restart_deployment(f: &Fixture, deployment_id: Uuid) {
    let res = common::as_superuser(
        f.server.client.post(
            f.server
                .url(&format!("/api/agents/deployment/{deployment_id}/restart")),
        ),
        &f.uid(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "restart: {}", res.status());
}

#[tokio::test]
#[serial]
async fn deployment_restart_carries_the_platform_env() {
    let f = start().await;
    let agent_id = f
        .agent_row("restarted-agent", Some("nasiko/restart:1"), "running")
        .await;
    let deployment_id = stopped_deployment(&f, agent_id).await;

    restart_deployment(&f, deployment_id).await;

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

const REAL_PLATFORM_KEY: &str = "sk-real-platform-key-never-in-a-container";

#[tokio::test]
#[serial]
async fn a_non_openai_agent_never_receives_the_platforms_real_openai_key() {
    let f = start_with(Some(GATEWAY_URL), Some(REAL_PLATFORM_KEY)).await;
    for (format, key_var, base_var) in [
        ("anthropic", "ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"),
        ("gemini", "GEMINI_API_KEY", "GOOGLE_GEMINI_BASE_URL"),
        ("openai", "OPENAI_API_KEY", "OPENAI_BASE_URL"),
    ] {
        let agent_id = f
            .agent_row(
                &format!("{format}-agent"),
                Some("nasiko/restart:1"),
                "running",
            )
            .await;
        sqlx::query("UPDATE agents SET inbound_format = $2 WHERE id = $1")
            .bind(agent_id)
            .bind(format)
            .execute(&f.server.db)
            .await
            .unwrap();
        let deployment_id = stopped_deployment(&f, agent_id).await;

        restart_deployment(&f, deployment_id).await;

        let env = f.first_spec(agent_id).await.env_vars;
        assert!(
            env.values().all(|v| v != REAL_PLATFORM_KEY),
            "{format} agent got the real platform key"
        );
        assert_eq!(env["OPENAI_API_KEY"].matches('.').count(), 2, "{format}");
        assert_eq!(
            env["OPENAI_BASE_URL"], "http://llm.test:8080/v1",
            "{format}"
        );
        assert_eq!(env[key_var].matches('.').count(), 2, "{format}");
        assert!(env[base_var].starts_with(LLM_BASE), "{format}");
    }
    f.server.cleanup().await;
}

async fn upload_zip(f: &Fixture, name: &str, extra: Vec<(&'static str, String)>) -> Uuid {
    let mut form = reqwest::multipart::Form::new()
        .text("name", name.to_owned())
        .text("version_tag", "1.0.0");
    for (key, value) in extra {
        form = form.text(key, value);
    }
    form = form.part(
        "source",
        reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
    );
    let res = common::as_superuser(
        f.server.client.post(f.server.url("/api/agents/upload")),
        &f.uid(),
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "upload: {}", res.status());
    let body: Value = res.json().await.unwrap();
    body["data"]["agent_id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
#[serial]
async fn zip_upload_deploy_carries_the_platform_env() {
    let f = start().await;
    let agent_id = upload_zip(&f, "uploaded-agent", vec![]).await;

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_update_deploy_carries_the_platform_env() {
    let f = start().await;
    let agent_id = f.create_agent("updated-agent", "1.0.0").await;

    let form = reqwest::multipart::Form::new().part(
        "source",
        reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
    );
    let res = common::as_superuser(
        f.server
            .client
            .put(f.server.url(&format!("/api/agents/{agent_id}/update"))),
        &f.uid(),
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_rollback_deploy_carries_the_platform_env() {
    let f = start().await;
    let agent_id = f.create_agent("rolled-back-agent", "1.0.1").await;
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '1.0.0', 'rolled-back-agent:1.0.0', false, true, 'archived')",
    )
    .bind(agent_id)
    .execute(&f.server.db)
    .await
    .unwrap();

    let res = common::as_superuser(
        f.server
            .client
            .post(f.server.url(&format!("/api/agents/{agent_id}/rollback"))),
        &f.uid(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn source_import_deploy_carries_the_platform_env() {
    let f = start().await;
    let package = common::make_zip(&[
        (
            "AgentCard.json",
            br#"{"name": "imported-agent", "version": "1.0.0"}"#,
        ),
        ("Dockerfile", b"FROM scratch"),
    ]);
    let form = reqwest::multipart::Form::new().part(
        "package",
        reqwest::multipart::Part::bytes(package).file_name("agent.zip"),
    );
    let res = common::as_superuser(
        f.server.client.post(f.server.url("/api/import/upload")),
        &f.uid(),
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "import: {}", res.status());

    let agent_id = f.agent_id_by_name("imported-agent").await;
    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

/// Queues the legacy `Clone` job the worker still runs: its arm and the GitHub arm both end in
/// `execute_clone_and_deploy`. The source builds (a Dockerfile and a valid `AgentCard.json`).
async fn queue_clone_job(f: &Fixture, agent_id: Uuid, name: &str) {
    use std::io::Write;
    let image = format!("nasiko/{name}:1.0.0");
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', $2) RETURNING id",
    )
    .bind(agent_id)
    .bind(&image)
    .fetch_one(&f.server.db)
    .await
    .unwrap();

    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for (file, data) in [
            ("AgentCard.json", br#"{"version": "1.0.0"}"#.as_slice()),
            ("Dockerfile", b"FROM scratch".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, file, data).unwrap();
        }
        builder.finish().unwrap();
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tar_bytes).unwrap();
    let tar_path = std::env::temp_dir().join(format!("deploy-env-clone-{}.tar.gz", Uuid::new_v4()));
    std::fs::write(&tar_path, encoder.finish().unwrap()).unwrap();

    let payload = BuildJobPayload::Clone {
        build_id,
        agent_id,
        owner_id: f.admin_id,
        upload_id: "deploy-env-clone".into(),
        name: name.into(),
        tar_gz_path: tar_path.to_string_lossy().into_owned(),
        image_tag: image,
        ports: vec![8000],
        env: HashMap::new(),
    };
    sqlx::query("INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)")
        .bind(agent_id)
        .bind(f.admin_id)
        .bind(serde_json::to_value(&payload).unwrap())
        .execute(&f.server.db)
        .await
        .unwrap();
    let _ = f.server.state.build_tx.send(()).await;
}

/// The clone pipeline the GitHub path ends in; see `queue_clone_job`.
#[tokio::test]
#[serial]
async fn clone_job_deploy_carries_the_platform_env() {
    let f = start().await;
    let agent_id = f.agent_row("cloned-agent", None, "deploying").await;

    queue_clone_job(&f, agent_id, "cloned-agent").await;

    assert_platform_env(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

// ─── The platform key fallback ──────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn zip_upload_deploy_hands_out_the_platform_key_only_while_the_fallback_is_on() {
    for fallback in [false, true] {
        let f = start_with_platform_key(fallback).await;
        let agent_id = upload_zip(&f, "uploaded-agent", vec![]).await;

        assert_platform_key(&f.first_spec(agent_id).await, fallback);
        f.server.cleanup().await;
    }
}

#[tokio::test]
#[serial]
async fn clone_job_deploy_hands_out_the_platform_key_only_while_the_fallback_is_on() {
    for fallback in [false, true] {
        let f = start_with_platform_key(fallback).await;
        let agent_id = f.agent_row("cloned-agent", None, "deploying").await;
        queue_clone_job(&f, agent_id, "cloned-agent").await;

        assert_platform_key(&f.first_spec(agent_id).await, fallback);
        f.server.cleanup().await;
    }
}

#[tokio::test]
#[serial]
async fn admin_restart_hands_out_the_platform_key_only_while_the_fallback_is_on() {
    for fallback in [false, true] {
        let f = start_with_platform_key(fallback).await;
        let agent_id = f
            .agent_row("restarted-agent", Some("nasiko/restart:1"), "running")
            .await;

        let res = common::as_superuser(
            f.server
                .client
                .post(f.server.url("/api/containers/restarted-agent/restart")),
            &f.uid(),
            "admin",
        )
        .send()
        .await
        .unwrap();
        assert!(res.status().is_success(), "restart: {}", res.status());

        assert_platform_key(&f.first_spec(agent_id).await, fallback);
        f.server.cleanup().await;
    }
}

// ─── The state credential ───────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn the_injected_state_credential_authenticates_against_the_state_api() {
    let f = start().await;
    let agent_id = f
        .agent_row("registering-agent", Some("nasiko/echo:1.0.0"), "running")
        .await;
    f.server.runtime.set_status_follows_deploys(true);
    f.server.state.init().await;
    let spec = f.first_spec(agent_id).await;

    let token = &spec.env_vars["NASIKO_AGENT_TOKEN"];
    assert_eq!(
        f.register_with(token, json!(["tasks"])).await,
        reqwest::StatusCode::NO_CONTENT
    );
    assert_eq!(f.state_mode(agent_id).await, "durable");
    // The gateway credential from the same env is not a key to this door.
    let gateway = &spec.env_vars["MCP_GATEWAY_TOKEN"];
    assert_eq!(
        f.register_with(gateway, json!(["tasks"])).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_redeploy_resets_the_state_mode_to_memory() {
    let f = start().await;
    let agent_id = f.create_agent("durable-agent", "1.0.1").await;
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '1.0.0', 'durable-agent:1.0.0', false, true, 'archived')",
    )
    .bind(agent_id)
    .execute(&f.server.db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE agents SET state_mode = 'durable', parking_policy = 'resident' WHERE id = $1",
    )
    .bind(agent_id)
    .execute(&f.server.db)
    .await
    .unwrap();

    let res = common::as_superuser(
        f.server
            .client
            .post(f.server.url(&format!("/api/agents/{agent_id}/rollback"))),
        &f.uid(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);
    f.first_spec(agent_id).await;

    assert_eq!(f.state_mode(agent_id).await, "memory");
    f.server.cleanup().await;
}

async fn mark_durable(f: &Fixture, agent_id: Uuid) {
    sqlx::query(
        "UPDATE agents SET state_mode = 'durable', parking_policy = 'resident' WHERE id = $1",
    )
    .bind(agent_id)
    .execute(&f.server.db)
    .await
    .unwrap();
}

/// A restart brings the same image back, so a durable agent must still be durable afterwards.
#[tokio::test]
#[serial]
async fn a_deployment_restart_keeps_the_state_mode() {
    let f = start().await;
    let agent_id = f
        .agent_row("kept-durable-agent", Some("nasiko/restart:1"), "running")
        .await;
    mark_durable(&f, agent_id).await;
    let deployment_id = stopped_deployment(&f, agent_id).await;

    restart_deployment(&f, deployment_id).await;
    f.first_spec(agent_id).await;

    assert_eq!(f.state_mode(agent_id).await, "durable");
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn startup_reconcile_keeps_the_state_mode() {
    let f = start().await;
    let agent_id = f
        .agent_row(
            "recovered-durable-agent",
            Some("nasiko/echo:1.0.0"),
            "running",
        )
        .await;
    mark_durable(&f, agent_id).await;
    f.server.runtime.set_status_follows_deploys(true);

    f.server.state.init().await;
    f.first_spec(agent_id).await;

    assert_eq!(f.state_mode(agent_id).await, "durable");
    f.server.cleanup().await;
}

/// An update ships a new image, which may not register a task store.
#[tokio::test]
#[serial]
async fn an_update_resets_the_state_mode_to_memory() {
    let f = start().await;
    let agent_id = f.create_agent("updated-durable-agent", "1.0.0").await;
    mark_durable(&f, agent_id).await;

    let form = reqwest::multipart::Form::new().part(
        "source",
        reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
    );
    let res = common::as_superuser(
        f.server
            .client
            .put(f.server.url(&format!("/api/agents/{agent_id}/update"))),
        &f.uid(),
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);
    f.first_spec(agent_id).await;

    assert_eq!(f.state_mode(agent_id).await, "memory");
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_an_agent_revokes_its_state_credential() {
    let f = start().await;
    let agent_id = f.create_agent("doomed-agent", "1.0.0").await;
    let res = common::as_superuser(
        f.server.client.post(f.server.url("/api/containers")),
        &f.uid(),
        "admin",
    )
    .json(&json!({"image": "nasiko/doomed:1", "name": "doomed-agent"}))
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success());
    let token = f.first_spec(agent_id).await.env_vars["NASIKO_AGENT_TOKEN"].clone();
    assert_eq!(
        f.register_with(&token, json!(["kv"])).await,
        reqwest::StatusCode::NO_CONTENT
    );

    let res = common::as_superuser(
        f.server
            .client
            .delete(f.server.url(&format!("/api/agents/{agent_id}"))),
        &f.uid(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // The token row itself is tombstoned, not just hidden by the soft delete: undeleting the
    // agent must not bring the credential back.
    let revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM agent_state_tokens WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    assert!(revoked);
    assert_eq!(
        f.register_with(&token, json!(["kv"])).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    f.server.cleanup().await;
}

// ─── Credential rotation vs. a failed build ─────────────────────────────────

/// Both credentials (the gateway row in `agent_gateway_tokens`, the state row in
/// `agent_state_tokens`) as `(scope, token_hash, rotated_at)`; `rotated_at` is NULL until a
/// credential is first rotated.
async fn credential_rows(f: &Fixture, agent_id: Uuid) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT 'gateway' AS scope, token_hash, rotated_at::text FROM agent_gateway_tokens \
         WHERE agent_id = $1 \
         UNION ALL \
         SELECT 'state', token_hash, rotated_at::text FROM agent_state_tokens \
         WHERE agent_id = $1 ORDER BY scope",
    )
    .bind(agent_id)
    .fetch_all(&f.server.db)
    .await
    .unwrap()
}

/// Makes `agent_id` look like a running agent that was built before and holds both credentials
/// (and an older version, so a new upload or clone is not a version conflict); returns them.
async fn running_agent_credentials(
    f: &Fixture,
    agent_id: Uuid,
) -> Vec<(String, String, Option<String>)> {
    use nasiko_mcp_gateway::agent_tokens::{self, TokenScope};
    sqlx::query("UPDATE agents SET version = '0.9.0' WHERE id = $1")
        .bind(agent_id)
        .execute(&f.server.db)
        .await
        .unwrap();
    // A prior successful build is what a running agent has, and it makes a failed deploy mark the
    // agent failed instead of deleting it (and its credential rows with it).
    sqlx::query(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status) \
         VALUES ($1, '0.9.0', 'nasiko/old:1', 'success')",
    )
    .bind(agent_id)
    .execute(&f.server.db)
    .await
    .unwrap();
    agent_tokens::mint_scoped(&f.server.db, agent_id, TokenScope::Gateway)
        .await
        .unwrap();
    agent_tokens::mint_scoped(&f.server.db, agent_id, TokenScope::State)
        .await
        .unwrap();
    let rows = credential_rows(f, agent_id).await;
    assert_eq!(rows.len(), 2, "one row per scope: {rows:?}");
    rows
}

async fn wait_for_failed_build(f: &Fixture, agent_id: Uuid) {
    for _ in 0..200 {
        let failed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM agent_builds WHERE agent_id = $1 AND status = 'failed')",
        )
        .bind(agent_id)
        .fetch_one(&f.server.db)
        .await
        .unwrap();
        if failed {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the build of agent {agent_id} never failed");
}

/// The running pods hold the current credentials; minting new ones for an upload whose image
/// never builds would shorten their remaining validity for nothing.
#[tokio::test]
#[serial]
async fn a_failed_upload_build_leaves_the_running_agents_credentials_unrotated() {
    let f = start().await;
    let agent_id = f
        .agent_row("running-agent", Some("nasiko/old:1"), "running")
        .await;
    let before = running_agent_credentials(&f, agent_id).await;
    f.server.runtime.set_fail_build(true);

    assert_eq!(upload_zip(&f, "running-agent", vec![]).await, agent_id);
    wait_for_failed_build(&f, agent_id).await;

    assert_eq!(credential_rows(&f, agent_id).await, before);
    assert!(f.specs_for(agent_id).is_empty(), "nothing was deployed");
    f.server.cleanup().await;
}

/// Same guarantee for the clone pipeline (the legacy `Clone` job and the GitHub clone arm share
/// it): a build that fails leaves the running agent's credentials alone.
#[tokio::test]
#[serial]
async fn a_failed_clone_build_leaves_the_running_agents_credentials_unrotated() {
    let f = start().await;
    let agent_id = f
        .agent_row("running-cloned-agent", Some("nasiko/old:1"), "running")
        .await;
    let before = running_agent_credentials(&f, agent_id).await;
    f.server.runtime.set_fail_build(true);

    queue_clone_job(&f, agent_id, "running-cloned-agent").await;
    wait_for_failed_build(&f, agent_id).await;

    assert_eq!(credential_rows(&f, agent_id).await, before);
    assert!(f.specs_for(agent_id).is_empty(), "nothing was deployed");
    f.server.cleanup().await;
}

// ─── Platform keys win ──────────────────────────────────────────────────────

fn hostile_env() -> Value {
    json!({
        "MCP_GATEWAY_TOKEN": "user-gateway",
        "NASIKO_AGENT_TOKEN": "user-state",
        "NASIKO_STATE_URL": "http://evil.test/api/agent-state",
        "KEPT": "user-value",
    })
}

async fn deploy_ad_hoc_with(f: &Fixture, name: &str) -> DeploymentSpec {
    let agent_id = f.create_agent(name, "1.0.0").await;
    let res = common::as_superuser(
        f.server.client.post(f.server.url("/api/containers")),
        &f.uid(),
        "admin",
    )
    .json(&json!({"image": "nasiko/hostile:1", "name": name, "env": hostile_env()}))
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "deploy: {}", res.status());
    f.first_spec(agent_id).await
}

#[tokio::test]
#[serial]
async fn platform_keys_win_over_user_supplied_env_of_the_same_name() {
    let f = start().await;
    let spec = deploy_ad_hoc_with(&f, "hostile-agent").await;

    assert_platform_env(&spec);
    assert_eq!(spec.env_vars["KEPT"], "user-value", "other user env stays");
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn user_env_cannot_stand_in_for_a_credential_the_platform_could_not_mint() {
    // No MCP_GATEWAY_PUBLIC_URL: the state API has no address to hand out, so no state credential
    // is minted. The user's lookalikes must not slip through in its place.
    let f = start_with(None, None).await;
    let spec = deploy_ad_hoc_with(&f, "hostile-unconfigured-agent").await;

    assert!(!spec.env_vars.contains_key("NASIKO_AGENT_TOKEN"));
    assert!(!spec.env_vars.contains_key("NASIKO_STATE_URL"));
    assert_ne!(spec.env_vars["MCP_GATEWAY_TOKEN"], "user-gateway");
    assert_eq!(spec.env_vars["KEPT"], "user-value");
    f.server.cleanup().await;
}

// ─── build_jobs.payload ─────────────────────────────────────────────────────

async fn raw_payload(f: &Fixture, agent_id: Uuid) -> String {
    sqlx::query_scalar("SELECT payload::text FROM build_jobs WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&f.server.db)
        .await
        .expect("build job row")
}

#[tokio::test]
#[serial]
async fn a_queued_upload_job_holds_no_plaintext_credential_or_user_secret() {
    let f = start().await;
    let agent_id = upload_zip(
        &f,
        "sealed-agent",
        vec![(
            "env",
            json!({"USER_SECRET": "plain-user-secret-123"}).to_string(),
        )],
    )
    .await;

    let raw = raw_payload(&f, agent_id).await;
    assert!(
        !raw.contains("plain-user-secret-123"),
        "user secret in the clear: {raw}"
    );
    assert!(
        !raw.contains("ngt_") && !raw.contains("nst_"),
        "credential in the payload"
    );
    assert!(raw.contains("\"env_encrypted\": true"));
    // The worker still opens it: the secret reaches the container.
    let spec = f.first_spec(agent_id).await;
    assert_eq!(spec.env_vars["USER_SECRET"], "plain-user-secret-123");
    assert_platform_env(&spec);
    f.server.cleanup().await;
}

/// A row queued by the previous release: plaintext env, a credential the old handler minted, and
/// no `env_encrypted` field. Still in the table during an upgrade, and must still deploy.
#[tokio::test]
#[serial]
async fn a_legacy_plaintext_upload_job_is_still_processed() {
    let f = start().await;
    let agent_id = f.agent_row("legacy-agent", None, "deploying").await;
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'nasiko/legacy-agent:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    let zip_dir = std::env::temp_dir().join(format!("deploy-env-legacy-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&zip_dir).unwrap();
    let zip_path = zip_dir.join("source.zip");
    std::fs::write(&zip_path, dockerfile_zip()).unwrap();

    let payload = json!({
        "kind": "Upload",
        "build_id": build_id,
        "agent_id": agent_id,
        "owner_id": f.admin_id,
        "upload_id": build_id.to_string(),
        "name": "legacy-agent",
        "zip_path": zip_path,
        "image_tag": "nasiko/legacy-agent:1.0.0",
        "ports": [8000],
        "env": {"LEGACY_SETTING": "legacy-value", "MCP_GATEWAY_TOKEN": "ngt_stale_minted_at_enqueue"},
        "writable": false,
    });
    sqlx::query("INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)")
        .bind(agent_id)
        .bind(f.admin_id)
        .bind(&payload)
        .execute(&f.server.db)
        .await
        .unwrap();
    let _ = f.server.state.build_tx.send(()).await;

    let spec = f.first_spec(agent_id).await;
    assert_eq!(spec.env_vars["LEGACY_SETTING"], "legacy-value");
    assert_platform_env(&spec);
    assert_ne!(
        spec.env_vars["MCP_GATEWAY_TOKEN"], "ngt_stale_minted_at_enqueue",
        "the worker mints its own; a stale plaintext one in the row is not reused"
    );
    f.server.cleanup().await;
}

// ─── NAS-1313: the agent env QA checks that need no live infra ────────────────

/// A server that holds the platform's real OpenAI key, hands it out as a fallback, and has the
/// LLM router configured: the setup in which a non-OpenAI agent used to keep the real key.
async fn start_with_router_and_platform_key() -> Fixture {
    // SAFETY: tests run serially (#[serial]), so no concurrent env mutation.
    unsafe {
        std::env::set_var("AGENT_JWT_SECRET", "deploy-env-test-secret-0123456789");
        std::env::set_var("LLM_GATEWAY_BASE_URL", LLM_BASE);
    }
    start_configured(|config| {
        config.mcp_gateway_public_url = Some(GATEWAY_URL.to_owned());
        config.agent_platform_key_fallback = true;
        config.openai_api_key = Some(PLATFORM_KEY.to_owned());
    })
    .await
}

impl Fixture {
    async fn post(&self, path: &str, body: Value) -> reqwest::StatusCode {
        common::as_superuser(
            self.server.client.post(self.server.url(path)),
            &self.uid(),
            "admin",
        )
        .json(&body)
        .send()
        .await
        .unwrap()
        .status()
    }

    async fn set_secret(&self, agent_id: Uuid, name: &str, value: &str) {
        let status = self
            .post(
                &format!("/api/agents/{agent_id}/secrets"),
                json!({"name": name, "value": value}),
            )
            .await;
        assert!(status.is_success(), "set secret: {status}");
    }

    async fn delete_secret(&self, agent_id: Uuid, name: &str) {
        let res = common::as_superuser(
            self.server.client.delete(
                self.server
                    .url(&format!("/api/agents/{agent_id}/secrets/{name}")),
            ),
            &self.uid(),
            "admin",
        )
        .send()
        .await
        .unwrap();
        assert!(res.status().is_success(), "delete secret: {}", res.status());
    }

    /// `nasiko restart <name>`, and the spec it deployed.
    async fn admin_restart(&self, agent_id: Uuid, name: &str) -> DeploymentSpec {
        let seen = self.specs_for(agent_id).len();
        let status = self
            .post(&format!("/api/containers/{name}/restart"), json!({}))
            .await;
        assert!(status.is_success(), "restart: {status}");
        self.spec_after(agent_id, seen).await
    }
}

/// `(agent_id, owner_id)` from the agent JWT under `key`.
fn jwt_claims(spec: &DeploymentSpec, key: &str) -> (String, String) {
    let bearer = format!("Bearer {}", spec.env_vars[key]);
    nasiko_llm_router::auth::verify_agent_jwt(
        Some(&bearer),
        &nasiko_llm_router::GatewayConfig::from_env(),
    )
    .unwrap_or_else(|e| panic!("{key} is not a valid agent JWT: {e:?}"))
}

/// E-02 / E-03 (NAS-1258): an Anthropic or Gemini agent's own SDK vars point at the router with
/// the agent JWT, and so does its OpenAI pair; the platform's real key reaches no agent.
#[tokio::test]
#[serial]
async fn a_non_openai_agent_gets_its_own_router_vars_and_never_the_real_openai_key() {
    let cases: [(&str, &str, &[&str]); 2] = [
        ("anthropic", "ANTHROPIC_BASE_URL", &["ANTHROPIC_API_KEY"]),
        (
            "gemini",
            "GOOGLE_GEMINI_BASE_URL",
            &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        ),
    ];
    for (format, base_var, key_vars) in cases {
        let f = start_with_router_and_platform_key().await;
        let name = format!("{format}-agent");
        let agent_id = f.agent_row(&name, Some("nasiko/echo:1"), "running").await;
        sqlx::query("UPDATE agents SET inbound_format = $2 WHERE id = $1")
            .bind(agent_id)
            .bind(format)
            .execute(&f.server.db)
            .await
            .unwrap();

        let spec = f.admin_restart(agent_id, &name).await;

        assert_eq!(spec.env_vars[base_var], LLM_BASE, "{format}");
        for key in key_vars.iter().chain(&["OPENAI_API_KEY"]) {
            assert_eq!(
                jwt_claims(&spec, key).0,
                agent_id.to_string(),
                "{format} {key}"
            );
        }
        assert_eq!(spec.env_vars["OPENAI_BASE_URL"], format!("{LLM_BASE}/v1"));
        assert!(
            spec.env_vars.values().all(|v| v != PLATFORM_KEY),
            "{format}: the platform's real OpenAI key reached the agent"
        );
        f.server.cleanup().await;
    }
}

/// E-01 with the platform key held: the OpenAI-format agent's key is the JWT, not the real one.
#[tokio::test]
#[serial]
async fn an_openai_agent_gets_a_jwt_in_place_of_the_real_openai_key() {
    let f = start_with_router_and_platform_key().await;
    let agent_id = f
        .agent_row("openai-agent", Some("nasiko/echo:1"), "running")
        .await;

    let spec = f.admin_restart(agent_id, "openai-agent").await;

    assert_platform_env(&spec);
    assert_eq!(jwt_claims(&spec, "OPENAI_API_KEY").0, agent_id.to_string());
    assert!(spec.env_vars.contains_key("OPENAI_MODEL"));
    assert_eq!(spec.env_vars["PORT"], "8000");
    f.server.cleanup().await;
}

/// E-05: an agent's own `OPENAI_MODEL` secret beats the platform default.
#[tokio::test]
#[serial]
async fn an_openai_model_secret_overrides_the_platform_default() {
    let f = start().await;
    let agent_id = f
        .agent_row("model-agent", Some("nasiko/echo:1"), "running")
        .await;
    f.set_secret(agent_id, "OPENAI_MODEL", "agent-chosen-model")
        .await;

    let spec = f.admin_restart(agent_id, "model-agent").await;

    assert_eq!(spec.env_vars["OPENAI_MODEL"], "agent-chosen-model");
    f.server.cleanup().await;
}

/// E-12: a rotated secret reaches the agent on the next restart and the old value is gone; a
/// deleted one is gone too.
#[tokio::test]
#[serial]
async fn a_rotated_secret_replaces_the_old_value_on_restart() {
    let f = start().await;
    let agent_id = f
        .agent_row("rotating-agent", Some("nasiko/echo:1"), "running")
        .await;
    f.set_secret(agent_id, "PARTNER_KEY", "old-value").await;
    let spec = f.admin_restart(agent_id, "rotating-agent").await;
    assert_eq!(spec.env_vars["PARTNER_KEY"], "old-value");

    f.set_secret(agent_id, "PARTNER_KEY", "new-value").await;
    let spec = f.admin_restart(agent_id, "rotating-agent").await;
    assert_eq!(spec.env_vars["PARTNER_KEY"], "new-value");
    assert!(spec.env_vars.values().all(|v| v != "old-value"));

    f.delete_secret(agent_id, "PARTNER_KEY").await;
    let spec = f.admin_restart(agent_id, "rotating-agent").await;
    assert!(!spec.env_vars.contains_key("PARTNER_KEY"));
    f.server.cleanup().await;
}

/// E-07 / E-13: every redeploy mints a fresh JWT, and after an ownership transfer it carries the
/// new owner.
#[tokio::test]
#[serial]
async fn a_redeploy_after_an_ownership_transfer_mints_a_jwt_for_the_new_owner() {
    let f = start().await;
    let agent_id = f
        .agent_row("transferred-agent", Some("nasiko/echo:1"), "running")
        .await;
    let before = f.admin_restart(agent_id, "transferred-agent").await;
    assert_eq!(
        jwt_claims(&before, "OPENAI_API_KEY").1,
        f.admin_id.to_string()
    );

    let new_owner: Value = common::as_superuser(
        f.server.client.post(f.server.url("/api/users")),
        &f.uid(),
        "admin",
    )
    .json(&json!({"username": "new-owner", "email": "new-owner@test.local"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let new_owner_id = new_owner["id"].as_str().unwrap().to_owned();
    let res = common::as_superuser(
        f.server
            .client
            .put(f.server.url(&format!("/api/agents/{agent_id}/owner"))),
        &f.uid(),
        "admin",
    )
    .json(&json!({"new_owner_id": new_owner_id}))
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "transfer: {}", res.status());

    let after = f.admin_restart(agent_id, "transferred-agent").await;
    assert_eq!(jwt_claims(&after, "OPENAI_API_KEY").1, new_owner_id);
    assert_ne!(
        before.env_vars["OPENAI_API_KEY"], after.env_vars["OPENAI_API_KEY"],
        "a redeploy mints a new JWT"
    );
    f.server.cleanup().await;
}

/// E-09 (NAS-1259): on a runtime whose new instances read the env it holds (Kubernetes), a
/// scale-up rebuilds that env first, so a secret changed since the last deploy reaches the new
/// replicas, and the running replicas are rolled onto it as well.
#[tokio::test]
#[serial]
async fn a_scale_up_hands_the_runtime_the_current_secrets_and_rolls_the_running_replicas() {
    let f = start().await;
    f.server.runtime.set_restarts_in_place(true);
    let agent_id = f
        .agent_row("scaled-agent", Some("nasiko/echo:1"), "running")
        .await;
    f.set_secret(agent_id, "PARTNER_KEY", "changed-after-deploy")
        .await;

    let status = f
        .post("/api/containers/scaled-agent/scale", json!({"replicas": 3}))
        .await;
    assert!(status.is_success(), "scale: {status}");

    let container = ContainerId::from_uuid(agent_id);
    let env = f
        .server
        .runtime
        .last_refreshed_env(&container)
        .expect("the scale must hand the runtime a fresh env");
    assert_eq!(env["PARTNER_KEY"], "changed-after-deploy");
    assert!(env["MCP_GATEWAY_TOKEN"].starts_with("ngt_"));
    assert_eq!(env["PORT"], "8000");
    assert!(
        f.server.runtime.restarted().contains(&container),
        "running replicas hold credentials the refresh superseded; they must be rolled"
    );
    f.server.cleanup().await;
}

/// A stopped agent has no replicas to roll: the scale-up starts them all on the fresh env.
#[tokio::test]
#[serial]
async fn a_scale_up_from_stopped_refreshes_the_env_without_a_roll() {
    let f = start().await;
    f.server.runtime.set_restarts_in_place(true);
    f.server
        .runtime
        .set_reported_state(nasiko_runtime::RuntimeState::Stopped);
    let agent_id = f
        .agent_row("stopped-scaled-agent", Some("nasiko/echo:1"), "stopped")
        .await;

    let status = f
        .post(
            "/api/containers/stopped-scaled-agent/scale",
            json!({"replicas": 2}),
        )
        .await;
    assert!(status.is_success(), "scale: {status}");

    let container = ContainerId::from_uuid(agent_id);
    assert!(f.server.runtime.last_refreshed_env(&container).is_some());
    assert!(!f.server.runtime.restarted().contains(&container));
    f.server.cleanup().await;
}

/// Scaling to zero stops the agent and needs no env.
#[tokio::test]
#[serial]
async fn a_scale_to_zero_does_not_touch_the_env() {
    let f = start().await;
    f.server.runtime.set_restarts_in_place(true);
    let agent_id = f
        .agent_row("zeroed-agent", Some("nasiko/echo:1"), "running")
        .await;

    let status = f
        .post("/api/containers/zeroed-agent/scale", json!({"replicas": 0}))
        .await;
    assert!(status.is_success(), "scale: {status}");

    assert!(
        f.server
            .runtime
            .last_refreshed_env(&ContainerId::from_uuid(agent_id))
            .is_none()
    );
    f.server.cleanup().await;
}

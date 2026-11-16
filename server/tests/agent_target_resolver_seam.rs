//! MAF steps and the HITL `mcp_tool` resume nudge reach a parked agent through the server's
//! resolver, which wakes it; the default resolvers (no seam wired) refuse it with the reason.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_target_resolver_seam -- --test-threads=1

mod common;

use std::sync::Arc;

use axum::{Json, Router, routing::post};
use nasiko_hitl::{ResumeNotifier, RuntimeResumeNotifier, repo};
use nasiko_orchestrator::DirectAddress;
use nasiko_orchestrator::maf::llm::LlmClient;
use nasiko_orchestrator::maf::types::{MafDefinition, MafStep, StepOutcome};
use nasiko_orchestrator::maf::{LinearMafExecutor, MafExecutor, MafRunContext};
use nasiko_server::agent_resolver::ServerTargetResolver;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

struct Unmetered;

impl nasiko_types::agent_call::AgentCallMeter for Unmetered {
    fn begin(&self, _agent_id: Uuid) -> Box<dyn nasiko_types::agent_call::AgentCallHandle> {
        Box::new(nasiko_types::agent_call::UnmeteredCall)
    }
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// A stub agent that completes every A2A call with a text reply.
async fn stub_agent() -> String {
    async fn answer(Json(body): Json<Value>) -> Json<Value> {
        Json(json!({ "jsonrpc": "2.0", "id": body["id"], "result": {
            "id": "t1", "contextId": "ctx",
            "status": { "state": "TASK_STATE_COMPLETED" },
            "artifacts": [{ "parts": [{ "text": "done" }] }]
        }}))
    }
    serve(
        Router::new()
            .route("/", post(answer))
            .route("/jsonrpc", post(answer)),
    )
    .await
}

/// An OpenAI-compatible endpoint answering MAF's structured calls.
async fn fake_llm() -> String {
    async fn complete(Json(body): Json<Value>) -> Json<Value> {
        let schema = body["response_format"]["json_schema"]["name"]
            .as_str()
            .unwrap_or_default();
        let content = match schema {
            "execution_plan" => json!({
                "output_generation": "Summarize.",
                "steps": [{ "prompt": "Do your task.", "to_extract": "the outcome" }]
            }),
            "generated_prompt" => json!({ "prompt": "Do your task." }),
            "extraction_result" => json!({ "extracted_info": "extracted outcome" }),
            "final_output" => json!({ "final_output": "final answer" }),
            other => panic!("unexpected LLM call: {other}"),
        };
        Json(json!({
            "model": "fake-model",
            "choices": [{ "message": { "role": "assistant", "content": content.to_string() } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        }))
    }
    format!(
        "{}/v1",
        serve(Router::new().route("/v1/chat/completions", post(complete))).await
    )
}

async fn seed_user(server: &common::TestServer) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash, role) \
         VALUES ('seam-user', 'seam@t.io', '', 'member'::user_role) RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_parked_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, parking_policy, state_mode) \
         VALUES ($1, $2, 'x:1.0.0', 'parked', '', 'auto', 'durable') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn agent_status(server: &common::TestServer, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

struct MafRun {
    definition: MafDefinition,
    execution_id: Uuid,
    llm: LlmClient,
    flow_guard: Arc<nasiko_flow::FlowGuard>,
    meter: Arc<dyn nasiko_types::agent_call::AgentCallMeter>,
    client: reqwest::Client,
    user_id: Uuid,
}

impl MafRun {
    async fn new(server: &common::TestServer, user_id: Uuid, agent_id: Uuid) -> Self {
        let definition = MafDefinition {
            description: None,
            steps: vec![MafStep {
                step_id: Uuid::new_v4(),
                step_index: 0,
                agent_id,
                agent_name: "parked-step".into(),
                agent_endpoint: String::new(),
                task_description: "Do the thing".into(),
                ext: Default::default(),
            }],
            output_generation: None,
        };
        let maf_json = serde_json::to_string(&definition).unwrap();
        let maf_id: Uuid = sqlx::query_scalar(
            "INSERT INTO mafs (user_id, name, maf_json) VALUES ($1, 'seam', $2::jsonb) RETURNING id",
        )
        .bind(user_id)
        .bind(&maf_json)
        .fetch_one(&server.db)
        .await
        .unwrap();
        let execution_id: Uuid = sqlx::query_scalar(
            "INSERT INTO maf_executions (maf_id, user_id, status, max_attempts, maf_json) \
             VALUES ($1, $2, 'running', 3, $3::jsonb) RETURNING id",
        )
        .bind(maf_id)
        .bind(user_id)
        .bind(&maf_json)
        .fetch_one(&server.db)
        .await
        .unwrap();
        let client = reqwest::Client::new();
        Self {
            definition,
            execution_id,
            llm: LlmClient::new(
                client.clone(),
                "k".into(),
                Some(fake_llm().await),
                "fake-model".into(),
            ),
            flow_guard: Arc::new(nasiko_flow::FlowGuard::new(
                nasiko_redis::SharedRedis::new(
                    redis::Client::open("redis://127.0.0.1:6379").unwrap(),
                ),
                nasiko_flow::FlowConfig::default(),
            )),
            meter: Arc::new(Unmetered),
            client,
            user_id,
        }
    }

    async fn run(
        &self,
        server: &common::TestServer,
        targets: &dyn nasiko_orchestrator::AgentTargetResolver,
    ) -> Result<StepOutcome, nasiko_orchestrator::maf::types::RunError> {
        let ctx = MafRunContext {
            client: &self.client,
            agent_calls: &self.meter,
            targets,
            outbound: &nasiko_orchestrator::outbound::NoOutboundPrefix,
            clock: nasiko_types::agent_clock::AgentClock::OFF,
            db: &server.db,
            flow_guard: &self.flow_guard,
            llm: &self.llm,
            execution_id: self.execution_id,
            user_id: self.user_id,
        };
        LinearMafExecutor
            .run(&ctx, &self.definition, Some("please"))
            .await
    }
}

#[tokio::test]
#[serial]
async fn a_maf_step_wakes_a_parked_agent_through_the_server_resolver() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let user = seed_user(&server).await;
    let agent = seed_parked_agent(&server, user, "maf-parked").await;
    wake.serve_at(agent, &stub_agent().await);
    let run = MafRun::new(&server, user, agent).await;

    let resolver = ServerTargetResolver::new(server.state.clone());
    let outcome = run.run(&server, &resolver).await;

    match outcome {
        Ok(StepOutcome::Completed(result)) => assert_eq!(result.output, "final answer"),
        other => panic!("expected a finished run: {:?}", other.err()),
    }
    assert_eq!(wake.woken_count(agent), 1);
    assert_eq!(agent_status(&server, agent).await, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn without_the_seam_a_maf_step_refuses_a_parked_agent_with_the_reason() {
    let server = common::TestServer::start().await;
    let wake = common::RowWake::install(&server);
    let user = seed_user(&server).await;
    let agent = seed_parked_agent(&server, user, "maf-parked-direct").await;
    wake.serve_at(agent, "http://never.reached:8000");
    let run = MafRun::new(&server, user, agent).await;

    let direct = DirectAddress::new(server.db.clone());
    let err = run.run(&server, &direct).await.err().expect("a failed run");

    assert!(err.to_string().contains("parked"), "{err}");
    assert_eq!(wake.woken_count(agent), 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_hitl_resume_nudge_wakes_a_parked_agent() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = seed_user(&server).await;
    let agent = seed_parked_agent(&server, owner, "nudge-parked").await;
    wake.serve_at(agent, &stub_agent().await);
    let context_id = "ses_nudge_1";
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 't')")
        .bind(context_id)
        .bind(owner)
        .execute(&server.db)
        .await
        .unwrap();
    let created = repo::create_pending_tool_approval(
        &server.db,
        repo::NewToolApproval {
            agent_id: agent,
            owner_user_id: owner,
            connector_id: Uuid::new_v4(),
            tool_name: "GITHUB_DELETE_REPO".into(),
            context_id: context_id.into(),
            question: json!({"tool_name": "GITHUB_DELETE_REPO"}),
        },
    )
    .await
    .unwrap();
    repo::resolve(
        &server.db,
        created.id,
        repo::ResolveDecision::Approve,
        owner,
        json!({"decision": "approve"}),
    )
    .await
    .unwrap()
    .expect("row was pending");
    let claimed = repo::claim_for_resume(&server.db)
        .await
        .unwrap()
        .expect("the resolved row is claimable");

    let notifier = RuntimeResumeNotifier::new(
        server.db.clone(),
        server.state.runtime.clone(),
        reqwest::Client::new(),
        600,
    )
    .with_resolver(Arc::new(ServerTargetResolver::new(server.state.clone())));
    notifier
        .notify(&claimed)
        .await
        .expect("the nudge is delivered");

    assert_eq!(wake.woken_count(agent), 1);
    assert_eq!(agent_status(&server, agent).await, "running");
    server.cleanup().await;
}

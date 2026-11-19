//! A retried MAF step reopens the flow its last attempt closed, so its agent's model and tool
//! calls are authorized again; a flow row of another user's under the same id stays closed.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test maf_step_flow_retry -- --test-threads=1

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::post};
use nasiko_orchestrator::maf::llm::LlmClient;
use nasiko_orchestrator::maf::types::{MafDefinition, MafStep, RunError, StepOutcome};
use nasiko_orchestrator::maf::{LinearMafExecutor, MafExecutor, MafRunContext};
use nasiko_orchestrator::target::ForwardTarget;
use nasiko_orchestrator::{AgentTargetResolver, ResolveCaller, ResolveFailure};
use serde_json::{Value, json};
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

/// The MCP gateway's and the LLM router's test for a live flow (`mcp/identity.rs`,
/// `llm-router/src/resolver`), with the default flow timeout.
const LIVE_FLOW: &str = "SELECT EXISTS(SELECT 1 FROM flows WHERE flow_id = $1 AND status = 'running' \
                         AND created_at > now() - make_interval(secs => 600))";

struct Unmetered;

impl nasiko_types::agent_call::AgentCallMeter for Unmetered {
    fn begin(&self, _agent_id: Uuid) -> Box<dyn nasiko_types::agent_call::AgentCallHandle> {
        Box::new(nasiko_types::agent_call::UnmeteredCall)
    }
}

/// Sends every step to one address.
struct At(String);

#[async_trait]
impl AgentTargetResolver for At {
    async fn resolve(
        &self,
        _agent_id: Uuid,
        _caller: &ResolveCaller,
        _context_id: Option<&str>,
    ) -> Result<ForwardTarget, ResolveFailure> {
        Ok(ForwardTarget::from(self.0.clone()))
    }
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// What the stub agent saw on each call: whether the flow its traceparent names was live.
#[derive(Clone)]
struct Agent {
    db: PgPool,
    seen_live: Arc<Mutex<Vec<bool>>>,
    fail_first: bool,
}

/// A stub agent that records, per call, whether the gateway would authorize its calls. With
/// `fail_first`, its first call answers 500 (a failed step, not sent again).
async fn stub_agent(agent: Agent) -> String {
    async fn answer(
        State(agent): State<Agent>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        let trace_id = headers
            .get("traceparent")
            .and_then(|v| v.to_str().ok())
            .and_then(|tp| tp.split('-').nth(1))
            .unwrap_or_default()
            .to_string();
        let live: bool = sqlx::query_scalar(LIVE_FLOW)
            .bind(&trace_id)
            .fetch_one(&agent.db)
            .await
            .unwrap();
        let call = {
            let mut seen = agent.seen_live.lock().unwrap();
            seen.push(live);
            seen.len()
        };
        if agent.fail_first && call == 1 {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Json(json!({ "jsonrpc": "2.0", "id": body["id"], "result": {
            "id": "t1", "contextId": "ctx",
            "status": { "state": "TASK_STATE_COMPLETED" },
            "artifacts": [{ "parts": [{ "text": "done" }] }]
        }}))
        .into_response()
    }
    serve(
        Router::new()
            .route("/", post(answer))
            .route("/jsonrpc", post(answer))
            .with_state(agent),
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

async fn seed_user(db: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash, role) \
         VALUES ($1, $1 || '@t.io', '', 'member'::user_role) RETURNING id",
    )
    .bind(name)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn seed_agent(db: &PgPool, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url) \
         VALUES ('retry-step', $1, 'x:1.0.0', 'running', '') RETURNING id",
    )
    .bind(owner)
    .fetch_one(db)
    .await
    .unwrap()
}

/// One single-step run, executed as many times as a test retries it.
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
    async fn new(db: &PgPool, user_id: Uuid, agent_id: Uuid) -> Self {
        let definition = MafDefinition {
            description: None,
            steps: vec![MafStep {
                step_id: Uuid::new_v4(),
                step_index: 0,
                agent_id,
                agent_name: "retry-step".into(),
                agent_endpoint: String::new(),
                task_description: "Do the thing".into(),
                ext: Default::default(),
            }],
            output_generation: None,
        };
        let maf_json = serde_json::to_string(&definition).unwrap();
        let maf_id: Uuid = sqlx::query_scalar(
            "INSERT INTO mafs (user_id, name, maf_json) VALUES ($1, 'retry', $2::jsonb) RETURNING id",
        )
        .bind(user_id)
        .bind(&maf_json)
        .fetch_one(db)
        .await
        .unwrap();
        let execution_id: Uuid = sqlx::query_scalar(
            "INSERT INTO maf_executions (maf_id, user_id, status, max_attempts, maf_json) \
             VALUES ($1, $2, 'running', 3, $3::jsonb) RETURNING id",
        )
        .bind(maf_id)
        .bind(user_id)
        .bind(&maf_json)
        .fetch_one(db)
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

    async fn attempt(&self, db: &PgPool, agent_url: &str) -> Result<StepOutcome, RunError> {
        let targets = At(agent_url.to_string());
        let ctx = MafRunContext {
            client: &self.client,
            agent_calls: &self.meter,
            targets: &targets,
            outbound: &nasiko_orchestrator::outbound::NoOutboundPrefix,
            clock: nasiko_types::agent_clock::AgentClock::OFF,
            db,
            flow_guard: &self.flow_guard,
            llm: &self.llm,
            execution_id: self.execution_id,
            user_id: self.user_id,
        };
        LinearMafExecutor
            .run(&ctx, &self.definition, Some("please"))
            .await
    }

    /// The flow the step's agent calls under (one per step, so the only one of this run).
    async fn step_flow(&self, db: &PgPool) -> (String, String) {
        sqlx::query_as("SELECT flow_id, status FROM flows WHERE session_id = $1")
            .bind(self.execution_id.to_string())
            .fetch_one(db)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[serial]
async fn a_retried_step_runs_under_a_live_flow_again() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server.db, "retry-user").await;
    let agent_id = seed_agent(&server.db, user).await;
    let seen_live = Arc::new(Mutex::new(Vec::new()));
    let agent = stub_agent(Agent {
        db: server.db.clone(),
        seen_live: seen_live.clone(),
        fail_first: true,
    })
    .await;
    let run = MafRun::new(&server.db, user, agent_id).await;

    run.attempt(&server.db, &agent)
        .await
        .err()
        .expect("the first attempt fails at the agent");
    let (flow_id, status) = run.step_flow(&server.db).await;
    assert_eq!(status, "completed", "a finished attempt closes its flow");
    // A retry can come long after the first attempt (a worker taking over a stalled run).
    sqlx::query("UPDATE flows SET created_at = now() - interval '1 hour' WHERE flow_id = $1")
        .bind(&flow_id)
        .execute(&server.db)
        .await
        .unwrap();

    match run.attempt(&server.db, &agent).await {
        Ok(StepOutcome::Completed(result)) => assert_eq!(result.output, "final answer"),
        other => panic!("expected the retry to finish: {:?}", other.err()),
    }

    assert_eq!(
        *seen_live.lock().unwrap(),
        vec![true, true],
        "the agent's calls are authorized on the retry too"
    );
    assert_eq!(
        run.step_flow(&server.db).await,
        (flow_id, "completed".into())
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_flow_of_another_users_under_the_steps_id_stays_closed() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server.db, "retry-owner").await;
    let other = seed_user(&server.db, "retry-other").await;
    let agent_id = seed_agent(&server.db, user).await;
    let seen_live = Arc::new(Mutex::new(Vec::new()));
    let agent = stub_agent(Agent {
        db: server.db.clone(),
        seen_live: seen_live.clone(),
        fail_first: false,
    })
    .await;
    let run = MafRun::new(&server.db, user, agent_id).await;
    // The id the step's first call will carry (`build_traceparent`: UUID v5 of the execution and
    // the step index), already taken by someone else's closed flow.
    let flow_id = Uuid::new_v5(&run.execution_id, b"0").simple().to_string();
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, session_id) \
         VALUES ($1, $2, 'x', 'x', 'completed', 'elsewhere')",
    )
    .bind(&flow_id)
    .bind(other)
    .execute(&server.db)
    .await
    .unwrap();

    let _ = run.attempt(&server.db, &agent).await;

    assert_eq!(*seen_live.lock().unwrap(), vec![false]);
    let (owner, status): (Uuid, String) =
        sqlx::query_as("SELECT user_id, status FROM flows WHERE flow_id = $1")
            .bind(&flow_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!((owner, status.as_str()), (other, "completed"));
    server.cleanup().await;
}

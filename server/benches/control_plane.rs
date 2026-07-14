//! Latency-regression benchmarks for the OSS control-plane server APIs:
//! agent-registry CRUD, the direct agent proxy, and orchestrator dispatch.
//!
//! Self-contained — creates a throwaway Postgres database, runs migrations,
//! and stands up the real server in-process against a `SimulatedRuntime` +
//! in-process sim agent + in-process mock LLM (see `nasiko-bench-support`).
//! No Docker/Kubernetes, no real LLM cost. This measures single-request
//! latency for regression tracking (`cargo bench -- --save-baseline <name>`),
//! not concurrent load — see `oss/bench` (Goose) for throughput/load testing.
//!
//! Prerequisites: `docker compose -f docker-compose.infra.yml up -d`
//! (Postgres + Redis). Run: `cargo bench -p nasiko-server`.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use nasiko_bench_support::{config, mock_llm, seed, server, sim_agent};
use nasiko_runtime::{ContainerRuntime, SimulatedRuntime};
use uuid::Uuid;

const SEED_USERS: u32 = 5;
const SEED_AGENTS: u32 = 10; // stays under router_shortlist_threshold (15)
// Generously oversized: at ~1ms/iteration, a 1s warm-up + 10s measurement
// window can consume several thousand setup() calls. Exhausting this panics
// with a clear message rather than silently reusing/erroring, so raise it if
// that ever happens.
const DELETE_POOL_SIZE: usize = 20_000;

struct Harness {
    db: server::BenchDb,
    srv: server::ServerHandle,
    token: String,
    agent_id: Uuid,
    delete_pool: RefCell<Vec<Uuid>>,
}

async fn setup() -> Harness {
    let sim = sim_agent::spawn_sim_agent().await;
    let llm = mock_llm::spawn_mock_llm().await;

    let db = server::BenchDb::create().await;
    let cfg = config::build_bench_config(db.database_url.clone(), &llm.base_url);

    let runtime: Arc<dyn ContainerRuntime> = Arc::new(SimulatedRuntime::new(sim.base_url.clone()));

    let manifest = seed::seed(
        &db.pool,
        SEED_USERS,
        SEED_AGENTS,
        &sim.base_url,
        config::BENCH_JWT_SECRET,
        &runtime,
    )
    .await;

    let owner_id = manifest.users[0].id;
    let delete_pool = create_delete_pool(&db.pool, owner_id, DELETE_POOL_SIZE).await;

    let auth: Arc<dyn nasiko_auth::AuthService> =
        Arc::new(nasiko_auth::AuthServiceImpl::new(db.pool.clone(), config::BENCH_JWT_SECRET.to_string()));

    let srv = server::start_server(cfg, db.pool.clone(), runtime, auth, |state| async move {
        nasiko_server::build_app(state, server::not_found)
    })
    .await;

    Harness {
        db,
        srv,
        token: manifest.users[0].token.clone(),
        agent_id: manifest.agents[0].id,
        delete_pool: RefCell::new(delete_pool),
    }
}

/// Pre-created throwaway agents for the `delete` benchmark — delete is
/// destructive, so each iteration needs a distinct row. Criterion's
/// `iter_batched` setup closure must be synchronous, so this pool is built
/// once up front (async, untimed) rather than per-iteration.
///
/// `status = 'stopped'` (not `'running'`) deliberately — `can_manage_agent`
/// (delete's authz check) is owner-based, not status-based, so these never
/// need to look "active". Marking them `'running'` would leak them into
/// `AgentSelector::fetch_active_agents` (used by orchestrator dispatch to
/// build its tool list), and since they were never `runtime.deploy()`ed into
/// `SimulatedRuntime`, each would fail its live-endpoint lookup and get
/// flipped to `'stopped'` by `resolve_endpoint`'s fallback — silently
/// polluting the orchestrator benchmark with hundreds of failed lookups.
async fn create_delete_pool(pool: &sqlx::PgPool, owner_id: Uuid, n: usize) -> Vec<Uuid> {
    let names: Vec<String> = (0..n).map(|i| format!("bench-delete-target-{i}")).collect();
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status, is_public)
         SELECT unnest($1::text[]), $2, 'http://127.0.0.1:1', 'stopped', false
         RETURNING id",
    )
    .bind(&names)
    .bind(owner_id)
    .fetch_all(pool)
    .await
    .expect("seed delete-pool agents")
}

async fn teardown(harness: Harness) {
    harness.db.drop_db().await;
}

fn bench_control_plane(c: &mut Criterion) {
    // Current-thread runtime: benchmarks are single-request-at-a-time by
    // design (see module docs), and it sidesteps `Send` requirements the
    // `RefCell`-based delete pool would otherwise trip over under a
    // multi-thread runtime moving the benchmark future between worker threads.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let harness = rt.block_on(setup());

    let mut catalog = c.benchmark_group("catalog_list");
    catalog.sample_size(20).measurement_time(Duration::from_secs(10));
    catalog.bench_function("get_agents", |b| {
        b.to_async(&rt).iter(|| async {
            let resp = harness
                .srv
                .client
                .get(harness.srv.url("/api/agents"))
                .bearer_auth(&harness.token)
                .send()
                .await
                .expect("catalog_list request");
            criterion::black_box(resp.status());
        });
    });
    catalog.finish();

    let mut proxy = c.benchmark_group("agent_proxy_chat");
    proxy.sample_size(20).measurement_time(Duration::from_secs(10));
    proxy.bench_function("post_agent", |b| {
        b.to_async(&rt).iter(|| async {
            let body = nasiko_types::a2a::build_send_request("What is the weather like today?", None);
            let resp = harness
                .srv
                .client
                .post(harness.srv.url(&format!("/api/agents/{}", harness.agent_id)))
                .bearer_auth(&harness.token)
                .json(&body)
                .send()
                .await
                .expect("agent_proxy request");
            criterion::black_box(resp.status());
        });
    });
    proxy.finish();

    let mut orchestrator = c.benchmark_group("orchestrator_a2a");
    orchestrator.sample_size(20).measurement_time(Duration::from_secs(10));
    orchestrator.bench_function("post_orchestrator", |b| {
        b.to_async(&rt).iter(|| async {
            let body =
                nasiko_types::a2a::build_send_request("Summarize the latest agent activity for me.", None);
            let resp = harness
                .srv
                .client
                .post(harness.srv.url("/api/orchestrator/a2a"))
                .bearer_auth(&harness.token)
                .json(&body)
                .send()
                .await
                .expect("orchestrator request");
            criterion::black_box(resp.status());
        });
    });
    orchestrator.finish();

    let mut crud = c.benchmark_group("agent_registry_crud");
    crud.sample_size(20).measurement_time(Duration::from_secs(10));
    crud.bench_function("create", |b| {
        b.to_async(&rt).iter_batched(
            || format!("bench-crud-create-{}", Uuid::new_v4()),
            |name| {
                let harness = &harness;
                async move {
                    let resp = harness
                        .srv
                        .client
                        .post(harness.srv.url("/api/agents"))
                        .bearer_auth(&harness.token)
                        .json(&serde_json::json!({ "name": name }))
                        .send()
                        .await
                        .expect("create request");
                    criterion::black_box(resp.status());
                }
            },
            BatchSize::SmallInput,
        );
    });
    crud.bench_function("get_one", |b| {
        b.to_async(&rt).iter(|| async {
            let resp = harness
                .srv
                .client
                .get(harness.srv.url(&format!("/api/agents/{}", harness.agent_id)))
                .bearer_auth(&harness.token)
                .send()
                .await
                .expect("get_one request");
            criterion::black_box(resp.status());
        });
    });
    crud.bench_function("update", |b| {
        b.to_async(&rt).iter(|| async {
            let resp = harness
                .srv
                .client
                .put(harness.srv.url(&format!("/api/agents/{}", harness.agent_id)))
                .bearer_auth(&harness.token)
                .json(&serde_json::json!({ "description": "bench update" }))
                .send()
                .await
                .expect("update request");
            criterion::black_box(resp.status());
        });
    });
    crud.bench_function("delete", |b| {
        b.to_async(&rt).iter_batched(
            || {
                harness
                    .delete_pool
                    .borrow_mut()
                    .pop()
                    .expect("delete pool exhausted — raise DELETE_POOL_SIZE")
            },
            |id| {
                let harness = &harness;
                async move {
                    let resp = harness
                        .srv
                        .client
                        .delete(harness.srv.url(&format!("/api/agents/{id}")))
                        .bearer_auth(&harness.token)
                        .send()
                        .await
                        .expect("delete request");
                    criterion::black_box(resp.status());
                }
            },
            BatchSize::SmallInput,
        );
    });
    crud.finish();

    rt.block_on(teardown(harness));
}

criterion_group! {
    name = benches;
    // Default 3s warm-up is tuned for microsecond-scale operations; ours are
    // ms-scale (network + DB), so a shorter warm-up keeps total run time
    // reasonable without meaningfully hurting measurement stability.
    config = Criterion::default().warm_up_time(Duration::from_secs(1));
    targets = bench_control_plane
}
criterion_main!(benches);

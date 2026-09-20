//! Owns the full lifecycle of the server-under-benchmark, mirroring
//! `oss/server/tests/common::TestServer` (real `TcpListener::bind` + real
//! `reqwest::Client`, not `tower::Service::oneshot`) — one throwaway Postgres
//! database created per bench run and dropped at teardown, migrations run
//! explicitly rather than relying on a pre-migrated template.

use std::future::Future;
use std::sync::Arc;

use nasiko_config::Config;
use nasiko_runtime::ContainerRuntime;
use nasiko_server::state::AppState;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::config::pg_admin_url;

/// A throwaway Postgres database for one bench run. Call `drop_db().await`
/// once benchmarking is done.
///
/// If a bench run panics or is killed before reaching teardown, its database
/// leaks (named `nasiko_bench_<uuid>`) — clean up manually:
/// `psql -c "SELECT datname FROM pg_database WHERE datname LIKE 'nasiko_bench_%'"`
/// then drop each. Not worth `catch_unwind`-wrapping the benchmark body for —
/// once the delete pool is sized generously (see `DELETE_POOL_SIZE`), panics
/// here should be rare.
pub struct BenchDb {
    pub pool: PgPool,
    pub database_url: String,
    db_name: String,
    admin_pool: PgPool,
}

fn minimal_pool_opts() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(10)
        .min_connections(0)
        .idle_timeout(std::time::Duration::from_secs(5))
        .acquire_timeout(std::time::Duration::from_secs(30))
}

impl BenchDb {
    pub async fn create() -> Self {
        let admin_url = pg_admin_url();
        let db_name = format!("nasiko_bench_{}", Uuid::new_v4().simple());

        let admin_pool = minimal_pool_opts()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("connect to postgres — is infra up? (set BENCH_PG_URL to override)");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin_pool)
            .await
            .expect("create bench database");

        let base = admin_url.rsplitn(2, '/').last().unwrap_or(&admin_url);
        let database_url = format!("{base}/{db_name}");

        let pool = minimal_pool_opts()
            .connect(&database_url)
            .await
            .expect("connect to bench database");

        AppState::run_migrations(&pool).await;

        BenchDb {
            pool,
            database_url,
            db_name,
            admin_pool,
        }
    }

    pub async fn drop_db(&self) {
        sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
            self.db_name
        ))
        .execute(&self.admin_pool)
        .await
        .ok();

        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
            .execute(&self.admin_pool)
            .await
            .ok();

        self.pool.close().await;
        self.admin_pool.close().await;
    }
}

/// A running server bound to a random localhost port.
pub struct ServerHandle {
    pub base_url: String,
    pub client: reqwest::Client,
}

impl ServerHandle {
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

/// Fallback handler for `build_app`/`build_ee_app` — benches never hit the
/// static-asset path, so 404 is fine. Function items are implicitly `Clone`,
/// so this can be passed directly wherever a `Handler<T, ()> + Clone` is
/// expected.
pub async fn not_found() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
}

/// Builds `AppState`, hands it to `build` (either `nasiko_server::build_app`
/// or `nasiko_server_ee::build_ee_app`, wrapped by the caller), and binds it
/// to a random localhost port. `auth` is caller-supplied rather than built
/// here since OSS and EE need different `AuthService` impls
/// (`AuthServiceImpl` vs `EeAuthService`).
pub async fn start_server<F, Fut>(
    config: Config,
    db: PgPool,
    runtime: Arc<dyn ContainerRuntime>,
    auth: Arc<dyn nasiko_auth::AuthService>,
    build: F,
) -> ServerHandle
where
    F: FnOnce(AppState) -> Fut,
    Fut: Future<Output = axum::Router>,
{
    let state = AppState::from_config_with_db(config, auth, runtime, db).await;
    let app = build(state).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind bench server");
    let port = listener
        .local_addr()
        .expect("bench server local_addr")
        .port();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Defensive — mirrors `ee/server/tests/common::TestServer::start` — the
    // accept loop needs a tick to actually start polling the listener.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    ServerHandle {
        base_url: format!("http://127.0.0.1:{port}"),
        client: reqwest::Client::new(),
    }
}

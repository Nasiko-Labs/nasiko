use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use nasiko_server::spa::{self, Spa};
use nasiko_server::telemetry::{TelemetryConfig, init_telemetry};
use rust_embed::Embed;

// `NASIKO_UI` is resolved by build.rs — see the comment there for why this
// path cannot be a literal (this crate sits at a different depth in the public
// repo, where the `oss/` prefix is stripped).
//
// One folder, not an overlay chain: a Vite build is self-contained. It carries
// `index.html`, the hashed `assets/`, and the `routes.json` / `csp.json`
// sidecars that `nasiko_server::spa` reads at startup.
#[derive(Embed)]
#[folder = "$NASIKO_UI/oss/app/dist/"]
struct OssAssets;

/// `depends_on: condition: service_healthy` guarantees Postgres itself is
/// ready, but the container's own DNS resolution can still have a brief
/// post-start hiccup unrelated to Postgres's readiness — especially under
/// alternative Docker backends (OrbStack, Colima) — surfacing as a
/// "temporary failure in name resolution" rather than a connection refusal.
/// Retrying here absorbs that instead of crashing the whole server on a
/// transient blip.
async fn connect_to_postgres_with_retry(database_url: &str, max_connections: u32) -> sqlx::PgPool {
    const MAX_ATTEMPTS: u32 = 10;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(database_url)
            .await
        {
            Ok(pool) => return pool,
            Err(e) => {
                tracing::warn!(
                    attempt,
                    max_attempts = MAX_ATTEMPTS,
                    error = %e,
                    "failed to connect to postgres, retrying"
                );
                last_err = Some(e);
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }
    panic!(
        "failed to connect to postgres after {MAX_ATTEMPTS} attempts: {}",
        last_err.expect("loop always sets last_err before exhausting attempts")
    );
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    // Explicitly select ring as the Rustls crypto provider (the workspace
    // convention). Required because sqlx/reqwest (ring) and the AWS SDK's HTTP
    // client (aws-lc-rs) both pull in rustls, and rustls panics at first use if
    // no provider is installed when multiple are compiled in — the redis client
    // builds its rediss:// config through the process-default provider.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // First, so a panic anywhere in startup is reported too. Held until `main` returns.
    let _error_reporting = nasiko_server::error_reporting::init(
        nasiko_server::error_reporting::ErrorReportingConfig::from_env(),
        "oss",
    );
    let telemetry_config = TelemetryConfig::from_env();
    init_telemetry(&telemetry_config);

    let config = nasiko_config::Config::from_env()
        .unwrap_or_else(|e| nasiko_config::exit_invalid_config(format!("{e:#}")));
    if let Err(e) = config.validate_secrets_key() {
        nasiko_config::exit_invalid_config(format!("SECRETS_ENCRYPTION_KEY: {e}"));
    }
    config.warn_unset_optional_keys();
    let bind = config.bind.clone();

    // Build DB pool early so it can be shared with auth services. Raised from sqlx's
    // default of 10 — load testing showed 10 saturates under a few hundred concurrent
    // requests (server CPU stays idle while sqlx's own acquire-timeout logs show
    // requests queuing tens of seconds for a connection).
    let db =
        connect_to_postgres_with_retry(&config.database_url, config.database_max_connections).await;

    let jwt_secret = nasiko_utils::required_env("JWT_SECRET")
        .unwrap_or_else(|e| nasiko_config::exit_invalid_config(e));
    let auth: Arc<dyn nasiko_auth::AuthService> =
        Arc::new(nasiko_auth::AuthServiceImpl::new(db.clone(), jwt_secret));

    // Built before the runtime because the Docker runtime's `ImageSource` reads
    // the same store the registry writes; one instance, handed to both.
    //
    // This edition ships the S3-compatible backend only. A provider it cannot
    // serve must stop the boot rather than fall through to S3, which would
    // write every artifact to a store the operator did not ask for and only
    // surface once the intended one turned out to be empty.
    if !nasiko_config::uses_s3_storage(&config.storage_provider) {
        panic!(
            "STORAGE_PROVIDER={} is not available in this edition, which ships the \
             S3-compatible object store only. Leave STORAGE_PROVIDER unset or set it to 's3'.",
            config.storage_provider
        );
    }
    let oci_storage: Arc<dyn nasiko_runtime::BlobStore> =
        Arc::new(nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await);

    let runtime: Arc<dyn nasiko_runtime::ContainerRuntime> = match config.agent_runtime.as_str() {
        "simulated" => {
            let sim_agent_url =
                std::env::var("SIM_AGENT_URL").unwrap_or_else(|_| "http://localhost:8000".into());
            Arc::new(nasiko_runtime::SimulatedRuntime::new(sim_agent_url))
        }
        _ => Arc::new(
            nasiko_server::runtime::build_docker_runtime(&config, db.clone(), oci_storage.clone())
                .await
                .expect("failed to create Docker runtime"),
        ),
    };

    let image_source: Arc<dyn nasiko_runtime::ImageSource> = Arc::new(nasiko_oci::OciState::new(
        db.clone(),
        oci_storage.clone(),
        config.agent_image_registry.clone(),
    ));
    let docker_sandbox =
        nasiko_server::sandbox_wiring::connect_docker_sandbox(&config, Some(image_source)).await;
    let sandbox = nasiko_server::sandbox_wiring::build_sandbox_provider(&config, docker_sandbox);

    nasiko_server::state::AppState::run_migrations(&db).await;
    let agent_scaling = Arc::new(nasiko_server::agent_scaling::SingleReplica::new(
        runtime.clone(),
        db.clone(),
    ));
    let state = nasiko_server::state::AppState::from_config_with_sandbox(
        config,
        auth,
        runtime,
        oci_storage,
        db,
        agent_scaling,
        sandbox,
    )
    .await;
    state.init().await;
    // Clear stale coding-agent session leases left by a prior crash, then keep
    // reconciling session rows against live containers in the background.
    nasiko_server::coding::spawn_reconciler(state.clone());
    // Suspend harness sandboxes that have gone quiet.
    nasiko_server::sessions::spawn_reaper(state.clone());
    // Purge expired session-token revocations so the table only holds tokens that
    // could still verify (the turn path records them; they self-expire anyway).
    tokio::spawn(nasiko_server::coding::revocation::sweep_loop(
        state.db.clone(),
    ));
    let shutdown = state.shutdown.clone();
    let drain_delay = std::time::Duration::from_secs(state.config.shutdown_drain_delay_secs);
    let grace = std::time::Duration::from_secs(state.config.shutdown_grace_secs);
    tokio::spawn(nasiko_server::shutdown::drain_on_terminate(
        shutdown.clone(),
        drain_delay,
    ));
    let app = nasiko_server::build_app(state, static_handler);

    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    tracing::info!("nasiko-server (OSS) listening on {bind}");
    // Peer addresses (for per-address limits), an idle-connection limit and a drain on SIGTERM:
    // see `serve` and `shutdown`.
    nasiko_server::serve::serve(listener, app, shutdown, grace).await;
    tracing::info!("nasiko-server stopped");
}

/// The open-source shell loads no third-party script; its inline scripts are pinned by hash from
/// `csp.json`. The one extra host is the Calendly frame on the free assessment's last step.
const CSP_EXTRAS: spa::CspExtras = spa::CspExtras {
    script_src: &[],
    connect_src: &[],
    img_src: &[],
    // The free assessment's last step embeds the sales Calendly as a plain iframe (no script).
    frame_src: &["https://calendly.com"],
};

/// Release caches the parsed manifest here; debug re-reads it per request so a
/// `just build-ui` is picked up without a restart. See `spa::serve`.
static SPA: OnceLock<Spa> = OnceLock::new();

async fn static_handler(req: Request<Body>) -> Response {
    spa::serve::<OssAssets>(&req, &SPA, &CSP_EXTRAS)
}

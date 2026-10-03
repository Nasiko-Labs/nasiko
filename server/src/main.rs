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
#[folder = "$NASIKO_UI/oss/dist/"]
struct OssAssets;

/// `depends_on: condition: service_healthy` guarantees Postgres itself is
/// ready, but the container's own DNS resolution can still have a brief
/// post-start hiccup unrelated to Postgres's readiness — especially under
/// alternative Docker backends (OrbStack, Colima) — surfacing as a
/// "temporary failure in name resolution" rather than a connection refusal.
/// Retrying here absorbs that instead of crashing the whole server on a
/// transient blip.
async fn connect_to_postgres_with_retry(database_url: &str) -> sqlx::PgPool {
    const MAX_ATTEMPTS: u32 = 10;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(50)
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

    let telemetry_config = TelemetryConfig::from_env();
    init_telemetry(&telemetry_config);

    let config = nasiko_config::Config::from_env().expect("invalid config");
    config
        .validate_secrets_key()
        .expect("invalid SECRETS_ENCRYPTION_KEY at startup");
    let bind = config.bind.clone();

    // Build DB pool early so it can be shared with auth services. Raised from sqlx's
    // default of 10 — load testing showed 10 saturates under a few hundred concurrent
    // requests (server CPU stays idle while sqlx's own acquire-timeout logs show
    // requests queuing tens of seconds for a connection).
    let db = connect_to_postgres_with_retry(&config.database_url).await;

    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
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

    nasiko_server::state::AppState::run_migrations(&db).await;
    let state =
        nasiko_server::state::AppState::from_config_with_db(config, auth, runtime, oci_storage, db)
            .await;
    state.init().await;
    let app = nasiko_server::build_app(state, static_handler);

    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    tracing::info!("nasiko-server (OSS) listening on {bind}");
    axum::serve(listener, app).await.unwrap();
}

/// The OSS shell loads the Reo analytics snippet, which injects a `<script>`
/// pointing at this host. The hash of the inline loader itself comes from
/// `csp.json`; the host it reaches for has to be named here.
///
/// `connect_src` is deliberately empty: `reo.js` chooses its own beacon
/// endpoints at runtime, and docs/designs/openruntime-embedding-recommendations.md
/// is explicit that those hosts must come from a browser network trace rather
/// than a guess. Until someone takes that trace, analytics beacons are blocked
/// and the app is unaffected.
const CSP_EXTRAS: spa::CspExtras = spa::CspExtras {
    script_src: &["https://static.reo.dev"],
    connect_src: &[],
    img_src: &[],
};

/// Release caches the parsed manifest here; debug re-reads it per request so a
/// `just build-ui` is picked up without a restart. See `spa::serve`.
static SPA: OnceLock<Spa> = OnceLock::new();

async fn static_handler(req: Request<Body>) -> Response {
    spa::serve::<OssAssets>(&req, &SPA, &CSP_EXTRAS)
}

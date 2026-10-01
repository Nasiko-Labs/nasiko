use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nasiko_server::telemetry::{TelemetryConfig, init_telemetry};
use rust_embed::Embed;

// `NASIKO_UI` is resolved by build.rs — see the comment there for why these
// paths cannot be literals (this crate sits at a different depth in the
// public repo, where the `oss/` prefix is stripped).
#[derive(Embed)]
#[folder = "$NASIKO_UI/oss/"]
struct OssAssets;

#[derive(Embed)]
#[folder = "$NASIKO_UI/common/"]
#[prefix = "common/"]
struct CommonAssets;

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
    // Clear stale coding-agent session leases left by a prior crash, then keep
    // reconciling session rows against live containers in the background.
    nasiko_server::coding::spawn_reconciler(state.clone());
    let app = nasiko_server::build_app(state, static_handler);

    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    tracing::info!("nasiko-server (OSS) listening on {bind}");
    axum::serve(listener, app).await.unwrap();
}

/// Short max-age lets repeat page loads skip the network entirely, while
/// `must-revalidate` + the ETag bound staleness after a deploy to ~5 minutes
/// instead of relying on users to hard-refresh (assets aren't content-hashed,
/// so a stale cached JS/CSS file would silently run against a new backend).
/// 5 min is safe at a once-a-day deploy cadence; revisit if deploys get more frequent.
// Debug builds serve from disk (rust-embed), so nothing is cached there at all:
// `just run` is for editing the frontend, and a UI change must show up on the
// next reload with no hard-refresh and no stale module. `no-store` rather than
// `no-cache` because the latter still stores and revalidates, which leaves room
// for a stale ES module to be reused. Use `just run-prod` to exercise the
// release headers below.
const STATIC_CACHE_CONTROL: &str = if cfg!(debug_assertions) {
    "no-store"
} else {
    "max-age=300, must-revalidate"
};

async fn static_handler(req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = OssAssets::get(path).or_else(|| CommonAssets::get(path)) {
        let etag = format!("\"{}\"", hex::encode(file.metadata.sha256_hash()));
        if req
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            == Some(etag.as_str())
        {
            return (
                StatusCode::NOT_MODIFIED,
                [
                    (header::CACHE_CONTROL, STATIC_CACHE_CONTROL.to_string()),
                    (header::ETAG, etag),
                ],
            )
                .into_response();
        }

        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            [
                (header::CONTENT_TYPE, mime.as_ref().to_string()),
                (header::CACHE_CONTROL, STATIC_CACHE_CONTROL.to_string()),
                (header::ETAG, etag),
            ],
            file.data,
        )
            .into_response();
    }

    // SPA fallback: serve index.html for any path that isn't a real static
    // file. The client-side router resolves the URL to the correct page
    // component. Paths with file extensions (CSS, JS, images, fonts) are
    // genuine 404s — they were requested as assets and should not get HTML.
    if !path.contains('.')
        && let Some(file) = OssAssets::get("index.html")
    {
        let etag = format!("\"{}\"", hex::encode(file.metadata.sha256_hash()));
        return (
            [
                (header::CONTENT_TYPE, "text/html".to_string()),
                // SPA shell must revalidate on every navigation so deploys
                // take effect within one page load.
                (header::CACHE_CONTROL, "no-cache".to_string()),
                (header::ETAG, etag),
            ],
            file.data,
        )
            .into_response();
    }

    if let Some(file) = OssAssets::get("404.html") {
        return (
            StatusCode::NOT_FOUND,
            [
                (header::CONTENT_TYPE, "text/html".to_owned()),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            file.data,
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

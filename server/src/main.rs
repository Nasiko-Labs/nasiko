use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nasiko_server::telemetry::{TelemetryConfig, init_telemetry};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../ui/oss/"]
struct OssAssets;

#[derive(Embed)]
#[folder = "../ui/common/"]
#[prefix = "common/"]
struct CommonAssets;

async fn static_handler(req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = OssAssets::get(path).or_else(|| CommonAssets::get(path)) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response();
    }

    if let Some(file) = OssAssets::get("404.html") {
        return (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/html")], file.data).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

#[tokio::main]
async fn main() {
    let telemetry_config = TelemetryConfig::from_env();
    init_telemetry(&telemetry_config);

    let config = nasiko_config::Config::from_env().expect("invalid config");
    let bind = config.bind.clone();

    // Build DB pool early so it can be shared with auth services.
    let db = sqlx::PgPool::connect(&config.database_url)
        .await
        .expect("failed to connect to postgres");

    // Select auth provider based on whether JWT_SECRET is configured.
    let auth: Arc<dyn nasiko_auth::AuthProvider> = if std::env::var("JWT_SECRET").is_ok() {
        Arc::new(nasiko_auth::SimpleJwtAuth::from_env())
    } else {
        tracing::warn!("JWT_SECRET not set — using passthrough auth (dev mode only)");
        Arc::new(nasiko_auth::SingleUserAuth)
    };

    let user_auth_svc = Arc::new(nasiko_auth::UserAuthServiceImpl::new(db.clone(), auth.clone()));

    let providers = nasiko_server::Providers {
        auth,
        acl: Arc::new(nasiko_auth::NoopAuthorizer),
        user_auth: user_auth_svc.clone(),
        token_svc: user_auth_svc,
    };

    let runtime: Arc<dyn nasiko_runtime::ContainerRuntime> = Arc::new(
        nasiko_server::runtime::build_docker_runtime(&config)
            .await
            .expect("failed to create Docker runtime"),
    );

    let state = nasiko_server::state::AppState::from_config_with_db(config, providers, runtime, db).await;
    let app = nasiko_server::build_app(state, static_handler);

    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    tracing::info!("nasiko-server (OSS) listening on {bind}");
    axum::serve(listener, app).await.unwrap();
}

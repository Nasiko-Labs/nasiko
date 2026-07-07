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

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let telemetry_config = TelemetryConfig::from_env();
    init_telemetry(&telemetry_config);

    let config = nasiko_config::Config::from_env().expect("invalid config");
    config
        .validate_secrets_key()
        .expect("invalid SECRETS_ENCRYPTION_KEY at startup");
    let bind = config.bind.clone();

    // Build DB pool early so it can be shared with auth services.
    let db = sqlx::PgPool::connect(&config.database_url)
        .await
        .expect("failed to connect to postgres");

    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
    let auth: Arc<dyn nasiko_auth::AuthService> =
        Arc::new(nasiko_auth::AuthServiceImpl::new(db.clone(), jwt_secret));

    let runtime: Arc<dyn nasiko_runtime::ContainerRuntime> = Arc::new(
        nasiko_server::runtime::build_docker_runtime(&config)
            .await
            .expect("failed to create Docker runtime"),
    );

    let state =
        nasiko_server::state::AppState::from_config_with_db(config, auth, runtime, db).await;
    state.init().await;
    let app = nasiko_server::build_app(state, static_handler);

    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    tracing::info!("nasiko-server (OSS) listening on {bind}");
    axum::serve(listener, app).await.unwrap();
}

async fn static_handler(req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = OssAssets::get(path).or_else(|| CommonAssets::get(path)) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response();
    }

    if let Some(file) = OssAssets::get("404.html") {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html")],
            file.data,
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

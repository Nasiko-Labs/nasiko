use std::sync::Arc;

use axum::http::StatusCode;
use nasiko_server::telemetry::{TelemetryConfig, init_telemetry};

#[tokio::main]
async fn main() {
    let telemetry_config = TelemetryConfig::from_env();
    init_telemetry(&telemetry_config);

    let config = nasiko_config::Config::from_env().expect("invalid config");
    let bind = config.bind.clone();

    let db = sqlx::PgPool::connect(&config.database_url)
        .await
        .expect("failed to connect to postgres");

    let auth: Arc<dyn nasiko_auth::AuthProvider> = if std::env::var("JWT_SECRET").is_ok() {
        Arc::new(nasiko_auth::SimpleJwtAuth::from_env())
    } else {
        tracing::warn!("JWT_SECRET not set — using passthrough auth (dev mode only)");
        Arc::new(nasiko_auth::SingleUserAuth)
    };

    let user_auth_svc = Arc::new(nasiko_auth::UserAuthServiceImpl::new(
        db.clone(),
        auth.clone(),
    ));

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

    let state =
        nasiko_server::state::AppState::from_config_with_db(config, providers, runtime, db).await;
    let app = nasiko_server::build_app(state, || async { StatusCode::NOT_FOUND });

    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    tracing::info!("nasiko-server (OSS) listening on {bind}");
    axum::serve(listener, app).await.unwrap();
}

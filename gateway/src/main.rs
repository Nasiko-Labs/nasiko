use std::sync::Arc;

use nasiko_flow::{FlowConfig, FlowGuard};
use nasiko_gateway::config::GatewayConfig;
use nasiko_gateway::state::GatewayState;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,nasiko_gateway=debug".parse().unwrap()),
        )
        .init();

    let config = GatewayConfig::from_env();

    tracing::info!(
        listen = %config.listen_addr,
        upstream = %config.server_upstream,
        "Starting nasiko gateway"
    );

    // Redis → flow guard
    let redis_client =
        redis::Client::open(config.redis_url.as_str()).expect("invalid REDIS_URL");
    let flow_guard = FlowGuard::new(redis_client, FlowConfig::from_env());

    // Auth provider (OSS: SimpleJwtAuth)
    let auth: Arc<dyn nasiko_auth::AuthService> = Arc::new(nasiko_auth::SimpleJwtAuth {
        secret: config.jwt_secret.clone(),
        expiry_secs: nasiko_auth::jwt::DEFAULT_EXPIRY_SECS,
    });

    // Container runtime
    let runtime: Arc<dyn nasiko_runtime::ContainerRuntime> = Arc::new(
        nasiko_runtime::DockerRuntime::new(nasiko_runtime::DockerRuntimeConfig::default())
            .await
            .expect("failed to create Docker runtime"),
    );

    let state = GatewayState::from_config(&config, auth, runtime, flow_guard).await;
    let app = nasiko_gateway::build_app(state);

    let listener = tokio::net::TcpListener::bind(&config.listen_addr)
        .await
        .unwrap();
    tracing::info!("Listening on {}", config.listen_addr);
    axum::serve(listener, app).await.unwrap();
}

use nasiko_observability::{TelemetryConfig, init_telemetry};

#[tokio::main]
async fn main() {
    let telemetry_config = TelemetryConfig::from_env();
    init_telemetry(&telemetry_config);

    let app = nasiko_server::build_app();

    let bind = std::env::var("SERVER_BIND").unwrap_or_else(|_| "0.0.0.0:9090".into());
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
    tracing::info!("nasiko-server listening on {bind}");
    axum::serve(listener, app).await.unwrap();
}

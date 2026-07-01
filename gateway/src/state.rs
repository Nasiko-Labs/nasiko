use std::sync::Arc;

use nasiko_flow::FlowGuard;

use crate::config::GatewayConfig;

#[derive(Clone)]
pub struct GatewayState {
    pub db: sqlx::PgPool,
    pub auth: Arc<dyn nasiko_auth::AuthProvider>,
    pub runtime: Arc<dyn nasiko_runtime::ContainerRuntime>,
    pub flow_guard: FlowGuard,
    pub http_client: reqwest::Client,
    /// The upstream server (control plane) base URL.
    pub server_upstream: String,
}

impl GatewayState {
    pub async fn from_config(
        config: &GatewayConfig,
        auth: Arc<dyn nasiko_auth::AuthProvider>,
        runtime: Arc<dyn nasiko_runtime::ContainerRuntime>,
        flow_guard: FlowGuard,
    ) -> Self {
        let db = sqlx::PgPool::connect(&config.database_url)
            .await
            .expect("failed to connect to postgres");

        let http_client = reqwest::Client::new();

        Self {
            db,
            auth,
            runtime,
            flow_guard,
            http_client,
            server_upstream: config.server_upstream.clone(),
        }
    }
}

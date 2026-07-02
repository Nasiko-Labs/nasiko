use std::sync::Arc;

use nasiko_flow::{FlowEventBus, FlowGuard};

use crate::config::GatewayConfig;

#[derive(Clone)]
pub struct GatewayState {
    pub db: sqlx::PgPool,
    pub auth: Arc<dyn nasiko_auth::AuthService>,
    pub runtime: Arc<dyn nasiko_runtime::ContainerRuntime>,
    pub flow_guard: FlowGuard,
    pub flow_events: FlowEventBus,
    pub http_client: reqwest::Client,
    /// The upstream server (control plane) base URL.
    pub server_upstream: String,
}

impl GatewayState {
    pub async fn from_config(
        config: &GatewayConfig,
        auth: Arc<dyn nasiko_auth::AuthService>,
        runtime: Arc<dyn nasiko_runtime::ContainerRuntime>,
        flow_guard: FlowGuard,
    ) -> Self {
        let db = sqlx::PgPool::connect(&config.database_url)
            .await
            .expect("failed to connect to postgres");

        Self {
            db,
            auth,
            runtime,
            flow_guard,
            flow_events: FlowEventBus::new(),
            http_client: reqwest::Client::new(),
            server_upstream: config.server_upstream.clone(),
        }
    }
}

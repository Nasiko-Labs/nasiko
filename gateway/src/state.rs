use std::sync::Arc;

use nasiko_flow::{FlowEventBus, FlowGuard};
use nasiko_router::{OssRoutingEngine, RoutingEngine, RouterConfig, AgentRegistry};

use crate::config::GatewayConfig;

#[derive(Clone)]
pub struct GatewayState {
    pub db: sqlx::PgPool,
    pub auth: Arc<dyn nasiko_auth::AuthProvider>,
    pub runtime: Arc<dyn nasiko_runtime::ContainerRuntime>,
    pub flow_guard: FlowGuard,
    pub flow_events: FlowEventBus,
    pub http_client: reqwest::Client,
    pub routing_engine: Arc<dyn RoutingEngine>,
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

        let routing_engine: Arc<dyn RoutingEngine> = Arc::new(OssRoutingEngine::new(
            Arc::new(AgentRegistry::new(60)),
            RouterConfig {
                shortlist_threshold: std::env::var("ROUTER_SHORTLIST_THRESHOLD")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(15),
                shortlist_size: std::env::var("ROUTER_SHORTLIST_SIZE")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(10),
                max_history_messages: std::env::var("MAX_ROUTER_HISTORY_MESSAGES")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(20),
            },
            http_client.clone(),
            std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".into()),
            std::env::var("ROUTER_MODEL").unwrap_or_else(|_| "gpt-4o".into()),
            std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".into()),
            std::env::var("OLLAMA_EMBEDDING_MODEL").unwrap_or_else(|_| "nomic-embed-text".into()),
        ));

        Self {
            db,
            auth,
            runtime,
            flow_guard,
            flow_events: FlowEventBus::new(),
            http_client,
            routing_engine,
            server_upstream: config.server_upstream.clone(),
        }
    }
}
